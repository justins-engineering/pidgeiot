#!/usr/bin/env bash
#
# Weekly activation metric: signup, then first device connected within 7
# days. This is the Phase 0 growth prerequisite (growth-strategy doc,
# "define and instrument the activation metric") - a cheap read against our
# own Postgres, not a new analytics project.
#
# "Signup" is every row in the Kratos identities table, counted regardless
# of current account state. State is a mutable, present-tense account
# status (for example an admin deactivation) rather than a property of the
# original signup event, so filtering on it would make an already-reported
# week's signup count change retroactively whenever someone's state changes
# later. If that ever needs revisiting, do it explicitly per week rather
# than by re-querying current state.
#
# "First device connected" is, per user, the earliest of:
#   - any of their pigeons' first pigeon_telemetry_history report, or
#   - any of their pigeons' shadow report-back (pigeon_shadow.updated_at,
#     gated on current_version > 0 or current_config being non-empty, since
#     updated_at alone also bumps on a dashboard-initiated shadow write and
#     would otherwise count a config push as a device connecting).
# A user is "activated" if that moment falls within 7 days of their signup.
#
# Caveat: a user's devices are found via flocks.user_id = identities.id.
# If Kratos identities are ever re-imported under new UUIDs (see CLAUDE.md's
# note on task #47's identity remap), a flock's user_id can stop matching
# its owner's current identity id, and that user's activation would go
# invisible to this join rather than erroring - worth a spot check after
# any future identity migration.
#
# Usage:
#   scripts/activation-metric.sh [weeks] [--csv]
#
# weeks defaults to 12. Run it weekly (Monday morning is fine) and log the
# all-time signups/activated/rate line plus the freshest week's row.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
secrets_file="$repo_root/secrets.env"

usage() {
  echo "usage: $(basename "$0") [weeks] [--csv]" >&2
  echo "  weeks   number of trailing ISO weeks to report (default 12)" >&2
  echo "  --csv   machine-readable CSV output instead of a table" >&2
}

csv=0
weeks=12
weeks_set=0
for arg in "$@"; do
  case "$arg" in
    --csv)
      csv=1
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      if [[ "$arg" =~ ^[0-9]+$ ]] && (( weeks_set == 0 )); then
        weeks="$arg"
        weeks_set=1
      else
        echo "activation-metric: unrecognized argument '$arg'" >&2
        usage
        exit 1
      fi
      ;;
  esac
done
if (( weeks < 1 )); then
  echo "activation-metric: weeks must be at least 1" >&2
  exit 1
fi

if [[ ! -f "$secrets_file" ]]; then
  echo "activation-metric: missing $secrets_file" >&2
  exit 1
fi
set -a
# shellcheck source=/dev/null
. "$secrets_file"
set +a

require_secret() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "activation-metric: $name is not set in secrets.env" >&2
    exit 1
  fi
}
require_secret KRATOS_PSQL_CONNECTION
require_secret DOVECOTE_PSQL_CONNECTION

# Decomposes a postgres://user:pass@host:port/db[?params] connection string
# into the standard PG* libpq environment variables and exports them, so
# the connection string itself never has to be passed to psql as a command
# argument (visible to anyone on the box via `ps`). Only sslmode is lifted
# out of an optional query string; anything else there is currently unused.
parse_pg_uri() {
  local uri="$1"
  local re='^[a-zA-Z][a-zA-Z0-9+.-]*://([^:@/]+):([^@]+)@([^:/]+):([0-9]+)/([^?]+)(\?(.*))?$'
  if [[ ! "$uri" =~ $re ]]; then
    echo "activation-metric: could not parse a postgres connection string" >&2
    exit 1
  fi
  PGUSER="${BASH_REMATCH[1]}"
  PGPASSWORD="${BASH_REMATCH[2]}"
  PGHOST="${BASH_REMATCH[3]}"
  PGPORT="${BASH_REMATCH[4]}"
  PGDATABASE="${BASH_REMATCH[5]}"
  export PGUSER PGPASSWORD PGHOST PGPORT PGDATABASE
  # Reset each call so a param present in one connection string can't leak
  # into a later connection whose string omits it.
  unset PGSSLMODE
  local query="${BASH_REMATCH[7]-}"
  if [[ -n "$query" ]]; then
    local pair
    local -a pairs
    IFS='&' read -ra pairs <<< "$query"
    for pair in "${pairs[@]}"; do
      case "$pair" in
        sslmode=*)
          PGSSLMODE="${pair#sslmode=}"
          export PGSSLMODE
          ;;
      esac
    done
  fi
}

# Query 1: every signup, from the Kratos database.
parse_pg_uri "$KRATOS_PSQL_CONNECTION"
signups_tsv=$(psql -X -q -At -F $'\t' -v ON_ERROR_STOP=1 -c "
  SELECT id, extract(epoch FROM created_at)::bigint
  FROM identities
  ORDER BY created_at
") || {
  echo "activation-metric: failed to read identities from the kratos database" >&2
  exit 1
}

# Build a VALUES list for the dovecote-side temp table. Both fields are
# validated before ever being embedded in SQL text: id must be a bare UUID
# and epoch must be a bare non-negative integer, so there is no free-text
# interpolation and no injection surface.
uuid_re='^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
epoch_re='^[0-9]+$'
signup_values=""
if [[ -n "$signups_tsv" ]]; then
  while IFS=$'\t' read -r id epoch; do
    if [[ ! "$id" =~ $uuid_re || ! "$epoch" =~ $epoch_re ]]; then
      echo "activation-metric: unexpected row shape reading identities, aborting" >&2
      exit 1
    fi
    signup_values+="('$id'::uuid, to_timestamp($epoch)),"
  done <<< "$signups_tsv"
fi
signup_values="${signup_values%,}"

if [[ -n "$signup_values" ]]; then
  insert_stmt="INSERT INTO signups (identity_id, created_at) VALUES $signup_values;"
else
  insert_stmt="-- no identities returned; signups stays empty"
fi

# Query 2: join against dovecote's own flocks/pigeons/telemetry/shadow
# tables in a single session, using the temp table built above instead of
# a cross-database query (this Postgres instance has no cross-database
# extension enabled, and Kratos/dovecote are separate logical databases
# even where they share a host).
parse_pg_uri "$DOVECOTE_PSQL_CONNECTION"

psql_flags=(-X -q -v "ON_ERROR_STOP=1")
if (( csv )); then
  psql_flags+=(--csv)
fi

psql "${psql_flags[@]}" -f - <<SQL
CREATE TEMP TABLE signups (
  identity_id uuid PRIMARY KEY,
  created_at timestamptz NOT NULL
);

$insert_stmt

WITH first_connected AS (
  SELECT
    f.user_id,
    MIN(
      LEAST(
        (SELECT MIN(pth.reported_at)
           FROM pigeon_telemetry_history pth
          WHERE pth.pigeon_id = p.id),
        (SELECT to_timestamp(ps.updated_at)
           FROM pigeon_shadow ps
          WHERE ps.id = p.id
            AND (ps.current_version > 0
                 OR (ps.current_config IS NOT NULL AND ps.current_config <> '{}'::jsonb)))
      )
    ) AS first_connected_at
  FROM flocks f
  JOIN pigeons p ON p.flock_id = f.id
  GROUP BY f.user_id
),
per_user AS (
  SELECT
    s.identity_id,
    s.created_at,
    (fc.first_connected_at IS NOT NULL
      AND fc.first_connected_at <= s.created_at + interval '7 days') AS activated
  FROM signups s
  LEFT JOIN first_connected fc ON fc.user_id = s.identity_id
),
weeks AS (
  SELECT gs AS week_start
  FROM generate_series(
    date_trunc('week', now() AT TIME ZONE 'utc') - (($weeks - 1) * interval '7 days'),
    date_trunc('week', now() AT TIME ZONE 'utc'),
    interval '7 days'
  ) AS gs
),
weekly AS (
  SELECT
    w.week_start,
    count(u.identity_id) AS signups,
    count(u.identity_id) FILTER (WHERE u.activated) AS activated_within_7d
  FROM weeks w
  LEFT JOIN per_user u
    ON date_trunc('week', u.created_at AT TIME ZONE 'utc') = w.week_start
  GROUP BY w.week_start
),
all_time AS (
  SELECT
    NULL::timestamp AS week_start,
    count(*) AS signups,
    count(*) FILTER (WHERE activated) AS activated_within_7d
  FROM per_user
)
SELECT
  COALESCE(to_char(week_start, 'YYYY-MM-DD'), 'all-time') AS week_start,
  signups,
  activated_within_7d,
  CASE
    WHEN signups = 0 THEN 'n/a'
    ELSE round(activated_within_7d::numeric / signups * 100, 1) || '%'
  END AS activation_rate
FROM (
  SELECT * FROM weekly
  UNION ALL
  SELECT * FROM all_time
) combined
ORDER BY week_start NULLS LAST;
SQL
