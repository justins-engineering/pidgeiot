#!/usr/bin/env bash
# init-greptime.sh
#
# Idempotent init/retention script for GreptimeDB (task #34) -- the
# counterpart to init-db.sql, but deliberately NOT a schema mirror.
# `pigeon_telemetry` is a GreptimeDB **auto-schema wide table**: it is
# auto-created, and every new metric key auto-adds its own column, on first
# InfluxDB line-protocol write (see dovecote/src/helpers/greptime.rs's
# `build_line_protocol`/`query_greptime_sql` doc comments, confirmed
# empirically against a real instance while that code was built). There is
# no per-metric-column DDL to pre-declare here, and this script does not try.
#
# What this DOES codify (the two things that don't happen automatically):
#
#   1. CREATE DATABASE IF NOT EXISTS for non-`public` databases. GreptimeDB's
#      InfluxDB line-protocol write does NOT auto-create a missing database
#      (it 400s "Failed to find schema") -- only `public` auto-exists. Prod
#      uses `public` (GREPTIMEDB_DB unset, see dovecote/wrangler.toml
#      [vars]); staging uses `staging` (GREPTIMEDB_DB="staging",
#      [env.staging.vars]) on the SAME shared self-hosted instance, and that
#      db has so far been created by hand. This script makes that step
#      idempotent/scripted instead of a one-off manual command.
#
#   2. RETENTION / TTL -- the actual point of this file. Telemetry today is
#      unbounded time-series with NO retention policy anywhere in the stack
#      (Postgres's own pigeon_telemetry_history mirror has no expiry either).
#      GreptimeDB supports both database-level TTL (every table in the db
#      inherits it unless a table sets its own) and per-table TTL. This
#      script sets a database-level default on both `public` and `staging`,
#      AND pins the same TTL explicitly on a pre-created `pigeon_telemetry`
#      skeleton table in each db, so retention is in effect from row zero
#      even if a future per-database change ever drifts the two apart.
#
# TTL WINDOW -- DECISION FLAGGED FOR THE LEAD, NOT UNILATERALLY CHOSEN HERE:
#   Recommending 90 DAYS as the starting default. Rationale:
#     - fancier's telemetry graph UI (`fancier/src/components/graph_widget.rs`,
#       `TimeRange` enum) tops out at a 30-day preset today -- no 90d/1y/"all
#       time" option exists yet. 90d gives 3x headroom above the longest
#       range a dashboard user can actually select right now.
#     - This is the FIRST retention policy introduced anywhere in the stack
#       (unbounded growth today), so a conservative multiple of real UI usage
#       beats guessing a much longer window nobody has asked for yet.
#   Override with GREPTIME_TTL=45d ./init-greptime.sh (or any other value)
#   if the lead picks differently -- nothing below is hardcoded past this
#   one default. Re-running this script after a TTL change is always safe
#   (see "Idempotent" below).
#
# VERIFIED SYNTAX -- checked against GreptimeDB v1.1.3 (the version pinned in
# both docker-compose.yml's `greptimedb` service and
# infra/proxmox-greptimedb-lxc.sh's GREPTIME_VERSION) via
# https://docs.greptime.com/reference/sql/create/ and .../alter/:
#
#   CREATE DATABASE [IF NOT EXISTS] db_name [WITH (ttl = '90d')]
#   ALTER DATABASE db_name SET 'ttl' = '90d'          -- for a db that already
#                                                          exists (e.g. `public`,
#                                                          which auto-exists and
#                                                          so never takes the
#                                                          CREATE ... WITH(...)
#                                                          path)
#   CREATE TABLE [IF NOT EXISTS] [db.]name (
#     col type [PRIMARY KEY] [TIME INDEX], ...
#   ) WITH (ttl = '90d')
#
#   TTL value format: a duration string ('90d', '24h', '1h 12m'); 'forever' /
#   NULL / '' / '0s' mean never-expire; 'instant' means delete-on-insert
#   (tables only -- a DATABASE's own ttl cannot be 'instant').
#
#   Additionally live-verified (not just doc-checked) against this repo's own
#   docker-compose `greptimedb` service (v1.1.2 -- one patch behind the
#   v1.1.3 pinned for the real Proxmox/tunneled instance, no syntax
#   difference in CREATE/ALTER DATABASE or TTL between the two): every
#   statement below ran clean end-to-end, `SHOW CREATE DATABASE public`
#   confirmed the ttl landed (GreptimeDB normalizes '90d' to its own
#   '2months 29days 2h 52m 48s' display, same duration), TTL inheritance to
#   a pre-existing table with no ttl of its own was confirmed via `SHOW
#   CREATE TABLE`, and a second run of the whole script was a clean
#   idempotent no-op (all affectedrows: 0 except the first-ever `CREATE
#   DATABASE staging`).
#
# HOW TO RUN against the live tunneled instance (telemetry.pidgeiot.com) --
# needs the CF-Access-Client-Id/Secret service-token headers, which the lead
# holds (not available in this worktree, so this has been syntax-verified
# against GreptimeDB's docs but NOT run live):
#
#   GREPTIME_ENDPOINT=https://telemetry.pidgeiot.com \
#   GREPTIMEDB_ACCESS_CLIENT_ID=<id> \
#   GREPTIMEDB_ACCESS_CLIENT_SECRET=<secret> \
#   ./init-greptime.sh
#
#   (Add GREPTIMEDB_AUTH_TOKEN=<token> too, only if the instance has its own
#   HTTP auth configured on top of the tunnel's Access gate -- it doesn't
#   today, see dovecote/wrangler.toml's comments.)
#
# Local dev (docker-compose's `greptimedb` service -- reachable directly, no
# tunnel/Access, so only the endpoint is needed; harmless to run against
# throwaway local data):
#
#   GREPTIME_ENDPOINT=http://127.0.0.1:4000 ./init-greptime.sh
#
# Dry run -- prints the exact curl invocations instead of sending them, no
# credentials required, safe to sanity-check the script itself (this is how
# it was verified in this worktree, with no live instance reachable):
#
#   DRY_RUN=1 ./init-greptime.sh
#
# Idempotent / re-runnable: every statement is IF-NOT-EXISTS or a SET that
# just reasserts the same value -- running this repeatedly (e.g. after a
# future TTL policy change, or once more before an unrelated deploy) is
# always safe and produces the same end state.

set -euo pipefail

GREPTIME_ENDPOINT="${GREPTIME_ENDPOINT:-${GREPTIMEDB_ENDPOINT:-}}"
GREPTIME_TTL="${GREPTIME_TTL:-90d}"
DRY_RUN="${DRY_RUN:-0}"

if [ -z "$GREPTIME_ENDPOINT" ] && [ "$DRY_RUN" != "1" ]; then
  echo "error: set GREPTIME_ENDPOINT (or GREPTIMEDB_ENDPOINT), e.g. https://telemetry.pidgeiot.com" >&2
  echo "       (or DRY_RUN=1 to sanity-check this script with no endpoint/credentials)" >&2
  exit 1
fi

# Minimal percent-encoding for the 'sql' form field -- mirrors dovecote's own
# url_encode_component (dovecote/src/helpers/greptime.rs), kept independent
# here since this script has no access to that Rust code at runtime.
url_encode() {
  local s="$1" out="" c i
  for (( i = 0; i < ${#s}; i++ )); do
    c="${s:$i:1}"
    case "$c" in
      [A-Za-z0-9.~_-]) out+="$c" ;;
      *) out+="$(printf '%%%02X' "'$c")" ;;
    esac
  done
  printf '%s' "$out"
}

# Runs one SQL statement against POST {endpoint}/v1/sql -- same request
# shape as dovecote's own query_greptime_sql (form-urlencoded body, `sql=`
# plus the same optional CF-Access / bearer-token headers), so this script
# and the running Worker agree on exactly how to reach the instance.
run_sql() {
  local sql="$1"
  local body="sql=$(url_encode "$sql")"

  local -a headers=(-H "Content-Type: application/x-www-form-urlencoded")
  if [ -n "${GREPTIMEDB_AUTH_TOKEN:-}" ]; then
    headers+=(-H "Authorization: Token ${GREPTIMEDB_AUTH_TOKEN}")
  fi
  if [ -n "${GREPTIMEDB_ACCESS_CLIENT_ID:-}" ] && [ -n "${GREPTIMEDB_ACCESS_CLIENT_SECRET:-}" ]; then
    headers+=(-H "CF-Access-Client-Id: ${GREPTIMEDB_ACCESS_CLIENT_ID}")
    headers+=(-H "CF-Access-Client-Secret: ${GREPTIMEDB_ACCESS_CLIENT_SECRET}")
  fi

  if [ "$DRY_RUN" = "1" ]; then
    printf '>> %s\n' "$sql"
    printf '   curl -sS %s --data-raw %q %s\n\n' "${headers[*]}" "$body" "'${GREPTIME_ENDPOINT:-<GREPTIME_ENDPOINT>}/v1/sql'"
    return 0
  fi

  local tmp status
  tmp="$(mktemp)"
  status="$(curl -sS -o "$tmp" -w '%{http_code}' "${headers[@]}" --data "$body" "${GREPTIME_ENDPOINT}/v1/sql")"

  if [ "$status" -ge 400 ]; then
    echo "error: GreptimeDB returned HTTP $status for: $sql" >&2
    cat "$tmp" >&2
    rm -f "$tmp"
    exit 1
  fi

  echo ">> $sql"
  cat "$tmp"
  echo
  rm -f "$tmp"
}

echo "== 1. Databases =="
# staging does NOT auto-create on write (only `public` does) -- see
# dovecote/wrangler.toml's GREPTIMEDB_DB comment block. Sets its TTL inline
# at creation time; harmless no-op (including the WITH clause) on a rerun
# once it already exists.
run_sql "CREATE DATABASE IF NOT EXISTS staging WITH (ttl = '${GREPTIME_TTL}');"

echo "== 2. Retention / TTL =="
# `public` always exists (GreptimeDB's built-in default), so it never takes
# the CREATE-time WITH(...) path above -- ALTER is how its TTL gets set,
# idempotently (re-asserting the same value is a no-op).
run_sql "ALTER DATABASE public SET 'ttl' = '${GREPTIME_TTL}';"
# staging's own ALTER too, so its TTL stays correct even if a later manual
# CREATE DATABASE staging (without WITH(...)) ever preceded this script.
run_sql "ALTER DATABASE staging SET 'ttl' = '${GREPTIME_TTL}';"

echo "== 3. pigeon_telemetry skeleton (tag + time-index only; TTL from row zero) =="
# Deliberately NOT a full schema mirror: only the two columns every device
# report always has (the pigeon_id tag, and the timestamp) are declared, in
# the exact shape GreptimeDB would auto-create on first line-protocol write
# (see dovecote/src/helpers/greptime.rs's build_line_protocol -- tag
# `pigeon_id`, default timestamp column name `greptime_timestamp`). Metric
# fields (cpu_temp, battery_mv, ...) still auto-add as their own columns on
# first appearance; pre-creating the table just means TTL (and the tag/
# time-index shape) are in place before that first write ever lands, rather
# than only from whatever moment a device happens to report first.
# Table-qualified names (`db.table`) target each database explicitly, so no
# separate `db=` request parameter is needed for these two statements.
#
# `TIMESTAMP` with no explicit precision defaults to millisecond (confirmed
# live: DESC TABLE showed `TimestampMillisecond`) -- this matches, not
# fights, dovecote's own write path: `write_greptime_default`
# (helpers/greptime.rs) always writes with `precision=ms` explicitly, so a
# fresh `staging`/`public` skeleton created by this script and dovecote's
# real device writes agree on precision from row zero.
for db in public staging; do
  run_sql "CREATE TABLE IF NOT EXISTS ${db}.pigeon_telemetry (
    pigeon_id STRING PRIMARY KEY,
    greptime_timestamp TIMESTAMP TIME INDEX
  ) WITH (ttl = '${GREPTIME_TTL}');"
done

echo "Done."
