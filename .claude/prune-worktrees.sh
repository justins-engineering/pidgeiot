#!/bin/bash
set -euo pipefail

# Safe worktree pruning for this repo. Dry-run by default; nothing is
# removed without --apply. Encodes two lessons a prior manual prune learned
# the hard way:
#
#   1. `git worktree remove` has no idea a worktree ever ran `docker compose
#      up`. A stack started from inside a worktree just keeps running after
#      the directory is gone -- one did, for ~20 hours, with Postgres and
#      Kratos's unauthenticated admin API bound to 0.0.0.0. So this script
#      always checks docker first and refuses to touch a worktree with a
#      container tied to it -- it never stops or removes containers itself,
#      that's a human call.
#
#   2. "Commits look unmerged" is usually a hash-comparison illusion.
#      `git cherry` marks a commit "+" whenever its hash differs, which is
#      true of anything that landed via cherry-pick or rebase even though
#      its *content* is already on main. Real equivalence is checked by
#      git-patch-id instead, and even that can miss (a later rename, a
#      transient merge artifact) -- so any doubt resolves toward keeping the
#      worktree, never removing it.
#
#   3. Patch-id equivalence is only trusted for throwaway
#      worktree-agent-*/worktree-leader-* branches, whose commits reach main
#      by cherry-pick and nothing else. A named branch is someone's feature
#      work: it may carry a rebase or squash whose content differs from what
#      landed, so it is prunable only once main actually contains its tip
#      (`merge-base --is-ancestor`), and only with a tree that has no
#      modified or untracked files. Build output is gitignored and never
#      counts as untracked; anything that does show up is a file a person
#      wrote. A named branch sitting exactly at main's tip has no commits of
#      its own, which looks the same whether it was just cut or just
#      fast-forwarded, so it stays too.
#
# Usage:
#   .claude/prune-worktrees.sh                    # dry run, changes nothing
#   .claude/prune-worktrees.sh --apply             # actually remove + delete branches
#   .claude/prune-worktrees.sh --idle-minutes N    # override the 30-minute freshness guard

APPLY=0
IDLE_MINUTES=30

while [ $# -gt 0 ]; do
  case "$1" in
    --apply)
      APPLY=1
      shift
      ;;
    --idle-minutes)
      IDLE_MINUTES="$2"
      shift 2
      ;;
    --idle-minutes=*)
      IDLE_MINUTES="${1#*=}"
      shift
      ;;
    -h|--help)
      echo "usage: $0 [--apply] [--idle-minutes N]"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

MAIN_BRANCH="main"
WORKTREES_DIR="$REPO_ROOT/.claude/worktrees"
THROWAWAY_RE='^worktree-agent-|^worktree-leader-'

echo "==== docker preflight ===="
DOCKER_UNAVAILABLE=0
declare -A DOCKER_HIT=()
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  HITS=0
  while IFS=$'\t' read -r cid cname cproject cwdir; do
    [ -n "$cid" ] || continue
    for wt_path in "$WORKTREES_DIR"/*/; do
      [ -d "$wt_path" ] || continue
      wt_real="$(cd "$wt_path" && pwd)"
      hit=""
      if [ -n "$cwdir" ] && { [ "$cwdir" = "$wt_real" ] || [[ "$cwdir" == "$wt_real"/* ]]; }; then
        hit="working_dir=$cwdir"
      elif [ -n "$cproject" ] && [[ "$cproject" == *"$(basename "$wt_real")"* ]]; then
        hit="project name '$cproject' matches worktree dir (compose working_dir label absent)"
      fi
      if [ -n "$hit" ]; then
        echo "  WARNING: container '$cname' ($cid) looks tied to $wt_real -- $hit"
        DOCKER_HIT["$wt_real"]="docker container '$cname' still running ($hit) -- stop it yourself first"
        HITS=$((HITS + 1))
      fi
    done
  done < <(docker ps -a --format '{{.ID}}	{{.Names}}	{{.Label "com.docker.compose.project"}}	{{.Label "com.docker.compose.project.working_dir"}}' 2>/dev/null || true)
  [ "$HITS" -gt 0 ] || echo "  no containers tied to any worktree"
else
  echo "  WARNING: docker unreachable -- cannot rule out a live stack, every worktree is protected this run"
  DOCKER_UNAVAILABLE=1
fi
echo

# Full patch-id set for everything already on main, computed once. Matching
# against this (not `git cherry`'s hash comparison) is what makes the
# merge-proof below trustworthy for cherry-picked/rebased commits.
declare -A MAIN_PATCH_IDS=()
while read -r pid _sha; do
  [ -n "$pid" ] || continue
  MAIN_PATCH_IDS["$pid"]=1
done < <(git log -p "$MAIN_BRANCH" | git patch-id --stable)

# Prints one line per offending commit and returns non-zero if any commit
# unique to $branch has no patch-id match in main's history. A branch whose
# HEAD is already an ancestor of main is trivially fully merged.
commits_provably_on_main() {
  local branch="$1"
  if git merge-base --is-ancestor "$branch" "$MAIN_BRANCH" 2>/dev/null; then
    return 0
  fi
  local total seen ok
  total="$(git rev-list --count "$MAIN_BRANCH..$branch")"
  seen=0
  ok=0
  while read -r pid sha; do
    [ -n "$sha" ] || continue
    seen=$((seen + 1))
    if [ -n "${MAIN_PATCH_IDS[$pid]:-}" ]; then
      continue
    fi
    echo "    unmatched commit $sha (patch-id $pid) -- not provably on $MAIN_BRANCH"
    ok=1
  done < <(git log -p "$MAIN_BRANCH..$branch" | git patch-id --stable)
  # `git log -p` omits diffs for merge commits by default, so a merge would
  # otherwise vanish from patch-id's output rather than showing as a
  # mismatch. Catch that by checking every unique commit got a patch-id.
  if [ "$seen" -lt "$total" ]; then
    echo "    $((total - seen)) commit(s) produced no diff for patch-id (merge commit?) -- not provably on $MAIN_BRANCH"
    ok=1
  fi
  [ "$ok" -eq 0 ]
}

# Joins up to the first 3 lines of $1 with spaces, appending "..." if there
# were more. Always exits 0 (a bare `[ ] && echo` here would return the
# test's own false status when short of 3 lines, which set -e treats as a
# failure even inside a command substitution).
summarize_lines() {
  local text="$1" n
  n="$(printf '%s\n' "$text" | grep -c .)"
  printf '%s' "$(printf '%s\n' "$text" | head -3 | tr '\n' ' ')"
  if [ "$n" -gt 3 ]; then printf '...'; fi
  return 0
}

# Parse `git worktree list --porcelain` into parallel arrays.
WT_PATH=()
WT_HEAD=()
WT_BRANCH=()
WT_LOCKED=()
cur_path="" cur_head="" cur_branch="" cur_locked=0
flush_wt() {
  [ -n "$cur_path" ] || return 0
  WT_PATH+=("$cur_path")
  WT_HEAD+=("$cur_head")
  WT_BRANCH+=("$cur_branch")
  WT_LOCKED+=("$cur_locked")
  cur_path="" cur_head="" cur_branch="" cur_locked=0
}
while IFS= read -r line; do
  case "$line" in
    "worktree "*) flush_wt; cur_path="${line#worktree }" ;;
    "HEAD "*) cur_head="${line#HEAD }" ;;
    "branch "*) cur_branch="${line#branch refs/heads/}" ;;
    "detached") cur_branch="(detached)" ;;
    "locked"*) cur_locked=1 ;;
    "") flush_wt ;;
  esac
done < <(git worktree list --porcelain)
flush_wt

DU_BEFORE="$(du -sh "$WORKTREES_DIR" 2>/dev/null | cut -f1)"
MAIN_TIP="$(git rev-parse --verify "$MAIN_BRANCH")"

# Evaluate every worktree except the primary checkout (REPO_ROOT itself,
# which `git worktree remove` can't touch anyway). Reasons are checked in
# priority order and short-circuit on the first hard keep found.
ROW_PATH=() ROW_BRANCH=() ROW_STATUS=() ROW_REASON=() ROW_NOTE=() ROW_KB=()
CUTOFF_REF="$(mktemp)"
touch -d "@$(( $(date +%s) - IDLE_MINUTES * 60 ))" "$CUTOFF_REF"

for i in "${!WT_PATH[@]}"; do
  path="${WT_PATH[$i]}"
  [ "$path" != "$REPO_ROOT" ] || continue
  branch="${WT_BRANCH[$i]}"
  status="PRUNE"
  reason=""
  note=""
  throwaway=0
  if [[ "$branch" =~ $THROWAWAY_RE ]]; then throwaway=1; fi

  if [ "${WT_LOCKED[$i]}" = "1" ]; then
    status="KEEP"
    reason="git worktree lock is set"

  elif [ "$DOCKER_UNAVAILABLE" = "1" ]; then
    status="KEEP"
    reason="docker preflight could not run -- refusing to guess"

  elif [ -n "${DOCKER_HIT[$path]:-}" ]; then
    status="KEEP"
    reason="${DOCKER_HIT[$path]}"

  elif [ "$branch" = "(detached)" ]; then
    status="KEEP"
    reason="detached HEAD -- no branch to judge against $MAIN_BRANCH"

  else
    dirty="$(git -C "$path" status --porcelain=v1 2>/dev/null | grep -v '^??' || true)"
    untracked="$(git -C "$path" status --porcelain=v1 2>/dev/null | grep '^??' || true)"
    if [ -n "$dirty" ]; then
      status="KEEP"
      reason="modified tracked files: $(summarize_lines "$dirty")"
    elif [ -n "$untracked" ] && [ "$throwaway" = "0" ]; then
      status="KEEP"
      reason="untracked files not covered by .gitignore: $(summarize_lines "$untracked")"
    elif [ -n "$untracked" ]; then
      note="untracked (non-blocking): $(summarize_lines "$untracked")"
    fi

    if [ "$status" = "PRUNE" ]; then
      recent="$(find "$path" -path "$path/.git" -prune -o -type f -newer "$CUTOFF_REF" -print -quit 2>/dev/null || true)"
      if [ -n "$recent" ]; then
        status="KEEP"
        reason="modified within last ${IDLE_MINUTES}m (e.g. ${recent#"$path"/})"
      fi
    fi

    if [ "$status" = "PRUNE" ] && [ "$throwaway" = "1" ]; then
      if ! proof="$(commits_provably_on_main "$branch")"; then
        status="KEEP"
        reason="commits not provably on $MAIN_BRANCH (patch-id mismatch, see below)"
        note="${note:+$note; }$proof"
      fi

    elif [ "$status" = "PRUNE" ]; then
      if [ "$(git rev-parse --verify "$branch")" = "$MAIN_TIP" ]; then
        status="KEEP"
        reason="sits at $MAIN_BRANCH's tip with no commits of its own -- just cut or just fast-forwarded, can't tell which"
      elif ! git merge-base --is-ancestor "$branch" "$MAIN_BRANCH" 2>/dev/null; then
        status="KEEP"
        reason="$(git rev-list --count "$MAIN_BRANCH..$branch") commit(s) not in $MAIN_BRANCH -- a named branch needs a real ancestor, not a patch-id match"
        if proof="$(commits_provably_on_main "$branch")"; then
          note="${note:+$note; }every commit's content is already on $MAIN_BRANCH by patch-id; delete the branch by hand if it's done"
        else
          note="${note:+$note; }$proof"
        fi
      else
        reason="merged into $MAIN_BRANCH (ancestor), tree clean"
      fi
    fi
  fi

  ROW_PATH+=("$path")
  ROW_BRANCH+=("$branch")
  ROW_STATUS+=("$status")
  ROW_REASON+=("${reason:-fully merged into $MAIN_BRANCH, clean, idle > ${IDLE_MINUTES}m}")
  ROW_NOTE+=("$note")
  ROW_KB+=("$(du -sk "$path" 2>/dev/null | cut -f1)")
done
rm -f "$CUTOFF_REF"

echo "==== worktree survey ===="
printf '%-7s  %7s  %-32s  %-42s  %s\n' "STATUS" "SIZE" "BRANCH" "PATH" "REASON"
PRUNE_KB=0
PRUNE_N=0
for i in "${!ROW_PATH[@]}"; do
  kb="${ROW_KB[$i]:-0}"
  printf '%-7s  %7s  %-32s  %-42s  %s\n' "${ROW_STATUS[$i]}" "$(numfmt --from-unit=1024 --to=iec "$kb")" \
    "${ROW_BRANCH[$i]}" "${ROW_PATH[$i]#"$REPO_ROOT"/}" "${ROW_REASON[$i]}"
  if [ -n "${ROW_NOTE[$i]}" ]; then
    while IFS= read -r note_line; do
      printf '           %s\n' "$note_line"
    done <<< "${ROW_NOTE[$i]}"
  fi
  if [ "${ROW_STATUS[$i]}" = "PRUNE" ]; then
    PRUNE_KB=$((PRUNE_KB + kb))
    PRUNE_N=$((PRUNE_N + 1))
  fi
done
echo
echo "PRUNE rows: $PRUNE_N worktree(s), $(numfmt --from-unit=1024 --to=iec "$PRUNE_KB") on disk"
echo

if [ "$APPLY" = "0" ]; then
  echo "==== DRY RUN -- no changes made. Re-run with --apply to remove PRUNE rows above. ===="
  git worktree prune -n -v
else
  echo "==== applying ===="
  for i in "${!ROW_PATH[@]}"; do
    [ "${ROW_STATUS[$i]}" = "PRUNE" ] || continue
    path="${ROW_PATH[$i]}"
    branch="${ROW_BRANCH[$i]}"
    echo "removing worktree $path (branch $branch)"
    if ! git worktree remove "$path" 2>/dev/null; then
      echo "  clean removal failed (likely stray untracked files, already confirmed no tracked changes) -- forcing"
      git worktree remove --force "$path"
    fi
    echo "deleting branch $branch"
    git branch -D "$branch"
  done
  git worktree prune -v
fi

echo
echo "==== final state ===="
git worktree list
DU_AFTER="$(du -sh "$WORKTREES_DIR" 2>/dev/null | cut -f1)"
echo "worktrees dir size: $DU_BEFORE -> $DU_AFTER"
