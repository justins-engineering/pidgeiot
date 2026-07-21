# Tenancy isolation: Postgres mirror + GreptimeDB (tasks #35, #36)

Status: design doc, no code changes. Covers both the Postgres/YugabyteDB
mirror (task #36) and GreptimeDB (task #35) together, since they're the same
underlying question — "what stops one tenant's cross-pigeon query from
returning another tenant's rows?" — asked against two different stores that
currently answer it two different ways.

Scope note up front: the Durable Objects are unaffected by anything in this
doc. Each pigeon's own DO is the source of truth and already does real
per-request auth (`X-User-Id` vs. `pigeon_acl` for dashboard routes,
Ed25519 device-token verification for device routes — see
`dovecote/CLAUDE.md`). Everything below is about the *mirrors* that exist so
dovecote can query across pigeons/flocks without asking every DO in a flock
one at a time.

## 1. Current state

### 1.1 Postgres (Hyperdrive binding `"YugabyteDB"`, task #36)

Audited every query against this binding (`dovecote/src/helpers/{pigeons,
telemetry,firmware,flocks}.rs`, every `get_db!`/`get_db_client` call site in
`dovecote/src/lib.rs`, and the two call sites inside
`dovecote/src/objects/pigeons.rs`). Full result already reported to
team-lead; summary:

- **No missing ownership-filter gap found.** Every query that returns
  cross-tenant data folds the ownership check directly into its `WHERE`:
  - `get_user_flocks` (`helpers/flocks.rs:6`) — `WHERE flocks.user_id = $1`.
  - `get_flock_pigeon_ids` / `query_telemetry_history_for_flock`
    (`helpers/telemetry.rs:134`, `:170`) — `JOIN flocks f ... WHERE f.id=$1
    AND f.user_id=$2`.
  - `is_flock_owner` (`helpers/firmware.rs:58`) gates firmware upload
    (`lib.rs:1107`) and firmware list (`lib.rs:1180`) before either touches
    `flock_firmware`.
  - In every case the `user_id` bound into the query is the Kratos-session
    identity resolved by `require_auth` (`lib.rs:68`) from the session
    cookie — never client-suppliable.
- **Two helpers intentionally trust the caller instead of filtering
  themselves**, and are the one structurally fragile spot:
  - `query_telemetry_history_for_pigeon` (`helpers/telemetry.rs:79`) — its
    doc comment says outright it "trusts pigeon_id unconditionally." Its
    only caller (`lib.rs:915-975`) proxies to the DO's `/pigeon/authz/check`
    first and returns the DO's error unchanged on `>=400` before this
    function ever runs. Verified `check_authorized` → `is_authorized`
    (`objects/pigeons.rs:437`) does a real `pigeon_acl` lookup keyed on
    `X-User-Id`.
  - `list_flock_firmware` (`helpers/firmware.rs:140`) — same contract,
    gated by `is_flock_owner` at its one call site (`lib.rs:1180`).
  - Both are safe *today* because both call sites happen to gate correctly.
    Neither function's signature enforces that a caller did — a new route
    added later that calls either helper directly (skipping the ACL/owner
    check) would silently leak cross-tenant rows, and nothing catches it at
    compile time or in a passing test suite. This is the single highest-
    value thing to fix, and it's cheap (§2.1).
- **Write-sync paths never need their own ownership check.** Every
  `insert_pigeon_pg_db` / `update_pigeon_pg_db` / `update_shadow_pg_db` /
  `upsert_acl_pg_db` / `update_telemetry_endpoint_pg_db` /
  `delete_pigeon_pg_db` call in `lib.rs` only fires *after* the
  corresponding DO route already returned success (`if do_response
  .status_code() >= 400 { return ... }` precedes every one of them — e.g.
  `lib.rs:645-659`, `:716-730`, `:745-757`, `:784-798`, `:824-838`,
  `:866-886`). The DO already did the real gating; Postgres here is
  best-effort mirroring of data that's already been authorized, matching
  the dual-persistence model in `dovecote/CLAUDE.md`.
- `/pigeons/batch` (`lib.rs:571`) never touches Postgres at all — it fans
  out per-pigeon-id straight to each pigeon's own DO with `X-User-Id`
  forwarded, relying on that DO's own `/pigeon/get` ACL check per pigeon.

**How isolation is enforced today, in one line:** app-layer SQL predicates,
100% of the time, no database-native isolation (no RLS, one shared
`dovecote` role via Hyperdrive for every tenant).

### 1.2 GreptimeDB (task #35)

- One shared measurement/table, `pigeon_telemetry`, holding every pigeon
  across every tenant in a given environment. It's a GreptimeDB **auto-
  schema wide table** (confirmed empirically, `helpers/greptime.rs:299`):
  every distinct telemetry key becomes its own column, `pigeon_id` is the
  tag/primary key, `greptime_timestamp` the time column.
- The *only* per-environment separation that exists is `GREPTIMEDB_DB`
  (`helpers/greptime.rs:34`) — a single database name per Worker
  environment (`dev`/`staging`/prod each get their own, see
  `wrangler.toml:59,130,168`), so staging telemetry can't land in prod's
  `public` db. This is environment isolation, not tenant isolation — every
  customer within one environment shares the same `pigeon_telemetry` table
  in the same Greptime database.
- Tenant isolation is enforced entirely by dovecote's query builder:
  - Per-pigeon read (`query_greptime_history_for_pigeon`,
    `helpers/greptime.rs:468`) and per-flock read
    (`query_greptime_history_for_pigeons`, `:486`) both build `SELECT *
    FROM pigeon_telemetry WHERE pigeon_id IN (...)` (`build_history_sql`,
    `:312`) against an explicit, pre-validated pigeon-ID allowlist.
  - The per-pigeon path's allowlist is exactly one ID, taken from the URL —
    same "caller must gate first" contract as its Postgres counterpart; its
    one call site (`lib.rs`, the `/pigeons/:id/telemetry/history` route)
    already ran the DO `/authz/check` before reaching it (§1.1 above — it's
    the same route, Greptime is just tried first with Postgres as
    fallback).
  - The per-flock path's allowlist comes from `get_flock_pigeon_ids`
    (Postgres, ownership-checked — §1.1) — Greptime has no `pigeons`/
    `flocks` tables of its own to resolve membership from, so this is
    unavoidably a two-store round trip: Postgres answers "which pigeon IDs
    does this user own in this flock," Greptime answers "give me telemetry
    for exactly these IDs."
  - `is_valid_pigeon_id` (`:295`) whitelists to ASCII-hex only before any ID
    is interpolated into a raw SQL string — Greptime's HTTP SQL endpoint has
    no bind-parameter mechanism, so this is the injection guard, not an
    isolation guard, but it's worth naming since a hole here would turn an
    isolation bug into a SQL-injection bug.
- **No database-native isolation exists or is planned in the current
  code**: no per-tenant database, no RLS-equivalent, no row-level ACL table
  inside Greptime itself (it doesn't have one). If the Rust-side `WHERE
  pigeon_id IN (...)` construction ever has a bug — e.g. a future
  "aggregate across all my flocks" endpoint that forgets to intersect with
  an owned-ID list, or a bug in how `pigeon_ids` gets built before reaching
  `query_greptime_history_for_pigeons` — there's nothing else standing
  between that bug and a cross-tenant read.
- The **per-pigeon user-configured `telemetry_endpoint`** path
  (`queue.rs::forward_line_protocol`) is a different concern entirely: it's
  the dashboard user's own external URL, deliberately never carries this
  Worker's own Greptime credentials (`greptime_access_headers`'s doc
  comment explains why), and isn't part of the multi-tenancy question — the
  user is forwarding their own data to their own endpoint.

**How isolation is enforced today, in one line:** identical shape to
Postgres — app-layer allowlist, no database-native isolation — except
Greptime *can't* do database-native row-level isolation the way Postgres
can (no RLS), so the only way to add DB-native defense-in-depth there is
data partitioning (separate databases), not policies.

## 2. Options

### 2.1 Postgres — cheap type-level guard (recommended near-term)

Make "the ACL/ownership check ran" part of the type a caller must produce,
instead of a doc-comment convention. Concretely: introduce a marker type
(e.g. `AuthorizedPigeon(String)` / `AuthorizedFlock(Uuid)`) that can only be
constructed by the functions that already do the real check
(`check_authorized`'s success path, `is_flock_owner`'s `true` branch), and
change `query_telemetry_history_for_pigeon`/`list_flock_firmware`'s
signatures to take that type instead of a bare `&str`/`String`. A caller
with only the raw ID literally cannot call the query function without
routing through the check first — the compiler enforces what today is only
a comment.

- Cost: small. Touches 2 function signatures, their 2 call sites, and
  whatever constructs the marker type (probably right where
  `authz_resp`/`owner` are checked today).
- Risk: none — it's a compile-time-only restriction, no schema change, no
  new runtime behavior, ships in the same PR as any other refactor.
- Doesn't help Greptime's equivalent pattern by itself, but the same
  marker type can be reused there too (§2.3) — one guard, shared by both
  stores' "trusts the caller" query functions.

### 2.2 Postgres — RLS via per-request `SET` + policies (defense-in-depth)

YugabyteDB is Postgres-wire-compatible and supports `ENABLE ROW LEVEL
SECURITY` / `CREATE POLICY`. The standard multi-tenant pattern: since every
row is owned by one Postgres role (`dovecote`, via Hyperdrive) rather than
one role per tenant, policies key off a session-local GUC
(`current_setting('app.current_user_id', true)`) instead of `current_user`,
and the app sets that GUC once per request to the already-resolved
`X-User-Id`.

**The pooling wrinkle turns out to be less scary than it first looks.**
Re-read `dovecote/src/helpers/hyperdrive.rs:38` while building this doc:
`get_db_client` opens a brand-new `Socket`/`tokio_postgres::Client` on
*every call* (`get_hyperdrive_conn`, `:3`) — there is no long-lived client
reused across unrelated requests within a Worker isolate. And the existing
code already depends on that one `Client` holding session affinity to a
single backend connection for a whole request's duration:
`insert_pigeon_pg_db` (`helpers/pigeons.rs:208`) opens a `client
.transaction()` and issues three statements before `commit()` — if
Hyperdrive silently multiplexed each statement to a different backend
connection mid-session, that transaction would already be broken today,
and it isn't. So a plain, non-`LOCAL`, session-level `SET
app.current_user_id = $1` issued immediately after `get_db_client` returns
should apply consistently to every query that `Client` runs for the rest of
that request — **not** the "wrap every single helper call in its own
transaction with `SET LOCAL`" refactor the task description worried about.

That said, this is an inference from the transaction behavior already
relied on, not a documented guarantee from Cloudflare about Hyperdrive's
pooling mode. Before committing to this design: run one empirical check
(open a client, `SET app.foo = 'bar'`, run a second unrelated query, read
back `current_setting('app.foo', true)`) against the real Hyperdrive
binding in dev, the same way the `board` column and the Greptime wide-table
behavior in this codebase were each verified empirically rather than
assumed from docs (`helpers/pigeons.rs:194`, `helpers/greptime.rs:299`).

- Cost: one `ALTER TABLE ... ENABLE ROW LEVEL SECURITY` + one `CREATE
  POLICY` per tenant-scoped table (`flocks`, `pigeons`, `pigeon_acl`,
  `pigeon_shadow`, `pigeon_telemetry_history`, `flock_firmware` — six
  tables, `init-db.sql`), one `SET app.current_user_id = $1` call added to
  `get_db_client` (or a thin wrapper around it) so every call site picks it
  up for free, and policies that walk the same `flock_id`/`flocks.user_id`
  join the app-layer queries already do (e.g. `pigeons`' policy would be
  `USING (flock_id IN (SELECT id FROM flocks WHERE user_id =
  current_setting('app.current_user_id')::uuid))`).
- Device-facing paths need a carve-out: `pigeons`/`pigeon_shadow` rows are
  also written from device-auth'd contexts that have no Kratos user id at
  all (`report_shadow_device`, etc. — device auth is per-pigeon Ed25519,
  not `X-User-Id`, per `dovecote/CLAUDE.md`). Those call sites would need
  either a policy `BYPASSRLS`-equivalent (a superuser/owner role — probably
  wrong, defeats the purpose) or a second, permissive-for-device-writes
  policy scoped by `id = current_setting('app.current_pigeon_id', true)`
  set from the same place `is_authorized_device` already resolves which
  pigeon owns the request. Doable, but it's real design work, not a
  one-liner.
- Payoff: genuine defense-in-depth — a future query that forgets its
  `WHERE` clause entirely (not just the "trusts the caller" pattern in
  §2.1, but a brand new bug) still can't return another tenant's rows,
  because Postgres itself won't return them regardless of what SQL dovecote
  sends.
- Given §1.1 found no live gap, this is worth doing but isn't urgent — it's
  insurance against future mistakes, not a fix for a current one.

### 2.3 GreptimeDB — per-flock (or per-user) database vs. keep shared-table + ACL

**Option A: keep the current shared-table + query-builder-allowlist model,
harden it with the same type-level guard as §2.1.** Apply the
`AuthorizedPigeon`/`AuthorizedFlock` marker type to
`query_greptime_history_for_pigeon(s)` too, so the one place that builds
the `pigeon_id IN (...)` list is structurally required to have come from a
checked source. Cheapest option; doesn't add DB-native isolation, but
closes the same class of future-regression risk §2.1 closes for Postgres,
and it's the same fix reused, not a second design.

**Option B: per-flock (or per-user) GreptimeDB database, `db=flock_<id>`.**
Mirrors what `GREPTIMEDB_DB` already does for environment separation, one
level down, for tenant separation.

- *Creation*: would need a `CREATE DATABASE flock_<uuid>` (Greptime's own
  SQL, same HTTP-SQL mechanism `query_greptime_sql` already uses) issued
  when `create_user_flock` (`helpers/flocks.rs:50`) runs. Today flock
  creation is a single Postgres `INSERT ... RETURNING`; this would add a
  second, cross-store side effect with its own failure mode (what happens
  to the flock if the Greptime `CREATE DATABASE` 400s or times out? — the
  existing codebase's answer to "cross-store write can fail" is always
  "log and don't block the primary write," so this would presumably follow
  suit: flock creation succeeds, its Greptime db gets lazily created on
  first telemetry write instead, closer to how `GREPTIMEDB_DB` already
  tolerates not existing yet, per `wrangler.toml:127`'s comment).
- *Teardown*: there is currently **no flock-delete route at all** (grepped
  `lib.rs` — pigeons have `DELETE /pigeons/:pigeon_id`, flocks don't). So
  "drop the database on flock delete" has no hook to attach to yet; this
  would be new surface area, not a gap in existing cleanup.
- *Write path*: every write site today (`write_telemetry_default`,
  `queue.rs::dispatch_to_do`, the two `objects/pigeons.rs` fallbacks) knows
  only a `pigeon_id`, not that pigeon's flock. `greptime_db(env)` is a
  single per-*environment* var today (`helpers/greptime.rs:34`) — moving to
  per-flock databases means every write call site needs a `pigeon_id ->
  flock_id -> db name` lookup before it can pick a target database, which
  is a Postgres round trip (`pigeons.flock_id`) on the hot device-telemetry
  path that doesn't exist today. That's a real latency/complexity cost on
  every single telemetry report, not a one-time migration cost.
- *Cross-flock reads*: `query_telemetry_history_for_flock` already only
  ever needs one flock's data, so that read gets simpler (one `db=`, no
  `WHERE pigeon_id IN`). But there is no current "across all my flocks"
  endpoint to weigh against — if one gets added later, it would need to
  fan out one HTTP-SQL request per flock-database rather than a single
  query, since Greptime SQL can't join across databases the way Postgres
  can join across schemas.
- *Migration*: existing shared-table rows for already-deployed pigeons
  would need to be re-written (Greptime has no cheap `ALTER ... SET
  SCHEMA`-equivalent for moving existing time-series data between
  databases that I could confirm) or left behind as a legacy shared
  database that new flocks simply don't write to anymore — a permanent
  split-brain in the data layout unless someone writes a backfill job.
- *Scaling*: GreptimeDB is designed around a bounded number of
  databases/tables per instance for metadata-overhead reasons (each
  database carries its own catalog/schema bookkeeping) — one database per
  flock scales with tenant count in a way the platform hasn't load-tested,
  versus the current single-table-many-tags design, which is exactly the
  pattern time-series databases are built for. This is the biggest open
  question for Option B and would need a real capacity test against a
  representative flock count before committing to it.

## 3. Recommendation

**Now (cheap, do both, no schema changes, no migration):**
1. §2.1 — wrap `query_telemetry_history_for_pigeon` and
   `list_flock_firmware` (Postgres) behind an `Authorized*` marker type
   that only the real ACL/ownership check can construct.
2. §2.3 Option A — apply the same marker type to
   `query_greptime_history_for_pigeon(s)`.

   Together these close the one real structural weak point this audit
   found (fragile-by-convention, not fragile-by-bug), in both stores, with
   one shared mechanism. This is the coherent-across-both-stores move: the
   "trusts the caller" pattern is identical in both, so the fix should be
   too, rather than inventing two different guard mechanisms for the same
   shape of problem.

**Next (worth doing, not urgent — defense-in-depth against future bugs,
not a fix for a current one):**
3. §2.2 — Postgres RLS via session-level `SET app.current_user_id`,
   *after* the one empirical Hyperdrive-pooling check described above comes
   back clean. Do this one first if the team wants DB-native isolation on
   any store, since it's a real (if non-trivial) win on the store that
   supports it natively, versus Greptime where the only DB-native
   equivalent (Option B) is a much bigger and less-validated lift.

**Not recommended right now: GreptimeDB per-flock databases (§2.3 Option
B).** The cost (new `pigeon_id → flock_id` lookup on the hot telemetry
write path, no existing teardown hook since flocks can't be deleted today,
unvalidated many-databases scaling, a real data migration for existing
rows) is high relative to the payoff, especially since §1.2 found the
current app-layer allowlist correct everywhere it's used today. Revisit
if/when: a flock-delete route gets built anyway (giving teardown a natural
hook), GreptimeDB's own docs/testing clarify its per-instance database
scaling limits, or a real incident/pentest finding shows the app-layer
allowlist model insufficient in practice — none of which are true today.

**One coherent tenancy story, stated plainly:** both stores are mirrors of
Durable-Object-authoritative data, both currently isolate tenants correctly
through application-layer query predicates alone, and both have exactly one
class of structural risk (a "caller must have already checked" convention
with no compiler enforcement). Fix that one class of risk the same way in
both stores first (§2.1 + §2.3A). Layer real DB-native isolation on top only
where it's cheap and low-risk (Postgres RLS, §2.2, pending the pooling
check) — Greptime's equivalent (per-tenant databases) is a materially
bigger and riskier change that the current audit gives no urgent reason to
take on.
