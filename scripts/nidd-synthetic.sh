#!/usr/bin/env bash
#
# The NIDD connector's Tier 1 synthetic suite (docs/design/nidd-connector.md, section 13.1):
# callbacks in Verizon ThingSpace's shape, posted to a local `wrangler dev --env dev`, around
# frames laid out as docs/api.md's "NIDD frames" says, with the dashboard driven through fresh
# dev Kratos sessions. No device and no Verizon. Dev never holds the ThingSpace API secrets, so
# every downlink the suite provokes is planned and then refused by the session object as
# `not_configured`; the `nidd_dl` lines in the wrangler log are where a planned frame shows.
#
# Some steps need NIDD configured differently, so wrangler dev is started five times: with the
# callback password blanked, with the callback allowlist and the account name blanked, fully
# configured, with every telemetry store failing, and fully configured again. Each sets values
# with --var, which wrangler applies over dovecote/.dev.vars, so no file changes.
# GREPTIMEDB_ENDPOINT is blanked too, so history lands in Postgres as it does in every deployed
# environment.
#
# Needs the dev stack up (infra/docker-compose.yml), dovecote/.dev.vars holding dev-only
# THINGSPACE_CALLBACK_PASSWORD and THINGSPACE_ACCOUNT_NAME, and curl, jq, psql, sqlite3, xxd and
# bunx. Every run registers two identities and uses random test IMEIs, so it can be rerun.
# Secrets reach curl and jq through files and the environment, never through arguments, and
# never reach the evidence.
#
# Usage: scripts/nidd-synthetic.sh [evidence-dir]
# Writes step-NN.txt per step, the wrangler logs and summary.txt; exits 1 on any failure.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dovecote="$root/dovecote"
ev="${1:-$(mktemp -d /tmp/nidd-synthetic.XXXXXX)}"
mkdir -p "$ev"
: >"$ev/summary.txt"
work="$(mktemp -d)"
chmod 700 "$work"
ua="pidgeiot-nidd-synthetic/1"
kratos="http://127.0.0.1:4433"
do_state="$dovecote/.wrangler/state/v3/do/dovecote-dev-Pigeons"

die() {
  echo "nidd-synthetic: $*" >&2
  exit 2
}

dev_var() { sed -n "s/^$1=//p" "$dovecote/.dev.vars" 2>/dev/null; }
cb_password="$(dev_var THINGSPACE_CALLBACK_PASSWORD)"
account="$(dev_var THINGSPACE_ACCOUNT_NAME)"
[[ -n $cb_password && -n $account ]] ||
  die "dovecote/.dev.vars needs THINGSPACE_CALLBACK_PASSWORD and THINGSPACE_ACCOUNT_NAME"

# psql takes the dev connection from the environment, so the string never reaches an argument.
dsn="$(sed -n 's/^localConnectionString = "\(.*\)"$/\1/p' "$dovecote/wrangler.toml")"
[[ $dsn =~ ^postgres(ql)?://([^:]+):([^@]+)@([^:/]+):([0-9]+)/([^?]+) ]] ||
  die "no dev localConnectionString in dovecote/wrangler.toml"
export PGUSER="${BASH_REMATCH[2]}" PGPASSWORD="${BASH_REMATCH[3]}" PGHOST="${BASH_REMATCH[4]}"
export PGPORT="${BASH_REMATCH[5]}" PGDATABASE="${BASH_REMATCH[6]}"
sql() { psql -X -qAt -v ON_ERROR_STOP=1 -c "$1"; }
bucket="$(sed -n '/^\[\[env.dev.r2_buckets\]\]/,/^$/s/^bucket_name = "\(.*\)"$/\1/p' \
  "$dovecote/wrangler.toml")"

port=8787
if ss -ltn "sport = :$port" | grep -q LISTEN; then port=8788; fi
ss -ltn "sport = :$port" | grep -q LISTEN && die "ports 8787 and 8788 are both in use"
base="http://127.0.0.1:$port"

# --- wrangler dev ---

wrangler_pid=""
wlog=""

# start_wrangler <phase> [--var NAME:VALUE ...]
start_wrangler() {
  local phase=$1
  shift
  wlog="$ev/wrangler-$phase.log"
  local extra=()
  if [[ $port != 8787 ]]; then extra=(--var "DEVICE_API_HOST:127.0.0.1:$port"); fi
  # Its own process group, so stopping it stops workerd too.
  set -m
  (cd "$dovecote" && exec bunx wrangler dev --ip 127.0.0.1 --port "$port" --env dev \
    --var GREPTIMEDB_ENDPOINT: "${extra[@]}" "$@") >"$wlog" 2>&1 </dev/null &
  wrangler_pid=$!
  set +m
  echo "wrangler dev ($phase) starting on $base"
  for _ in $(seq 1 180); do
    if curl -s -o /dev/null -m 2 "$base/"; then return 0; fi
    kill -0 "$wrangler_pid" 2>/dev/null || die "wrangler dev ($phase) exited; see $wlog"
    sleep 5
  done
  die "wrangler dev ($phase) did not come up; see $wlog"
}

stop_wrangler() {
  [[ -n $wrangler_pid ]] || return 0
  kill -TERM -- "-$wrangler_pid" 2>/dev/null || true
  for _ in $(seq 1 20); do
    pgrep -g "$wrangler_pid" >/dev/null || break
    sleep 1
  done
  kill -KILL -- "-$wrangler_pid" 2>/dev/null || true
  wrangler_pid=""
}

usage_restore=""
holder_pid=""
cleanup() {
  if [[ -n $usage_restore ]]; then sql "$usage_restore" >/dev/null || true; fi
  if [[ -n $holder_pid ]]; then kill "$holder_pid" 2>/dev/null || true; fi
  stop_wrangler
  rm -rf "$work"
}
trap cleanup EXIT

mark() { wc -c <"$wlog"; }

# log_since <mark>: the NIDD-relevant wrangler log lines written since the mark, colour stripped.
log_since() {
  tail -c +"$(($1 + 1))" "$wlog" | sed 's/\x1b\[[0-9;]*m//g' |
    grep -E 'nidd_|NIDD|thingspace|ThingSpace|wrangler:info|rror' || true
}

# wait_log <mark> <pattern>: whether a matching line appears within ten seconds.
wait_log() {
  for _ in $(seq 1 20); do
    if log_since "$1" | grep -qE "$2"; then return 0; fi
    sleep 0.5
  done
  return 1
}

# --- evidence ---

step_no=0
step_file=""
passes=0
fails=0

step() {
  step_no=$1
  step_file="$ev/step-$(printf '%02d' "$1").txt"
  printf '\n== Step %s: %s\n' "$1" "$2" | tee -a "$step_file"
}
note() { printf '%s\n' "$*" >>"$step_file"; }
note_log() {
  note "  wrangler log:"
  log_since "$1" | sed 's/^/    /' >>"$step_file"
}
pass() {
  passes=$((passes + 1))
  printf 'PASS [%s] %s\n' "$step_no" "$*" | tee -a "$step_file" "$ev/summary.txt"
}
fail() {
  fails=$((fails + 1))
  printf 'FAIL [%s] %s\n' "$step_no" "$*" | tee -a "$step_file" "$ev/summary.txt"
}
# expect <what> <want> <got>
expect() {
  if [[ $3 == "$2" ]]; then pass "$1: $3"; else fail "$1: want $2, got $3"; fi
}
# expect_log <what> <mark> <pattern>
expect_log() {
  if wait_log "$2" "$3"; then pass "$1"; else fail "$1: no log line /$3/"; fi
}
# expect_no_log <what> <mark> <pattern>: waits three seconds, then checks nothing matched.
expect_no_log() {
  sleep 3
  if log_since "$2" | grep -qE "$3"; then fail "$1: unexpected /$3/"; else pass "$1"; fi
}

# --- HTTP ---

resp="$work/resp"
status=""

# http <label> <curl args...>: the status lands in $status, the body in $resp.
http() {
  local label=$1
  shift
  status=$(curl -sS -A "$ua" -o "$resp" -w '%{http_code}' "$@") || status=000
  note "> $label -> $status"
}

# api <who> <method> <path> [json]: a dashboard call with that identity's session.
api() {
  local who=$1 method=$2 path=$3 body=${4-}
  local args=(-b "$work/jar-$who" -X "$method")
  if [[ -n $body ]]; then args+=(-H 'Content-Type: application/json' --data-binary "$body"); fi
  http "$who $method $path" "${args[@]}" "$base$path"
}

# callback <label>: posts $work/body, a callback JSON, to the ThingSpace route.
callback() {
  http "POST /internal/thingspace/nidd $1" -X POST -H 'Content-Type: application/json' \
    --data-binary @"$work/body" "$base/internal/thingspace/nidd"
}

# --- frames and bodies ---

# frame <type-hex> <body>: a device frame in $work/frame, the type byte then the body.
frame() { { printf '%s' "$1" | xxd -r -p; printf '%s' "$2"; } >"$work/frame"; }

# hello <claim-key-hex>: a HELLO frame, the type byte then the key's 32 hex characters.
hello() { { printf '\x04'; printf '%s' "$1"; } >"$work/frame"; }

# uplink_body <request-id> <attempt> <imei> <iccid>: $work/frame as ThingSpace's MO callback, on
# stdout; the line is named by the ICCID in the inner identifier list.
uplink_body() {
  local rid=$1 attempt=$2 imei=$3 line=$4
  base64 -w0 <"$work/frame" |
    NIDD_PW=$cb_password NIDD_ACCT=$account jq -cR --arg rid "$rid" --argjson n "$attempt" \
      --arg imei "$imei" --arg line "$line" '
      . as $msg
      | {username: "pidgeiot", password: $ENV.NIDD_PW, requestId: $rid,
         deviceIds: [{id: $imei, kind: "IMEI"}],
         niddResponse: {niddMONotificationResponse: {accountName: $ENV.NIDD_ACCT,
           message: $msg,
           deviceIds: ([{id: $imei, kind: "IMEI"}]
             + if $line == "" then [] else [{id: $line, kind: "ICCID"}] end)}},
         callbackCount: $n, maxCallbackThreshold: 4}'
}

# uplink <label> <request-id> <attempt> <imei> <iccid>: posts $work/frame as that callback.
uplink() {
  local label=$1 rid=$2 attempt=$3
  uplink_body "$rid" "$attempt" "$4" "$5" >"$work/body"
  callback "$label (request $rid, attempt $attempt, frame $(wc -c <"$work/frame") bytes)"
}

# post_async <body-file> <status-file>: posts a callback body in the background, its status
# landing in the file once it answers; the caller waits on $!.
post_async() {
  curl -sS -A "$ua" -o /dev/null -w '%{http_code}' -m 60 -X POST \
    -H 'Content-Type: application/json' --data-binary @"$1" \
    "$base/internal/thingspace/nidd" >"$2" 2>/dev/null &
}

# hold_history: a SHARE lock on pigeon_telemetry_history, taken by a psql kept open as a
# coprocess, so every history insert waits until release_history commits. Should the suite die
# holding it, cleanup kills that psql, and the server ends the transaction after a minute idle.
hold_history() {
  coproc HOLDER { PGAPPNAME=nidd-synthetic-hold exec psql -X -qAt -v ON_ERROR_STOP=1 \
    >/dev/null 2>&1; }
  holder_pid=$HOLDER_PID
  holder_in=${HOLDER[1]}
  printf '%s\n' "SET idle_in_transaction_session_timeout = '60s';" 'BEGIN;' \
    'LOCK TABLE pigeon_telemetry_history IN SHARE MODE;' >&"$holder_in"
  for _ in $(seq 1 20); do
    if [[ $(sql "SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid = l.relation
      JOIN pg_stat_activity a ON a.pid = l.pid WHERE c.relname = 'pigeon_telemetry_history'
      AND a.application_name = 'nidd-synthetic-hold' AND l.granted;") == 1 ]]; then
      note "> pigeon_telemetry_history locked in SHARE mode"
      return 0
    fi
    sleep 0.5
  done
  die "could not lock pigeon_telemetry_history"
}

release_history() {
  printf 'COMMIT;\n' >&"$holder_in"
  exec {holder_in}>&-
  wait "$holder_pid" || true
  holder_pid=""
  note "> pigeon_telemetry_history released"
}

# insert_held: whether a history insert is waiting on that lock, within ten seconds.
insert_held() {
  for _ in $(seq 1 20); do
    if (($(sql "SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid = l.relation
      WHERE c.relname = 'pigeon_telemetry_history' AND NOT l.granted;") > 0)); then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

# report_body <variant> <status> <reason> <imei> <iccid>: a delivery report or configuration
# result, in the shapes of Verizon's documented examples.
report_body() {
  NIDD_PW=$cb_password NIDD_ACCT=$account jq -cn --arg variant "$1" --arg status "$2" \
    --arg reason "$3" --arg rid "$6" --arg imei "$4" --arg line "$5" '
    {username: "pidgeiot", password: $ENV.NIDD_PW, requestId: $rid,
     deviceIds: [{id: $imei, kind: "IMEI"}],
     niddResponse: {($variant): ({accountName: $ENV.NIDD_ACCT,
       deviceIds: [{id: $imei, kind: "Imei"}, {id: $line, kind: "ICCID"}]}
       + if $reason == "" then {} else {reason: $reason} end)},
     status: $status, callbackCount: 1, maxCallbackThreshold: 4}' >"$work/body"
}

rid() { printf 'nidd-synth-%s-%s-%s' "$1" "$(date +%s)" "$RANDOM"; }

digits() {
  local out="" i
  for ((i = 0; i < $1; i++)); do out+=$((RANDOM % 10)); done
  printf '%s' "$out"
}

# luhn_imei <14-digit body>: the body and its Luhn check digit.
luhn_imei() {
  local body=$1 sum=0 i d
  for ((i = 0; i < 14; i++)); do
    d=${body:13-i:1}
    if ((i % 2 == 0)); then
      d=$((d * 2))
      if ((d > 9)); then d=$((d - 9)); fi
    fi
    sum=$((sum + d))
  done
  printf '%s%s' "$body" $(((10 - sum % 10) % 10))
}

[[ $(luhn_imei 49015420323751) == 490154203237518 ]] || die "luhn_imei is wrong"

imei_a="$(luhn_imei "00$(digits 12)")"
imei_q="$(luhn_imei "00$(digits 12)")"
imei_x="$(luhn_imei "00$(digits 12)")"
imei_num="$(luhn_imei "49$(digits 12)")"
imei_bad="${imei_a:0:14}$(((${imei_a:14:1} + 1) % 10))"
iccid1="8914800000$(digits 10)"
iccid2="8914800000$(digits 10)"
junk_key="$(openssl rand -hex 16)"
claim_keys=()

# nidd_state <pigeon-id> <columns>: the pigeon object's pigeon_nidd row, read-only.
nidd_state() {
  sqlite3 -readonly "$do_state/$1.sqlite" "SELECT $2 FROM pigeon_nidd WHERE id = 1;" 2>&1 || true
}

usage_of() {
  sql "SELECT coalesce(sum(billable_messages), 0) FROM billing_usage_periods
       WHERE owner_kind = 'org' AND owner_id = '$1';"
}

# wait_usage <org> <want>: the org's billed messages once they reach want, or after ten seconds.
wait_usage() {
  local got
  for _ in $(seq 1 20); do
    got=$(usage_of "$1")
    if [[ $got == "$2" ]]; then break; fi
    sleep 0.5
  done
  printf '%s' "$got"
}

epoch() { date -d "$1" +%s; }
iso_ago() { date -u -d "@$(($(date +%s) - $1))" +%Y-%m-%dT%H:%M:%SZ; }

# --- dev Kratos ---

# register <who>: a fresh identity through the self-service registration flow, profile step then
# password step; its session cookie stays in $work/jar-<who>.
register() {
  local who=$1 jar="$work/jar-$1" email flow_id csrf code
  email="nidd-synthetic-$who-$(date +%s)-$RANDOM@example.com"
  curl -sS -A "$ua" -c "$jar" -b "$jar" -H 'Accept: application/json' -o "$work/flow" \
    "$kratos/self-service/registration/browser"
  flow_id=$(jq -r .id "$work/flow")
  for method in profile password; do
    csrf=$(jq -r '.ui.nodes[] | select(.attributes.name == "csrf_token") | .attributes.value' \
      "$work/flow")
    NIDD_CSRF=$csrf NIDD_EMAIL=$email NIDD_UPW="$(openssl rand -base64 24)" jq -cn \
      --arg method "$method" '{method: $method, csrf_token: $ENV.NIDD_CSRF,
        traits: {email: $ENV.NIDD_EMAIL}}
        + if $method == "password" then {password: $ENV.NIDD_UPW} else {} end' >"$work/kbody"
    code=$(curl -sS -A "$ua" -c "$jar" -b "$jar" -o "$work/flow" -w '%{http_code}' \
      -H 'Content-Type: application/json' -H 'Accept: application/json' \
      --data-binary @"$work/kbody" "$kratos/self-service/registration?flow=$flow_id")
  done
  [[ $code == 200 && $(jq -r .session.active "$work/flow") == true ]] ||
    die "registering $who failed ($code)"
  note "> registered $who as $(jq -r .identity.id "$work/flow") ($email): session active"
}

# org_with_flock <who>: an organization owned by who and a flock moved into it.
org_with_flock() {
  local who=$1
  api "$who" POST /orgs "{\"name\":\"NIDD synthetic $who\"}"
  [[ $status == 201 ]] || die "POST /orgs for $who answered $status"
  org_id=$(jq -r .id "$resp")
  api "$who" POST /flocks "{\"name\":\"NIDD synthetic $who org flock\"}"
  [[ $status == 201 ]] || die "POST /flocks for $who answered $status"
  flock_id=$(jq -r .id "$resp")
  api "$who" POST "/flocks/$flock_id/transfer" "{\"org_id\":\"$org_id\"}"
  [[ $status == 200 ]] || die "flock transfer for $who answered $status"
}

# create_nidd <who> <flock> <imei>
create_nidd() {
  api "$1" POST /flock/pigeons \
    "{\"flock_id\":\"$2\",\"name\":\"NIDD synthetic\",\"connector\":{\"Nidd\":{\"imei\":\"$3\"}}}"
  note "  $(jq -c 'if type == "object" and has("pigeon") then {id: .pigeon.id,
    endpoint: .pigeon.connector.Nidd.endpoint, imei: .pigeon.connector.Nidd.imei,
    token_set: (.pigeon.connector.Nidd.token != ""),
    claim_key_len: (.pigeon.connector.Nidd.claim_key // "" | length)} else . end' "$resp" \
    2>/dev/null || head -c 300 "$resp")"
}

claim_key_of() { jq -r '.pigeon.connector.Nidd.claim_key // .connector.Nidd.claim_key' "$resp"; }

# ======================================================================================
# Phase 1: the callback password blanked. Setup, and the 503 of step 1.
# ======================================================================================

step 0 "setup: two dev Kratos identities, their organizations and flocks"
start_wrangler pw-blank --var THINGSPACE_CALLBACK_PASSWORD:
register a
register b
org_with_flock a
org_a=$org_id
flock_a=$flock_id
org_with_flock b
org_b=$org_id
flock_b=$flock_id
api a POST /flocks '{"name":"NIDD synthetic personal flock"}'
flock_p=$(jq -r .id "$resp")
note "  org A $org_a flock $flock_a; org B $org_b flock $flock_b; personal flock $flock_p"
note "  IMEIs: A $imei_a, Q $imei_q, X $imei_x, as a JSON number $imei_num"
pass "setup complete"

step 1 "callback gates"
m=$(mark)
r=$(rid s1c)
report_body niddConfigResponse ConfigCreated "" "$imei_a" "$iccid1" "$r"
callback "(password removed)"
expect "password removed answers 503" 503 "$status"
note "  $(head -c 200 "$resp")"
expect_log "password removed logs not_configured with the request id and attempt" "$m" \
  "nidd_cb outcome=not_configured request=$r attempt=1"
note_log "$m"
stop_wrangler

# ======================================================================================
# Phase 2: the callback allowlist and the account name blanked.
# ======================================================================================

start_wrangler closed --var THINGSPACE_CALLBACK_ALLOWED_IPS: --var THINGSPACE_ACCOUNT_NAME: \
  --var "NIDD_ALLOWED_ORG_IDS:$org_a"

step 1 "callback gates (allowlist emptied)"
m=$(mark)
report_body niddConfigResponse ConfigCreated "" "$imei_a" "$iccid1" "$(rid s1b)"
callback "(allowlist emptied)"
expect "allowlist emptied answers 403" 403 "$status"
expect_log "the refused address is logged" "$m" 'ThingSpace callback from disallowed address'
note_log "$m"

step 2 "Nidd create (account name unset)"
m=$(mark)
create_nidd a "$flock_a" "$imei_x"
expect "create with THINGSPACE_ACCOUNT_NAME unset answers 403" 403 "$status"
expect "and says NIDD is off here" "Forbidden: NIDD is not enabled in this environment" \
  "$(cat "$resp")"
note_log "$m"
stop_wrangler

# ======================================================================================
# Phase 3: fully configured, organization A allowlisted.
# ======================================================================================

# A rotation's grace window, with a fixture value that is no credential anywhere.
previous_password="nidd-synthetic-previous-password"
start_wrangler main --var "NIDD_ALLOWED_ORG_IDS:$org_a" \
  --var "THINGSPACE_CALLBACK_PASSWORD_PREVIOUS:$previous_password"

step 1 "callback gates"
m=$(mark)
NIDD_ACCT=$account jq -cn '{username: "pidgeiot", password: "wrong-password", requestId: "s1a",
  niddResponse: {niddConfigResponse: {accountName: $ENV.NIDD_ACCT}}}' >"$work/body"
callback "(wrong password)"
expect "wrong password answers 403" 403 "$status"
NIDD_PW=$previous_password NIDD_ACCT=$account jq -cn '{username: "pidgeiot",
  password: $ENV.NIDD_PW, requestId: "s1p", callbackCount: 1,
  niddResponse: {niddConfigResponse: {accountName: $ENV.NIDD_ACCT}}}' >"$work/body"
callback "(the previous password, inside its grace window)"
expect "the previous password answers 200" 200 "$status"
expect_log "and is logged as the previous one" "$m" \
  'nidd_cb password=previous request=s1p attempt=1'
{
  printf '{"pad":"'
  head -c $((8193 - 10)) /dev/zero | tr '\0' x
  printf '"}'
} >"$work/body"
callback "(8193-byte body)"
expect "an 8193-byte body answers 413" 413 "$status"
printf 'not json' >"$work/body"
callback "(non-JSON body)"
expect "a non-JSON body answers 400" 400 "$status"
printf '{"requestId":"s1e"}' >"$work/body"
callback "(no password)"
expect "a body without a password answers 400" 400 "$status"
expect_log "the wrong password is logged by outcome, request id and attempt only" "$m" \
  'nidd_cb outcome=wrong_password request=s1a attempt=none'
note_log "$m"

step 2 "Nidd create"
m=$(mark)
create_nidd a "$flock_a" "$imei_a"
expect "create in allowlisted org A answers 201" 201 "$status"
pigeon=$(jq -r .pigeon.id "$resp")
claim_key=$(claim_key_of)
claim_keys+=("$claim_key")
expect "the claim key is 32 lowercase hex characters" yes \
  "$([[ $claim_key =~ ^[0-9a-f]{32}$ ]] && echo yes || echo no)"
expect "the endpoint is minted" "nidd://VZWSCEF" "$(jq -r .pigeon.connector.Nidd.endpoint "$resp")"
expect "the IMEI is kept" "$imei_a" "$(jq -r .pigeon.connector.Nidd.imei "$resp")"
create_nidd a "$flock_a" "$imei_a"
expect "the same IMEI again answers 409" 409 "$status"
note "  $(cat "$resp")"
create_nidd a "$flock_a" "$imei_bad"
expect "a bad check digit answers 400" 400 "$status"
create_nidd b "$flock_b" "$imei_a"
expect "org B, outside the allowlist, answers 403 for that IMEI" 403 "$status"
expect "and names the organization" "Forbidden: NIDD is not enabled for this organization" \
  "$(cat "$resp")"
create_nidd a "$flock_p" "$imei_a"
expect "a personal flock answers 403 for that IMEI" 403 "$status"
expect "and names the organization" "Forbidden: NIDD is not enabled for this organization" \
  "$(cat "$resp")"
mirror=$(sql "SELECT connector->'Nidd'->>'imei' = '$imei_a',
  connector->'Nidd'->>'claim_key' IS NULL, connector->'Nidd'->>'token' = ''
  FROM pigeons WHERE id = '$pigeon';")
expect "the Postgres mirror holds the IMEI, no claim key and no token" "t|t|t" "$mirror"
note_log "$m"

step 3 "the pigeon through the existing routes"
m=$(mark)
api a GET "/pigeons/$pigeon"
expect "GET /pigeons/:id answers 200" 200 "$status"
note "  $(jq -c '{id, connector: {Nidd: {endpoint: .connector.Nidd.endpoint,
  imei: .connector.Nidd.imei, token: .connector.Nidd.token,
  claim_key: .connector.Nidd.claim_key}}}' "$resp")"
expect "the name-derived id survives get_pigeon_do!" "$pigeon" "$(jq -r .id "$resp")"
expect "the read carries no secrets" '""|null' \
  "$(jq -r '"\(.connector.Nidd.token | tojson)|\(.connector.Nidd.claim_key)"' "$resp")"
api a GET "/pigeons/$pigeon/detail"
expect "GET /pigeons/:id/detail answers 200" 200 "$status"
expect "the detail carries no claim key" null "$(jq -r .pigeon.connector.Nidd.claim_key "$resp")"
note_log "$m"

step 4 "telemetry before HELLO"
m=$(mark)
r=$(rid s4)
frame 01 '{"temp_c":"20.5"}'
uplink "TELEMETRY before HELLO" "$r" 1 "$imei_a" "$iccid1"
expect "answers 200" 200 "$status"
expect_log "the uplink is dropped as unclaimed" "$m" \
  "nidd_cb kind=uplink outcome=unclaimed pigeon=$pigeon request=$r"
expect_log "an UNCLAIMED 0 notice is planned, and its send says not_configured" "$m" \
  "nidd_dl kind=status outcome=unavailable reason=not_configured pigeon=$pigeon"
api a GET "/pigeons/$pigeon/telemetry"
expect "nothing is stored" 0 "$(jq length "$resp")"
state=$(nidd_state "$pigeon" "claimed_at IS NULL, notice_at > 0")
expect "pigeon_nidd: unclaimed, a notice recorded" "1|1" "$state"
note_log "$m"

step 5 "HELLO, the claim and the line pin"
m=$(mark)
r=$(rid s5a)
hello "$junk_key"
uplink "HELLO with a wrong key" "$r" 1 "$imei_a" "$iccid1"
expect_log "a wrong key leaves the pigeon unclaimed" "$m" \
  "outcome=unclaimed pigeon=$pigeon request=$r"
expect_no_log "no second notice inside the hour after step 4's" "$m" "nidd_dl .*pigeon=$pigeon"
note_log "$m"

m=$(mark)
create_nidd a "$flock_a" "$imei_q"
expect "a second pigeon Q for a wrong-key HELLO answers 201" 201 "$status"
pigeon_q=$(jq -r .pigeon.id "$resp")
r=$(rid s5b)
hello "$junk_key"
uplink "HELLO with a wrong key to Q" "$r" 1 "$imei_q" "$iccid1"
expect_log "Q stays unclaimed" "$m" "outcome=unclaimed pigeon=$pigeon_q request=$r"
expect_log "UNCLAIMED 1 is planned for Q" "$m" \
  "nidd_dl kind=status outcome=unavailable reason=not_configured pigeon=$pigeon_q"
expect "Q: unclaimed, a notice recorded" "1|1" \
  "$(nidd_state "$pigeon_q" "claimed_at IS NULL, notice_at > 0")"
note_log "$m"

m=$(mark)
r=$(rid s5c)
hello "$claim_key"
uplink "HELLO with the right key from ICCID1" "$r" 1 "$imei_a" "$iccid1"
expect "answers 200" 200 "$status"
expect_log "the pigeon is claimed" "$m" "outcome=claimed pigeon=$pigeon request=$r"
expect_log "a SHADOW is planned" "$m" \
  "nidd_dl kind=shadow outcome=unavailable reason=not_configured pigeon=$pigeon"
expect "claimed and pinned to ICCID1" "1|1" \
  "$(nidd_state "$pigeon" "claimed_at IS NOT NULL, line_id = '$iccid1'")"
note_log "$m"

m=$(mark)
r=$(rid s5d)
frame 01 '{"temp_c":"21.0"}'
uplink "TELEMETRY naming ICCID2" "$r" 1 "$imei_a" "$iccid2"
expect_log "another line's frame is dropped as unclaimed" "$m" \
  "outcome=unclaimed pigeon=$pigeon request=$r"
expect "the claim and its pin are kept" "1|1" \
  "$(nidd_state "$pigeon" "claimed_at IS NOT NULL, line_id = '$iccid1'")"
r=$(rid s5e)
frame 01 '{"temp_c":"21.5"}'
uplink "TELEMETRY from ICCID1" "$r" 1 "$imei_a" "$iccid1"
expect_log "the pinned line's frame is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
api a GET "/pigeons/$pigeon/telemetry"
expect "temp_c is the pinned line's value" 21.5 \
  "$(jq -r '.[] | select(.key == "temp_c") | .value' "$resp")"
r=$(rid s5f)
hello "$claim_key"
uplink "HELLO with the right key from ICCID2" "$r" 1 "$imei_a" "$iccid2"
expect_log "a good HELLO from ICCID2 claims again" "$m" "outcome=claimed pigeon=$pigeon request=$r"
expect "the pin moved to ICCID2" 1 "$(nidd_state "$pigeon" "line_id = '$iccid2'")"
r=$(rid s5g)
frame 01 '{"temp_c":"22.0"}'
uplink "TELEMETRY from ICCID1 after the move" "$r" 1 "$imei_a" "$iccid1"
expect_log "the old line is now dropped" "$m" "outcome=unclaimed pigeon=$pigeon request=$r"
r=$(rid s5h)
frame 01 '{"temp_c":"22.5"}'
uplink "TELEMETRY from ICCID2 after the move" "$r" 1 "$imei_a" "$iccid2"
expect_log "the new line is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
note_log "$m"

step 6 "flat, batched and retried telemetry"
m=$(mark)
r=$(rid s6a)
frame 01 '{"batt_mv":"3712","rsrp":"-97"}'
uplink "flat TELEMETRY" "$r" 1 "$imei_a" "$iccid2"
expect_log "the flat report is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
api a GET "/pigeons/$pigeon/telemetry"
expect "the dashboard shows batt_mv and rsrp" "3712|-97" \
  "$(jq -r '[(.[] | select(.key == "batt_mv") | .value),
    (.[] | select(.key == "rsrp") | .value)] | join("|")' "$resp")"
r=$(rid s6b)
sent=$(date +%s)
frame 01 "$(printf '%s' '{"reports":[{"age_secs":600,"metrics":{"uptime_s":"85800","rsrp":"-97",' \
  '"batt_mv":"3712","temp_c":"21.5"}},{"age_secs":300,"metrics":{"uptime_s":"86100",' \
  '"rsrp":"-98","batt_mv":"3711","temp_c":"21.4"}},{"age_secs":0,"metrics":{"uptime_s":' \
  '"86400","rsrp":"-97","batt_mv":"3711","temp_c":"21.4"}}]}')"
expect "the batch is the design's 294-byte frame 1" 294 "$(wc -c <"$work/frame")"
uplink "batched TELEMETRY (frame 1)" "$r" 1 "$imei_a" "$iccid2"
expect_log "the batch is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
api a GET "/pigeons/$pigeon/telemetry/history?raw=true&keys=uptime_s&since=$(iso_ago 3600)"
note "  $(jq -c '[.[] | {value, reported_at}]' "$resp")"
ages=$(jq -r '.[] | "\(.value) \(.reported_at)"' "$resp" | while read -r v t; do
  printf '%s:%s ' "$v" $(((sent - $(epoch "$t") + 30) / 60 * 60))
done)
expect "three history rows, 600 s, 300 s and 0 s old (to the minute)" \
  "85800:600 86100:300 86400:0 " "$ages"
# ThingSpace's only retry follows a refusal by about a second, so a later attempt is not aged.
r=$(rid s6c)
sent=$(date +%s)
frame 01 '{"hum_pct":"41"}'
uplink "flat TELEMETRY on callbackCount 2" "$r" 2 "$imei_a" "$iccid2"
expect_log "the retried report is stored" "$m" "outcome=stored pigeon=$pigeon request=$r attempt=2"
api a GET "/pigeons/$pigeon/telemetry/history?raw=true&keys=hum_pct&since=$(iso_ago 3600)"
note "  $(jq -c '[.[] | {value, reported_at}]' "$resp")"
age=$((sent - $(epoch "$(jq -r '.[0].reported_at' "$resp")")))
expect "stored at its arrival, not backdated (within 30 s)" yes \
  "$( ((age >= -30 && age <= 30)) && echo yes || echo "no ($age s)")"
note_log "$m"

step 7 "a resend of a stored uplink, and a retry that overlaps its first attempt"
m=$(mark)
r=$(rid s7)
frame 01 '{"steps":"7"}'
uplink "TELEMETRY" "$r" 1 "$imei_a" "$iccid2"
uplink "the same body and request id again" "$r" 1 "$imei_a" "$iccid2"
expect "the repeat answers 200" 200 "$status"
expect_log "the first is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
expect_log "the second is recognised as a duplicate" "$m" \
  "outcome=duplicate pigeon=$pigeon request=$r"
api a GET "/pigeons/$pigeon/telemetry/history?raw=true&keys=steps&since=$(iso_ago 3600)"
expect "one history row, one write" 1 "$(jq length "$resp")"
note "  Billing: dev binds no telemetry queue and bills telemetry on no surface; in a deployed"
note "  environment the consumer bills per enqueued reading, and a duplicate is never enqueued."
note_log "$m"

# ThingSpace also retries a callback it has had no answer to for about 4 s, while the first
# attempt is still running. The lock holds the first attempt inside its history write, dev's
# stand-in for the enqueue, while the retry arrives.
m=$(mark)
r=$(rid s7o)
frame 01 '{"laps":"1"}'
uplink_body "$r" 1 "$imei_a" "$iccid2" >"$work/body-first"
uplink_body "$r" 2 "$imei_a" "$iccid2" >"$work/body-retry"
hold_history
note "> POST /internal/thingspace/nidd the first attempt (request $r, attempt 1), not awaited"
post_async "$work/body-first" "$work/status-first"
first=$!
expect "the first attempt is held inside its history write" yes \
  "$(insert_held && echo yes || echo no)"
note "> POST /internal/thingspace/nidd the retry (request $r, attempt 2), not awaited"
post_async "$work/body-retry" "$work/status-retry"
retry=$!
expect_log "the retry answers duplicate while the first is held" "$m" \
  "outcome=duplicate pigeon=$pigeon request=$r attempt=2"
expect "the first is still held then, and not yet stored" "yes 0" \
  "$(insert_held && echo yes || echo no) $(log_since "$m" |
    grep -c "outcome=stored pigeon=$pigeon request=$r" || true)"
release_history
wait "$first" "$retry" || true
expect "both attempts answer 200" "200 200" \
  "$(cat "$work/status-first") $(cat "$work/status-retry")"
expect_log "released, the first attempt is stored" "$m" \
  "outcome=stored pigeon=$pigeon request=$r attempt=1"
expect "one stored, one duplicate" "1 1" \
  "$(log_since "$m" | grep -c "outcome=stored pigeon=$pigeon request=$r" || true) $(log_since "$m" |
    grep -c "outcome=duplicate pigeon=$pigeon request=$r" || true)"
api a GET "/pigeons/$pigeon/telemetry/history?raw=true&keys=laps&since=$(iso_ago 3600)"
expect "one history row" 1 "$(jq length "$resp")"
note "  Billing: the one history row stands for the one enqueue a deployed environment bills."
note_log "$m"

step 8 "shadow reports"
m=$(mark)
api a PUT "/pigeons/$pigeon/shadow" '{"target_config":{"telemetry_interval":900,"log":false}}'
expect "a dashboard write sets target_version 1" "200 1" "$status $(jq -r .target_version "$resp")"
expect_log "and plans a SHADOW" "$m" "nidd_dl kind=shadow .*pigeon=$pigeon"
billed=$(usage_of "$org_a")
note "  org A billed messages before: $billed"

m=$(mark)
r=$(rid s8a)
frame 02 '{"current_config":{"telemetry_interval":300},"current_version":0}'
uplink "SHADOW_REPORT behind" "$r" 1 "$imei_a" "$iccid2"
expect_log "the behind report is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
expect_log "and answered with a SHADOW" "$m" "nidd_dl kind=shadow .*pigeon=$pigeon"
expect "one billable message" $((billed + 1)) "$(wait_usage "$org_a" $((billed + 1)))"
note_log "$m"

m=$(mark)
r=$(rid s8b)
frame 02 '{"current_config":{"telemetry_interval":900,"log":false},"current_version":1}'
uplink "SHADOW_REPORT converged" "$r" 1 "$imei_a" "$iccid2"
expect_log "the converged report is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
expect_log "and answered with STATUS STORED" "$m" "nidd_dl kind=status .*pigeon=$pigeon"
expect "one more billable message" $((billed + 2)) "$(wait_usage "$org_a" $((billed + 2)))"
note_log "$m"

m=$(mark)
r=$(rid s8c)
uplink "the same report again, a new request id" "$r" 1 "$imei_a" "$iccid2"
expect_log "the repeat is recognised" "$m" "outcome=repeat pigeon=$pigeon request=$r"
expect_log "and answered with STATUS STORED" "$m" "nidd_dl kind=status .*pigeon=$pigeon"
sleep 2
expect "and not billed" $((billed + 2)) "$(usage_of "$org_a")"
api a GET "/pigeons/$pigeon/shadow"
expect "the shadow is converged at 1" "1 1" \
  "$(jq -r '"\(.target_version) \(.current_version)"' "$resp")"
expect "nothing is owed" 0 "$(nidd_state "$pigeon" awaiting_version)"
note_log "$m"

m=$(mark)
r=$(rid s8d)
frame 01 '{"temp_c":"20.0"}'
uplink "TELEMETRY from the converged device" "$r" 1 "$imei_a" "$iccid2"
expect_log "the telemetry is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
expect_no_log "and, nothing being owed, draws no reply" "$m" "nidd_dl .*pigeon=$pigeon"
note_log "$m"

step 9 "dashboard writes"
m=$(mark)
api a PUT "/pigeons/$pigeon/shadow" '{"target_config":{"telemetry_interval":600}}'
expect "a dashboard write answers 200 at target_version 2" "200 2" \
  "$status $(jq -r .target_version "$resp")"
expect_log "and plans a SHADOW" "$m" \
  "nidd_dl kind=shadow outcome=unavailable reason=not_configured pigeon=$pigeon"
sleep 1
# Dev cannot send, so the push is marked unsent (design 6.4), which is what makes it due again.
expect "the unsent push is marked for the next trigger" "2|0|0" \
  "$(nidd_state "$pigeon" "awaiting_version, pushed_version, pushed_at")"
note_log "$m"

m=$(mark)
api a PUT "/pigeons/$pigeon/shadow" '{"target_config":{"telemetry_interval":601}}'
expect "a second write inside 900 s answers 200 at target_version 3" "200 3" \
  "$status $(jq -r .target_version "$resp")"
expect_log "and, the first push never having left, plans a SHADOW again" "$m" \
  "nidd_dl kind=shadow .*pigeon=$pigeon"
note "  In an environment that can send, the first push stays recorded and this write is held"
note "  for the next uplink to carry; shadow_push_and_reply_truth_table covers the hold."
note_log "$m"

m=$(mark)
pad() { head -c "$1" /dev/zero | tr '\0' x; }
# The write creates version 4, so the SHADOW header is "4 4" and a newline at its longest before
# the next write: 1358 less the type byte, those 4 bytes and the 16-character tag leaves 1337.
api a PUT "/pigeons/$pigeon/shadow" "{\"target_config\":{\"pad\":\"$(pad 1328)\"}}"
expect "a 1338-byte target_config answers 413" 413 "$status"
expect "naming this pigeon's cap" \
  "Payload Too Large: this NIDD pigeon's target_config must serialize to at most 1337 bytes" \
  "$(cat "$resp")"
api a GET "/pigeons/$pigeon/shadow"
expect "and changes nothing" 3 "$(jq -r .target_version "$resp")"
api a PUT "/pigeons/$pigeon/shadow" "{\"target_config\":{\"pad\":\"$(pad 1327)\"}}"
expect "a 1337-byte target_config is accepted" "200 4" "$status $(jq -r .target_version "$resp")"
expect_log "and plans its SHADOW" "$m" "nidd_dl kind=shadow .*pigeon=$pigeon"
note_log "$m"

# A delivery report that the push missed marks it pending, and the device's next uplink, which
# finds it connected, carries it as the reply.
m=$(mark)
r=$(rid s9r)
report_body niddMTDeliveryResponse DeliveryFailed "Backend service error" "$imei_a" "$iccid2" "$r"
callback "(DeliveryFailed while version 4 is owed)"
expect "a missed delivery answers 200" 200 "$status"
expect_log "and marks the owed push pending" "$m" \
  "nidd_cb kind=delivery outcome=pending status=DeliveryFailed .*pigeon=$pigeon request=$r"
expect "pigeon_nidd: version 4 owed, its push pending" "4|0" \
  "$(nidd_state "$pigeon" "awaiting_version, pushed_version")"
r=$(rid s9t)
frame 01 '{"temp_c":"20.5"}'
uplink "TELEMETRY while version 4 is owed" "$r" 1 "$imei_a" "$iccid2"
expect_log "the telemetry is stored" "$m" "outcome=stored pigeon=$pigeon request=$r"
expect_log "and its reply is the owed SHADOW" "$m" \
  "nidd_dl kind=shadow outcome=unavailable reason=not_configured pigeon=$pigeon"
note_log "$m"

step 10 "token refresh"
m=$(mark)
api a POST "/pigeons/$pigeon/token/refresh"
expect "refresh answers 200" 200 "$status"
new_key=$(claim_key_of)
claim_keys+=("$new_key")
expect "a new claim key" yes \
  "$([[ $new_key =~ ^[0-9a-f]{32}$ && $new_key != "$claim_key" ]] && echo yes || echo no)"
expect "the IMEI and endpoint are kept" "$imei_a nidd://VZWSCEF" \
  "$(jq -r '"\(.connector.Nidd.imei) \(.connector.Nidd.endpoint)"' "$resp")"
expect "pigeon_nidd: unclaimed and unpinned" "1|1" \
  "$(nidd_state "$pigeon" "claimed_at IS NULL, line_id IS NULL")"
r=$(rid s10a)
frame 01 '{"temp_c":"23.0"}'
uplink "TELEMETRY after the refresh" "$r" 1 "$imei_a" "$iccid2"
expect_log "the next telemetry is refused as unclaimed" "$m" \
  "outcome=unclaimed pigeon=$pigeon request=$r"
r=$(rid s10b)
hello "$claim_key"
uplink "HELLO with the old key" "$r" 1 "$imei_a" "$iccid2"
expect_log "the old key no longer claims" "$m" "outcome=unclaimed pigeon=$pigeon request=$r"
claim_key=$new_key
note_log "$m"

step 11 "delete, then recreate with the same IMEI"
m=$(mark)
printf '{"build_id":"nidd-synthetic","version":3}' >"$work/dict"
http "a PUT /pigeons/:id/log-dictionary" -b "$work/jar-a" -X PUT --data-binary @"$work/dict" \
  "$base/pigeons/$pigeon/log-dictionary"
expect "a dictionary is uploaded" 200 "$status"
note "  history rows before the delete: $(sql "SELECT count(*) FROM pigeon_telemetry_history
  WHERE pigeon_id = '$pigeon';")"
api a DELETE "/pigeons/$pigeon"
expect "delete answers 200" 200 "$status"
# Leftovers as a failed best-effort cleanup would leave them: the mirror row with its history,
# and the dictionary.
sql "INSERT INTO pigeons (id, flock_id, name, connector, created_at, updated_at)
  VALUES ('$pigeon', '$flock_a', 'leftover', '{}', now(), now());
  INSERT INTO pigeon_telemetry_history (pigeon_id, key, value, value_num)
  VALUES ('$pigeon', 'leftover', '1', 1);" >/dev/null
# The local explorer API wrangler dev serves, which writes the bucket the Worker reads.
leftover="$base/cdn-cgi/local/explorer/api/r2/buckets/$bucket/objects"
leftover="$leftover/log-dictionaries%2F$pigeon.json"
http "PUT leftover dictionary into local R2" -X PUT --data-binary @"$work/dict" "$leftover"
http "GET it back from local R2" "$leftover"
expect "leftovers injected: a mirror row, a history row and a dictionary" "1|1|200" \
  "$(sql "SELECT (SELECT count(*) FROM pigeons WHERE id = '$pigeon'),
    (SELECT count(*) FROM pigeon_telemetry_history WHERE pigeon_id = '$pigeon');")|$status"
sleep 1
create_nidd a "$flock_a" "$imei_a"
expect "recreate answers 201" 201 "$status"
expect "under the same, name-derived id" "$pigeon" "$(jq -r .pigeon.id "$resp")"
claim_key=$(claim_key_of)
claim_keys+=("$claim_key")
expect "the old history is gone before the first reading" "0|1" \
  "$(sql "SELECT (SELECT count(*) FROM pigeon_telemetry_history WHERE pigeon_id = '$pigeon'),
    (SELECT count(*) FROM pigeons WHERE id = '$pigeon' AND name <> 'leftover');")"
api a GET "/pigeons/$pigeon/log-dictionary"
expect "the dictionary uploaded before the recreate answers 404" 404 "$status"
api a GET "/pigeons/$pigeon/telemetry"
expect "the object holds no telemetry" 0 "$(jq length "$resp")"
sleep 1
http "a PUT /pigeons/:id/log-dictionary (after the recreate)" -b "$work/jar-a" -X PUT \
  --data-binary @"$work/dict" "$base/pigeons/$pigeon/log-dictionary"
api a GET "/pigeons/$pigeon/log-dictionary"
expect "a dictionary uploaded after it is served" 200 "$status"
note_log "$m"

step 12 "delivery reports and configuration results"
for report in "niddMTDeliveryResponse Delivered" "niddMTDeliveryResponse Queued" \
  "niddMTDeliveryResponse DeliveryFailed Device_Not_Reachable" \
  "niddConfigResponse ConfigCreated" "niddConfigResponse ConfigFailed Plan_Not_Supported"; do
  read -r variant st reason <<<"$report"
  m=$(mark)
  r=$(rid s12)
  report_body "$variant" "$st" "${reason//_/ }" "$imei_a" "$iccid2" "$r"
  callback "($variant $st)"
  expect "$variant $st answers 200" 200 "$status"
  wait_log "$m" "request=$r" || true
  sleep 1
  expect "$variant $st logs one line" 1 "$(log_since "$m" | grep -c "request=$r" || true)"
  expect "$variant $st logs neither the IMEI nor the ICCID" 0 \
    "$(log_since "$m" | grep -cE "$imei_a|$iccid2" || true)"
  note_log "$m"
done
m=$(mark)
NIDD_PW=$cb_password NIDD_ACCT=$account jq -cn --argjson imei "$imei_num" '
  {username: "pidgeiot", password: $ENV.NIDD_PW, requestId: "s12-number",
   deviceIds: [{id: $imei, kind: "IMEI"}],
   niddResponse: {niddMTDeliveryResponse: {accountName: $ENV.NIDD_ACCT,
     deviceIds: [{id: $imei, kind: "IMEI"}]}}, status: "Delivered"}' >"$work/body"
callback "(an IMEI as a JSON number)"
expect "a body with an IMEI as a JSON number answers 200" 200 "$status"
expect_log "it is logged as an unknown shape" "$m" \
  "nidd_cb outcome=unknown_shape request=s12-number"
leaked=0
for ((i = 0; i + 5 <= ${#imei_num}; i++)); do
  if log_since "$m" | grep -q "${imei_num:i:5}"; then leaked=$((leaked + 1)); fi
done
expect "the log holds no five digits of that IMEI in a row" 0 "$leaked"
note_log "$m"

step 13 "an account over its free-tier allowance"
m=$(mark)
r=$(rid s13a)
hello "$claim_key"
uplink "HELLO with the recreated pigeon's key" "$r" 1 "$imei_a" "$iccid2"
expect_log "the recreated pigeon is claimed" "$m" "outcome=claimed pigeon=$pigeon request=$r"
period="date_trunc('month', now())"
before=$(sql "SELECT billable_messages FROM billing_usage_periods
  WHERE owner_kind = 'org' AND owner_id = '$org_a' AND period_start = $period;")
if [[ -n $before ]]; then
  usage_restore="UPDATE billing_usage_periods SET billable_messages = $before
    WHERE owner_kind = 'org' AND owner_id = '$org_a' AND period_start = $period;"
else
  usage_restore="DELETE FROM billing_usage_periods
    WHERE owner_kind = 'org' AND owner_id = '$org_a' AND period_start = $period;"
fi
sql "INSERT INTO billing_usage_periods (owner_kind, owner_id, period_start, period_end,
  billable_messages) VALUES ('org', '$org_a', $period, $period + interval '1 month', 300000)
  ON CONFLICT (owner_kind, owner_id, period_start) DO UPDATE SET billable_messages = 300000;" \
  >/dev/null
note "  org A forced to 300000 billable messages this month (the free tier's allowance)"
note_log "$m"

m=$(mark)
r=$(rid s13b)
frame 01 '{"temp_c":"24.0"}'
uplink "TELEMETRY while paused" "$r" 1 "$imei_a" "$iccid2"
expect "answers 200" 200 "$status"
expect_log "the uplink is dropped as paused" "$m" "outcome=paused pigeon=$pigeon request=$r"
expect_log "a PAUSED notice is planned" "$m" "nidd_dl kind=status .*pigeon=$pigeon"
notice=$(nidd_state "$pigeon" notice_at)
note_log "$m"

m=$(mark)
r=$(rid s13c)
frame 01 '{"temp_c":"24.5"}'
uplink "TELEMETRY while paused, again" "$r" 1 "$imei_a" "$iccid2"
expect_log "the second is dropped as paused too" "$m" "outcome=paused pigeon=$pigeon request=$r"
r=$(rid s13d)
frame 02 '{"current_config":{"telemetry_interval":900},"current_version":0}'
uplink "SHADOW_REPORT while paused" "$r" 1 "$imei_a" "$iccid2"
expect_log "a shadow report is paused as well" "$m" "outcome=paused pigeon=$pigeon request=$r"
expect_no_log "no second PAUSED within the hour" "$m" "nidd_dl .*pigeon=$pigeon"
expect "notice_at unchanged" "$notice" "$(nidd_state "$pigeon" notice_at)"
api a GET "/pigeons/$pigeon/telemetry"
expect "nothing was stored while paused" 0 \
  "$(jq '[.[] | select(.key == "temp_c")] | length' "$resp")"
sql "$usage_restore" >/dev/null
usage_restore=""
r=$(rid s13e)
frame 01 '{"temp_c":"25.0"}'
uplink "TELEMETRY after the allowance is restored" "$r" 1 "$imei_a" "$iccid2"
expect_log "ingest resumes" "$m" "outcome=stored pigeon=$pigeon request=$r"
note_log "$m"
stop_wrangler

# ======================================================================================
# Phase 4: every telemetry store failing, as a deployed environment without its queue binding
# does; a non-loopback DEVICE_API_HOST is what makes dev count as deployed. Then phase 3's
# configuration again, for ThingSpace's retry.
# ======================================================================================

start_wrangler failing --var "NIDD_ALLOWED_ORG_IDS:$org_a" --var DEVICE_API_HOST:api-dev.invalid

step 7 "a store that fails releases its claim, so ThingSpace's retry can store the uplink"
m=$(mark)
r=$(rid s7f)
frame 01 '{"splits":"1"}'
uplink "TELEMETRY while the telemetry store fails" "$r" 1 "$imei_a" "$iccid2"
expect "the failed store answers 503" 503 "$status"
expect_log "and is logged as lost" "$m" "nidd_cb lost pigeon=$pigeon request=$r attempt=1"
expect "pigeon_nidd keeps no key for it" 0 "$(nidd_state "$pigeon" "instr(seen, '$r:') > 0")"
note_log "$m"
stop_wrangler

start_wrangler restored --var "NIDD_ALLOWED_ORG_IDS:$org_a"
m=$(mark)
uplink "the retry, the store working again" "$r" 2 "$imei_a" "$iccid2"
expect_log "the retry is stored, not taken for a duplicate" "$m" \
  "outcome=stored pigeon=$pigeon request=$r attempt=2"
api a GET "/pigeons/$pigeon/telemetry/history?raw=true&keys=splits&since=$(iso_ago 3600)"
expect "one history row" 1 "$(jq length "$resp")"
note_log "$m"

step 14 "after the thirteen: what the logs never hold"
leaks=$(cat "$ev"/wrangler-*.log | grep -cFf <(printf '%s\n' "$cb_password" "$account" \
  "${claim_keys[@]}" "$imei_a" "$imei_q" "$iccid1" "$iccid2" | awk 'length >= 8') || true)
expect "no password, account name, claim key, IMEI or ICCID in any wrangler log" 0 "$leaks"
stop_wrangler

printf '\n%s passed, %s failed; evidence in %s\n' "$passes" "$fails" "$ev" |
  tee -a "$ev/summary.txt"
((fails == 0))
