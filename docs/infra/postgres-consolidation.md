# Postgres consolidation: dropping single-node YugabyteDB + GreptimeDB for a
# leaner, $0/mo interim posture

Researched 2026-07-23. This is a **pre-revenue cost posture**, not a reversal
of [`production-ha-plan.md`](./production-ha-plan.md) — that doc's 3-node
YugabyteDB RF3 + GreptimeDB cluster plan stays on the shelf, unedited, as the
thing to come back to once there's real revenue to fund it. This doc answers
a narrower question: **while there's no cash flow, is single-node YugabyteDB
+ a separate GreptimeDB LXC still the right thing to be running at all**, or
is there a strictly simpler, cheaper posture that gives up nothing the
platform actually uses today? Pricing/limits are cited inline and will
drift — treat every number as "roughly this, as of this date," not a quote.

## TL;DR

- **Recommendation: fold GreptimeDB into Postgres (yes — see §3), and run
  Postgres self-hosted on the existing home-lab box (node 1) rather than a
  managed free tier.** Concretely: replace the single-node YugabyteDB
  install with plain PostgreSQL 18 on the same box, same Cloudflare Tunnel
  pattern already in place, decommission the GreptimeDB LXC entirely, and
  point Kratos + Hyperdrive at the new instance. Net infra cost: **$0/mo
  incremental** — this reuses hardware that's already paid for and already
  running, same as the GreptimeDB LXC and (presumably) Yugabyte do today.
- **The deciding factor the task asked me to nail down precisely: Neon's free
  tier is not actually free for this workload.** Neon's own control-plane
  health checks alone (`check_availability`, ~30-40/day, independent of any
  traffic this platform generates) keep a Neon compute at roughly **0.25 CU
  continuously — ~6 CU-hours/day, ~180 CU-hours/month** — which **on its
  own, before counting this platform's 5-minute alert-sweep cron or any real
  device telemetry**, blows past the free tier's 100 CU-hour/month cap
  (source: a Neon staff-confirmed GitHub discussion, cited below). Layering
  our own `*/5 * * * *` cron (prod **and** staging, both hitting Postgres)
  and per-device telemetry writes on top only makes this worse, not better.
  Realistic cost if forced onto Neon's metered Launch plan to keep this
  workload running: **≈$15-25/mo** (compute alone, at $0.106/CU-hour if the
  workload keeps a 0.25 CU compute continuously awake), not $0.
- **Supabase's free tier doesn't have Neon's CU-hour metering problem** (it's
  paused-after-7-days-idle, not billed-per-active-second, so routine
  platform activity keeps it running for free) — **but its own blocker is a
  genuine, unresolved Hyperdrive-compatibility risk**: Cloudflare's own
  Hyperdrive docs say to use Supabase's **Direct connection**, not the
  pooler — but Supabase's free-tier Direct connection is **IPv6-only**, and
  the IPv4 add-on that would work around that is **not available on the
  free tier at all** (Pro-and-above only). Whether Cloudflare Workers/
  Hyperdrive can actually reach an IPv6-only origin isn't clearly documented
  either way. This needs a real empirical test before Supabase free tier can
  be trusted at all — see §1.2.
- **Self-hosting sidesteps both problems entirely** and matches the existing
  pattern this repo already uses for Kratos and GreptimeDB (Docker/LXC on
  node 1, no third-party account limits, no cold-start latency on dashboard
  requests, no compute-hour budget to blow). The schema itself is already
  proven portable: dev's `docker-compose.yml` has been running plain
  `postgres:18-alpine` against the exact same `init-db.sql` this whole time
  (Yugabyte is only in staging/prod) — see §2.1. This is the strongest
  argument for self-hosting over either managed free tier: **the migration
  risk is close to zero because dev has effectively been dry-running it
  continuously already.**
- **Effort estimate: 1-2 focused sessions (roughly 6-10 hours), not a
  multi-week project** — see §4. The overwhelming majority of the "migration
  plan" work in §2 is operational (stand up Postgres on node 1, dump/
  restore, repoint two config values), not code, because dovecote's code has
  no Yugabyte-specific assumptions to unwind (see §2.1's audit).
- **Task #35 (per-flock DB isolation design) is unaffected in substance,
  reframed in venue**: `docs/design/tenancy-isolation.md` already assumed
  Postgres/Yugabyte-wire-compatible RLS (`ENABLE ROW LEVEL SECURITY`) as its
  own §2.2 recommendation — that design was already written against "some
  Postgres-wire-compatible database reached via Hyperdrive," not against any
  Yugabyte-specific feature, so nothing in that doc needs to change. It was
  already, in effect, a Postgres RLS question — this doc just confirms it
  more explicitly and removes the one open dependency that doc flagged
  (Hyperdrive's connection-pooling/session-affinity behavior), which is
  unaffected by which Postgres-wire database sits behind Hyperdrive. See
  §5.
- **The SYS-3 ×3 hardware purchase in `production-ha-plan.md` is deferred by
  this posture, explicitly** — this doc's whole premise is not spending that
  money right now. See §5.
- **What would make me NOT recommend consolidating**: if Justin's actual
  goal is "prove out the 3-node HA architecture soon regardless of revenue"
  (i.e., HA work is the point, not a cost problem) — this doc's premise is
  specifically "no cash flow right now," and if that constraint lifts sooner
  than expected, going straight to `production-ha-plan.md` instead of
  consolidating first is a reasonable choice too (see §4's "trigger to
  return to HA" criteria).

## 1. Target selection: managed free tier vs. self-hosted

### 1.1 Neon free tier

| Dimension | Free tier limit | Source |
|---|---|---|
| Compute | 100 CU-hours/mo, autoscales to 2 CU (~8GB RAM) when active | [[1]](#sources) |
| Autosuspend | Scale-to-zero after **5 minutes idle, fixed, cannot be disabled or reconfigured on Free** | [[2]](#sources) |
| Storage | 0.5 GB/project | [[1]](#sources) |
| Projects / branches | Up to 100 projects, 10 branches/project | [[1]](#sources) |
| Backup/PITR | Instant restore history: 6 hours, capped at 1 GB-month of change history; 1 manual snapshot | [[1]](#sources) |
| Egress | 5 GB public egress/project/mo | [[1]](#sources) |
| Overage behavior | CU-hours exhausted → **compute suspended until next billing period or upgrade** (not a bill) | [[1]](#sources) |

**The deciding finding, worked through precisely**: a Neon staff-confirmed
GitHub discussion (a **Launch**-plan project, so this is Neon's own
architecture, not a free-tier quirk) reports **~6 CU-hours/day** consumed
by a project seeing only ~30 real visits/day, with **30-40 suspend/resume
cycles/day** traced to Neon's own control-plane `check_availability` health
check — described by Neon staff as "a periodic load generated by the
Control Plane" independent of application traffic [[3]](#sources). 6
CU-hours/day × 30 days ≈ **180 CU-hours/month from Neon's own background
health-checking alone** — already 80% over the entire 100 CU-hour free
budget, **before this platform sends it a single request.**

Layer this platform's actual workload on top and it gets worse, not better:

- **The `*/5 * * * *` missing-heartbeat cron** (`dovecote/wrangler.toml`,
  both the production `[triggers]` block and `[env.staging.triggers]`) fires
  every 5 minutes in both environments, each invocation opening a Postgres
  connection via `evaluate_scheduled_alerts` (`dovecote/src/scheduled.rs`,
  `dovecote/src/helpers/alerts.rs`). Five minutes is *exactly* Neon free
  tier's fixed idle-suspend window — a cron this frequent, by itself,
  already keeps the same failure mode Neon's own health-check does: the
  compute never gets a genuine 5-minute idle gap to actually suspend in,
  so it trends toward being **counted as continuously active** rather than
  mostly-suspended.
- **Real device telemetry** (`queue.rs`'s consumer, `report_telemetry_device`,
  `handle_ws_telemetry` — every one of these calls
  `write_telemetry_default`, which now (if Greptime is folded per §3) always
  lands in Postgres) adds further activity at whatever cadence pigeons
  report on (`telemetry_interval`, commonly minutes) — any fleet with even a
  handful of active pigeons reporting more often than every 5 minutes keeps
  the compute active essentially continuously on its own, with or without
  the cron.
- **Net result**: a compute that's realistically continuously-or-near-
  continuously active, at the 0.25 CU floor, is **0.25 × 720 hours/month ≈
  180 CU-hours/month** — matching the health-check-alone finding almost
  exactly, and **1.8x the free allotment**. Once free CU-hours run out mid-
  month, the compute suspends until the next billing cycle **or an
  upgrade** [[1]](#sources) — i.e., the dashboard and every device route
  that touches Postgres goes down for whoever's left in the free tier that
  month, which is not an acceptable production failure mode.
- **Realistic cost if run for real on Neon**: the metered Launch plan has
  **no monthly minimum since December 2025** [[4]](#sources) and bills
  $0.106/CU-hour [[4]](#sources) — a continuously-active 0.25 CU compute
  costs **0.25 × 720 × $0.106 ≈ $19/month** for compute alone, plus storage
  at $0.35/GB-month (negligible at hobby data volumes). **Call it $15-25/mo,
  not $0** — a real, budgetable number if Neon is ever revisited, but not
  the free-tier outcome the "no cash flow" premise needs right now.

**Verdict: Neon free tier is not viable for this workload's actual traffic
shape.** This isn't a hypothetical edge case — it's Neon's own architecture
(control-plane health checks) colliding with this platform's own architecture
(a 5-minute cron in two environments plus a live device-telemetry write path)
in exactly the way that maximizes CU-hour consumption. Neon remains a
reasonable **paid** option later (~$15-25/mo, still cheap) if self-hosting
ever stops being viable, but it is not a free option today.

### 1.2 Supabase free tier

| Dimension | Free tier limit | Source |
|---|---|---|
| Database storage | 500 MB | [[5]](#sources) |
| File storage | 1 GB | [[5]](#sources) |
| Egress | 5 GB/mo | [[5]](#sources) |
| Compute | Shared CPU, always-on (Supabase does **not** scale-to-zero the way Neon does — you pay for/get the selected compute continuously, but on Free that's $0) | [[5]](#sources) |
| Pause behavior | Project **pauses after 7 days with zero API requests**; data retained, must be manually resumed | [[5]](#sources) |
| Backups | **None** on free tier; no PITR, no SLA | [[5]](#sources) |
| Active projects | Up to 2 | [[5]](#sources) |

Supabase's model sidesteps Neon's specific problem: since it doesn't meter
compute-seconds against a monthly budget, and this platform's cron + real
device traffic both guarantee routine API activity, the project would never
hit the 7-day-idle pause in practice. **No CU-hour blowout risk here.**

**But there is a real, unresolved compatibility risk specific to Hyperdrive
+ Supabase's free tier, not present on any paid Supabase tier:**

- Cloudflare's own Hyperdrive-for-Supabase guide is explicit: **"you should
  use the Direct connection connection string rather than the pooled
  connection strings"** [[6]](#sources) — Hyperdrive already does its own
  pooling, and stacking Supabase's own Supavisor transaction-mode pooler
  underneath it risks exactly the double-pooling/session-affinity problem
  this repo's own `docs/design/tenancy-isolation.md` §2.2 depends on
  *not* happening (its RLS design assumes one Hyperdrive `Client` keeps a
  stable backend connection for a whole request — see §5 below).
- **Supabase's free-tier Direct connection is IPv6-only** — "all Supabase
  databases provide a direct connection string that maps to an IPv6
  address" [[7]](#sources) — and the IPv4 add-on that would put a normal
  IPv4 address in front of it **"is unavailable on the Free plan"**,
  Pro-and-above only, at $0.0055/hr (~$4/mo) [[7]](#sources).
- Whether Cloudflare Workers/Hyperdrive's outbound networking can actually
  reach an IPv6-only origin is **not clearly documented either way** in
  this research pass — community reports are mixed and inconclusive
  [[8]](#sources). If it can't, the only fallback is Supabase's IPv4
  Supavisor pooler, which is exactly the "don't do this" path Cloudflare's
  own docs warn against, for reasons that matter to this project's own
  planned RLS work, not just performance.
- **This is a real go/no-go gate, not a hypothetical**: before Supabase free
  tier can be trusted for anything beyond a throwaway test, someone needs to
  actually stand up a free Supabase project and confirm a Hyperdrive binding
  can reach its Direct/IPv6 connection string end-to-end from a real Worker
  request. This doc can't resolve that from static research — it's flagged
  as the one item that would change the Supabase recommendation from "not
  recommended" to "viable," and it's a cheap, fast thing to check (create
  one free project, wire one Hyperdrive binding, run one query) if there's
  ever a reason to prefer Supabase's specific extras (built-in auth, storage,
  edge functions) over plain self-hosted Postgres.

**Verdict: not recommended right now**, on the strength of the unresolved
IPv6 compatibility question alone — not because Supabase's limits are
inherently worse than Neon's (they're arguably better-suited to this
workload's shape), but because "will Hyperdrive even connect" is a more basic
question than cost, and this research pass couldn't close it out.

### 1.3 Self-hosted PostgreSQL on the existing home-lab box (recommended)

- **This is the same pattern the platform already uses successfully for
  Kratos (`infra/docker-compose.yml`) and GreptimeDB
  (`infra/proxmox-greptimedb-lxc.sh`)** — a Cloudflare Tunnel in front of a
  self-hosted service on node 1, no new operational pattern to learn.
  `second-node-hosting.md` already notes Yugabyte's own provisioning isn't
  in this repo, presumably reached through an equivalent tunnel — swapping
  the database *software* on that same box (Yugabyte out, plain Postgres
  18 in) changes nothing about the networking/tunnel/Access-policy shape
  already in place, only what's listening on the other end of it.
- **$0/mo incremental** — no new hardware, no new account, no new billing
  relationship. This is the only option of the three that actually hits the
  "$0/mo target" stated in the task, not "close to $0 with caveats."
- **No cold-start latency, no compute-hour budget, no IPv6 question.** A
  dashboard request or device report either reaches the box or it doesn't —
  the failure modes are the same ones node 1 already has today for Kratos
  and GreptimeDB (home-lab power/ISP reliability, already accepted as the
  known tradeoff of not yet being on the HA plan — see
  `production-ha-plan.md`'s own reasoning on why node 1 isn't a voting
  member, which applies here too but was never a blocker for running Kratos
  or GreptimeDB there).
- **Real backup story requires equivalent self-discipline** (a periodic
  `pg_dump`/`pg_basebackup` to R2, same idea `production-ha-plan.md` already
  proposes for its own HA scenario) — no managed-provider PITR safety net
  either way, but this was already true of the current single-node Yugabyte
  setup, so it's not a regression.
- **Effort**: install Postgres 18 (matching dev's pinned version in
  `infra/docker-compose.yml`) either as a new LXC (mirroring
  `proxmox-greptimedb-lxc.sh`'s pattern) or inside the existing box's
  Yugabyte-hosting environment if it's already containerized there too —
  this doc doesn't have visibility into exactly how Yugabyte is packaged on
  node 1 today (per `second-node-hosting.md`'s own admission that "YugabyteDB's
  own provisioning script isn't in this repo"), so the precise steps depend
  on what's actually there — see the open question in §2.4.

**Recommendation: self-host.** It's the only option that actually delivers
$0/mo with no asterisks, it reuses an operational pattern already proven
twice in this stack, and — per §2.1 below — the schema migration risk is
about as low as this kind of migration ever gets, because dev has already
been continuously validating "this schema runs fine on plain Postgres" for
as long as `docker-compose.yml`'s `postgresd` service has existed.

## 2. Migration plan: YugabyteDB → self-hosted PostgreSQL

### 2.1 Schema port — the good news first

**`infra/init-db.sql` has already been running against plain
`postgres:18-alpine` continuously in dev** (`infra/docker-compose.yml`'s
`postgresd` service) — Yugabyte is *only* in staging/prod via the
`[[hyperdrive]]` binding's connection string; dev's own `[[env.dev.hyperdrive]]`
binding already points `localConnectionString` at this same vanilla-Postgres
container (`postgres://kratos:secret@127.0.0.1:5432/dovecote?sslmode=disable`).
This means the schema-portability audit the task asked for has, in effect,
already been running as a continuous integration test since dev's compose
file was written — every `cargo check -p dovecote` + `wrangler dev`
session against dev has already exercised this exact schema against real
(non-Yugabyte) Postgres.

Auditing `init-db.sql` and every runtime `ensure_*`/`CREATE TABLE`
statement in `dovecote/src/helpers/{alerts,telemetry,pigeons,firmware}.rs`
for Yugabyte-specific syntax turned up **nothing that doesn't already run on
vanilla Postgres**:

- `gen_random_uuid()` — built into Postgres 13+ (`pgcrypto`/`pgcrypto`-free
  since PG13's native `gen_random_uuid()`), not Yugabyte-specific.
- `JSONB`, `TIMESTAMPTZ`, `BIGSERIAL`, triggers (`trigger_set_timestamp`,
  `trigger_prevent_immutable_updates`), `CHECK` constraints, partial indexes
  (`WHERE pigeon_id IS NOT NULL`) — all standard Postgres, all already
  exercised in dev.
- **No `CREATE INDEX CONCURRENTLY` anywhere in this codebase** — the
  "concurrent-index/transaction-block" incident lived in a since-deleted,
  never-committed migration file (`migrations/2026-07-22-alerts.sql`, applied
  by hand 2026-07-22 then removed once `init-db.sql` absorbed its schema, so
  it's invisible to git log). What happened: the migration was originally
  wrapped in `BEGIN`/`COMMIT` and YugabyteDB rejected it with "Create index
  in transaction block cannot be concurrent" — **Yugabyte builds even plain
  `CREATE INDEX` online (concurrently) by default** (yugabyte-db issue
  #6240), so index creation can't run inside an explicit transaction there.
  The fix was removing the transaction wrapper. **Migration relevance: this
  constraint disappears on vanilla Postgres** — plain `CREATE INDEX` inside
  a transaction is fine there, so future migrations get *simpler* after the
  move, not trickier. What *is* true and worth noting for the future:
  every runtime schema-bootstrap helper (`ensure_alert_tables`,
  `ensure_telemetry_history_table`, etc.) uses `client.batch_execute` with
  multiple statements, which Postgres's simple-query protocol runs as an
  implicit transaction block — and `CREATE INDEX CONCURRENTLY` cannot run
  inside a transaction block **on any Postgres**, vanilla or Yugabyte. This
  isn't a Yugabyte-specific gotcha to design around during migration; it's a
  general constraint that already applies today and would need the same
  workaround (a separate non-batched call) regardless of which Postgres-wire
  database is behind Hyperdrive, if `CONCURRENTLY` is ever introduced later.
- `ensure_alert_tables`'s own doc comment already notes Postgres has no
  `CREATE TRIGGER IF NOT EXISTS` — this is a vanilla-Postgres limitation the
  code already works around (skips re-creating the trigger in the runtime
  helper, relies on `init-db.sql`'s one-time `CREATE TRIGGER` for a fresh
  database), not something the migration changes.

**Net: no schema changes needed.** The dump/restore in §2.2 can go straight
across.

### 2.2 Data move

- Given hobby-scale data volume (a handful of flocks/pigeons, whatever
  telemetry history has accumulated), a straightforward **maintenance-window
  dump/restore** is the right level of ceremony — not a live-replication cutover.
- **From Yugabyte**: `ysql_dump` (Yugabyte's `pg_dump`-compatible tool,
  YSQL-aware) against the current instance, or plain `pg_dump` since
  Yugabyte is Postgres-wire-compatible and this schema uses nothing
  Yugabyte-specific (§2.1) — either should produce a standard SQL dump.
- **Into new Postgres**: plain `psql < dump.sql` or `pg_restore`, into a
  freshly created `dovecote` database/role on the new instance (same
  `CREATE ROLE dovecote WITH LOGIN PASSWORD '...'; CREATE DATABASE dovecote
  OWNER dovecote;` preamble `init-db.sql` already has, then the rest of
  `init-db.sql` for a from-scratch target, or the dump's own `CREATE TABLE`
  statements if restoring data-and-schema together).
- **Sequencing to minimize downtime**: since dovecote's DOs are the
  authoritative source of truth and Postgres is only ever a best-effort
  mirror (`dovecote/CLAUDE.md`'s dual-persistence model), a short window
  where Postgres-mirror reads/writes fail doesn't lose any data — worst case
  during the cutover window is a few best-effort sync failures that get
  logged and silently skipped, exactly the same as any other transient
  Postgres hiccup this codebase already tolerates by design. Practical
  sequencing:
  1. Stand up new Postgres instance, run `init-db.sql` fresh (or restore a
     dump taken moments before cutover) against it.
  2. Update `wrangler.toml`'s `[[hyperdrive]]`/`[[env.staging.hyperdrive]]`
     `id` (and `[[env.dev.hyperdrive]]`'s `localConnectionString`, if dev's
     target changes too — though dev is already on vanilla Postgres and
     doesn't strictly need to move) to point at the new instance.
  3. `wrangler deploy` (both prod and staging scripts) to pick up the new
     Hyperdrive config.
  4. Confirm a few real requests round-trip correctly (dashboard pigeon
     list, a telemetry history query) against the new instance.
  5. Decommission the old Yugabyte instance once confident.
- **Hyperdrive repoint is genuinely zero application-code changes** —
  confirmed by re-reading `dovecote/src/helpers/hyperdrive.rs`: it calls
  `env.hyperdrive("YugabyteDB")` and does nothing else provider-specific;
  TLS is handled generically (`SecureTransport::StartTls` +
  `PassthroughTls`) by Hyperdrive itself regardless of what's behind it.
  The binding name `"YugabyteDB"` is just a string key at this point — it
  can stay as-is (purely cosmetic, if slightly confusing after this
  migration) or be renamed in one small follow-up PR touching
  `hyperdrive.rs` + all three `wrangler.toml` blocks. Not required for the
  migration to work; worth doing eventually for clarity, not urgent.
- **Kratos DSN repoint + its own migrations**: `schemas/kratos/kratos.yml`'s
  `dsn` and `infra/docker-compose.yml`'s `kratos-migrate`/`kratos` services'
  `DSN` env var both need to point at wherever Kratos's database actually
  lives post-migration, then `kratos migrate sql -e --yes` needs a fresh run
  against the new target (same command `kratos-migrate` already runs on
  every dev `docker-compose up`, just needs to be re-run once against
  the new production/staging destination). **Open question this doc
  can't close from static repo research** (flagged for confirmation, not
  guessed at): does prod Kratos currently share the *same* Postgres/Yugabyte
  instance as dovecote (different database, `kratos` vs `dovecote`, mirroring
  exactly what dev's one `postgresd` container already does), or does it run
  its own separate self-hosted Postgres? Dev's pattern (one Postgres
  instance, two databases) is the best evidence available and the more
  likely setup by convention, but `infra/docker-compose.yml` is documented
  in this repo as the **local dev** stack (`docker-compose -f
  docker-compose.yml up`, per root `CLAUDE.md`'s dev commands) — what
  actually runs on node 1 for staging/prod isn't represented in this repo at
  all (same gap `second-node-hosting.md` already flagged for Yugabyte's own
  provisioning). **Confirm this on the home lab directly before planning
  exact sequencing** — if Kratos and dovecote already share one instance,
  this is one dump/restore/repoint, not two.

### 2.3 Verification checklist

- [ ] `init-db.sql` runs clean end-to-end against the fresh instance (it
      already does in dev — this just confirms the same is true wherever
      the new self-hosted instance lands).
- [ ] Row counts match between old and new for every table after the dump/
      restore (`flocks`, `pigeons`, `pigeon_acl`, `pigeon_shadow`,
      `pigeon_telemetry_history`, `flock_firmware`, `alert_definitions`,
      `alert_state`).
- [ ] A live dashboard session (Kratos login, pigeon list, pigeon detail,
      telemetry history graph) round-trips correctly against the new
      instance in staging first, then prod.
- [ ] A real device request (shadow report, telemetry report) round-trips
      correctly — confirms `report_shadow_device`/`report_telemetry_device`'s
      Postgres-sync paths still work.
- [ ] The `*/5 * * * *` alert-sweep cron fires clean against the new
      instance (`wrangler tail` or logs during a live cron window, or the
      local `cdn-cgi/handler/scheduled` test endpoint against a
      staging-pointed dev config).
- [ ] Kratos login/registration/recovery flows all still work post-DSN-
      repoint (confirms its own migration ran clean).

### 2.4 Rollback plan

- Keep the old Yugabyte instance running, untouched, for a defined grace
  period (e.g. 1-2 weeks) after cutover — reverting is just pointing
  `wrangler.toml`'s Hyperdrive `id`/`localConnectionString` (and Kratos's
  `dsn`) back and redeploying, exactly the same mechanism as the forward
  migration, in reverse.
- Since Postgres is a best-effort mirror and DOs remain authoritative
  throughout, a rollback loses at most whatever best-effort syncs happened
  *only* against the new instance during the cutover window — the DOs
  themselves are never at risk either direction.
- Take one more `pg_dump` of the new instance immediately before decommissioning
  the old one, so there's a recovery point even after the old instance is
  gone.

## 3. Fold GreptimeDB in, or keep it? — **Fold in.**

- **Query patterns actually used**: `GET /pigeons/:id/telemetry/history` and
  the flock-wide variant, both key/since/until-filterable with a 5000-point
  cap. `pigeon_telemetry_history` (`init-db.sql`, already the existing
  fallback path via `write_telemetry_default`/`query_telemetry_history_for_pigeon(s)`
  in `dovecote/src/helpers/telemetry.rs`) **already implements exactly this
  read shape today** — it's not a new code path to build, it's the code path
  that already runs whenever Greptime is unset or a write to it fails. At
  hobby-to-small-IoT scale (the same scale `production-ha-plan.md` and
  `second-node-hosting.md` both size everything else around), a row-per-
  key-per-report table with a `(pigeon_id, reported_at)` index (already
  present) comfortably handles this — nothing about the actual query
  patterns needs Greptime's wide-table/auto-schema model.
- **`fancier`'s graph UI**: per `init-greptime.sh`'s own reasoning for
  picking a 90-day TTL, the dashboard's `TimeRange` presets top out at 30
  days today — Postgres's existing 5000-point cap and index shape already
  serve this range fine (it's the same code path that already backs every
  Greptime-unset environment, e.g. dev, right now).
- **User-configurable `telemetry_endpoint` forwarding is unaffected either
  way** — it bypasses this platform's own storage entirely
  (`queue.rs::forward_line_protocol` posts line-protocol straight to the
  user's own URL). Folding in our own default Greptime instance doesn't
  touch this feature at all; `build_line_protocol`/`escape_key_or_tag`/
  `escape_field_string`/`post_line_protocol`
  (`dovecote/src/helpers/greptime.rs`) stay exactly as they are, since
  they're shared by both the (now-removed) default write path and the
  per-pigeon forwarding path. **Only `write_greptime_default`,
  `query_greptime_sql`, `query_greptime_history_for_pigeon(s)`, and the
  `greptime_origin`/`greptime_db`/`greptime_auth_token`/
  `greptime_access_headers` config helpers become dead code and get
  removed** — the line-protocol-building utilities the forwarding feature
  depends on are not part of the removal.
- **Retention**: propose the simplest possible Postgres equivalent —
  a periodic `DELETE FROM pigeon_telemetry_history WHERE reported_at < now()
  - interval '90 days'`, matching the same 90-day window
  `init-greptime.sh` already picked (keeps parity with whatever's already
  been decided, rather than re-litigating the number). Simplest
  implementation: fold this into the existing `#[event(scheduled)]` handler
  (`dovecote/src/scheduled.rs`) that already fires every 5 minutes for the
  alert sweep — a `DELETE ... WHERE reported_at < ...` with the existing
  `(pigeon_id, reported_at)` index is cheap enough to just run on the same
  cadence as a no-op-shaped statement (deletes nothing on 99% of
  invocations), no need for `pg_partman` or a separate cron trigger at this
  scale. If the 5-minute cadence ever feels wasteful, a simple day-of-month/
  hour check inside the handler (`if now.hour() == 3 { ...run the delete...
  }`) gets it down to once a day for near-zero extra code.
- **If folded, decommission list**:
  - The GreptimeDB LXC itself (`infra/proxmox-greptimedb-lxc.sh`'s deployed
    container) and its Cloudflare Tunnel + Access policy
    (`telemetry.pidgeiot.com`).
  - `infra/docker-compose.yml`'s `greptimedb` service (dev no longer needs
    it once the default write/read path is Postgres-only).
  - `infra/init-greptime.sh` (no more instance to initialize/set retention
    on).
  - `GREPTIMEDB_ENDPOINT`/`GREPTIMEDB_DB` vars in all three `wrangler.toml`
    blocks, and the `GREPTIMEDB_AUTH_TOKEN`/`GREPTIMEDB_ACCESS_CLIENT_ID`/
    `GREPTIMEDB_ACCESS_CLIENT_SECRET` Worker secrets (`wrangler secret
    delete ... --env ...`).
  - `write_greptime_default`/`query_greptime_sql`/
    `query_greptime_history_for_pigeon(s)`/the four `greptime_*` config
    helpers in `dovecote/src/helpers/greptime.rs`, and the two call sites in
    `dovecote/src/lib.rs`/`objects/pigeons.rs` that currently try Greptime
    first and fall back to Postgres — those become "just call the Postgres
    path directly," which is also a nice simplification (removes an entire
    fallback branch, not just config).
  - This doc's own `production-ha-plan.md` is **not** edited to remove
    GreptimeDB's clustering section — that plan is shelved as a whole, not
    selectively pruned; if/when revenue justifies revisiting it, the
    decision of whether Greptime is even still part of the target
    architecture at that point is a fresh call, not something this doc
    should presume by editing that file now.

**Recommendation: fold in.** This isn't just "acceptable" given the existing
fallback — it's a straightforward simplification: one fewer self-hosted
service, one fewer Cloudflare Tunnel, one fewer set of secrets, one fewer
piece of client code with a fallback branch to maintain, and zero loss of
capability against what's actually queried today.

## 4. Cost + effort summary

| | Today | Post-consolidation |
|---|---|---|
| Database compute | Single-node YugabyteDB (home lab, $0 marginal, but distributed-DB operational overhead for zero HA payoff at RF1) | Single-node vanilla PostgreSQL 18 (same box, same $0 marginal, no distributed-DB tooling/overhead) |
| Telemetry store | GreptimeDB LXC + Cloudflare Tunnel (home lab, $0 marginal, separate service to patch/monitor/back up) | Folded into the same Postgres instance — one fewer service |
| Monthly infra cost | $0 (self-hosted) | **$0** (self-hosted) — the managed-tier alternatives surveyed in §1 would cost **$0 only if Supabase's IPv6 question resolves favorably**, or **~$15-25/mo on Neon** once its CU-hour ceiling is hit |
| Services to operate on node 1 | Kratos, YugabyteDB, GreptimeDB (3) | Kratos, PostgreSQL (2) |

- **Effort estimate**: this is a **1-2 focused session** (roughly **6-10
  hours** total) task, phased:
  1. **Stand up Postgres on node 1** (~1-2 hrs) — install/containerize
     Postgres 18, mirror the existing Tunnel+Access pattern already proven
     for GreptimeDB.
  2. **Dump/restore + verification** (~2-3 hrs) — `ysql_dump`/`pg_dump`,
     restore, run the §2.3 checklist against staging first.
  3. **Config repoint + redeploy** (~1 hr) — `wrangler.toml` Hyperdrive
     `id`s, Kratos `dsn`, `wrangler deploy` both scripts, re-run Kratos
     migrations.
  4. **Greptime fold-in code removal** (~2-3 hrs) — delete the four
     now-dead functions + config helpers in `greptime.rs`, simplify the two
     call sites that currently try-Greptime-then-fall-back, add the
     retention DELETE to `scheduled.rs`, remove the LXC/tunnel/vars/secrets.
  5. **Decommission old instances** (~30 min, after the rollback grace
     period in §2.4 passes).
- **Trigger criteria to come back off this posture onto `production-ha-plan.md`**:
  recurring revenue (even modest — the plan's own $185-215/mo bare-metal
  total is the number to compare against), a real paying customer whose
  contract implies an uptime expectation node 1 alone can't honestly promise,
  or a device fleet/telemetry volume that starts actually stressing a
  single-node Postgres instance (not hypothetically — a real, observed
  resource ceiling, the same "derive it from real numbers, not
  hypothesis" discipline `production-ha-plan.md` already applied to compute
  sizing).

## 5. Impact on open work

- **Task #35 (per-flock DB isolation, `docs/design/tenancy-isolation.md`)
  becomes explicitly a Postgres RLS/schema question under this plan** — but
  it already effectively was one. That doc's §2.2 recommendation (`ENABLE
  ROW LEVEL SECURITY` + session-local `SET app.current_user_id`) was already
  written against "whatever Postgres-wire-compatible database Hyperdrive
  reaches," not against any Yugabyte-specific capability — it explicitly
  notes "YugabyteDB is Postgres-wire-compatible and supports `ENABLE ROW
  LEVEL SECURITY`," treating that compatibility as the baseline, not the
  point. The one real open dependency that design flagged — whether
  Hyperdrive's connection pooling preserves session-local `SET` state for a
  whole request — is a property of **Hyperdrive itself**, not of what's
  behind it, so this migration doesn't reopen or resolve that question
  either way; the same "run one empirical check against the real Hyperdrive
  binding" recommendation in that doc still applies verbatim, just against
  the new self-hosted Postgres instead of Yugabyte.
- **The SYS-3 ×3 hardware purchase (`production-ha-plan.md`'s bill of
  materials) is deferred by this posture, explicitly.** This doc's entire
  premise — no cash flow right now — is incompatible with also spending
  ~$180-215/mo on 3 fresh DC boxes. `production-ha-plan.md` itself is not
  edited or contradicted; it stays exactly as written, ready to execute once
  the trigger criteria in §4 are met.

## Sources

1. Neon Free Plan limits (compute, storage, projects/branches, backup/PITR,
   egress, overage behavior) —
   [Neon FAQ: What are the limits and quotas for Neon's Free plan?](https://neon.com/faqs/free-plan-limits-and-quotas)
   (accessed 2026-07-23)
2. Neon autosuspend/scale-to-zero default timeout (5 min, fixed on Free) —
   [Configuring Scale to Zero for Neon computes](https://neon.com/docs/guides/scale-to-zero-guide),
   [Scale to Zero — Neon Docs](https://neon.com/docs/introduction/scale-to-zero)
   (accessed 2026-07-23)
3. Real-world Neon compute-never-truly-scales-to-zero finding (~6 CU-hour/day
   on ~30 visits/day, Neon-staff-attributed to `check_availability`
   control-plane health checks) —
   [Compute never truly scales to zero despite auto-suspend — GitHub Discussion #12900](https://github.com/neondatabase/neon/discussions/12900)
   (accessed 2026-07-23)
4. Neon Launch plan pricing (no monthly minimum since Dec 2025, $0.106/
   CU-hour, $0.35/GB-month storage) —
   [Neon Pricing: The Honest Cost of Serverless Postgres](https://selfhost.dev/blog/neon-pricing-cost-of-serverless-postgres/),
   [Neon plans — Neon Docs](https://neon.com/docs/introduction/plans)
   (accessed 2026-07-23)
5. Supabase Free tier limits (storage, egress, compute/pause behavior, no
   backups) —
   [Supabase Free Tier Limits in 2026: Hidden Pauses & Caps](https://www.itpathsolutions.com/supabase-free-tier-limits)
   (accessed 2026-07-23)
6. Cloudflare Hyperdrive + Supabase official setup guide (use Direct
   connection, not pooled) —
   [Supabase — Cloudflare Hyperdrive docs](https://developers.cloudflare.com/hyperdrive/examples/connect-to-postgres/postgres-database-providers/supabase/)
   (accessed 2026-07-23)
7. Supabase Direct connection IPv6-only on all tiers, IPv4 add-on Pro-and-
   above only ($0.0055/hr) —
   [Supabase & Your Network: IPv4 and IPv6 compatibility](https://supabase.com/docs/guides/troubleshooting/supabase--your-network-ipv4-and-ipv6-compatibility-cHe3BP),
   [Dedicated IPv4 Address for Ingress — Supabase Docs](https://supabase.com/docs/guides/platform/ipv4-address)
   (accessed 2026-07-23)
8. Cloudflare Workers/Hyperdrive outbound IPv6 support — inconclusive
   community reports, no definitive first-party confirmation found either
   way —
   [Outbound IPv6 in Cloudflare Workers — Cloudflare Community](https://community.cloudflare.com/t/outbound-ipv6-in-cloudfare-workers/486346)
   (accessed 2026-07-23)
9. Cloudflare Hyperdrive general connection-pooling behavior (transaction-
   scoped pooling, `SET` statement handling, Neon-serverless-driver
   incompatibility note) —
   [Connection pooling — Cloudflare Hyperdrive docs](https://developers.cloudflare.com/hyperdrive/concepts/connection-pooling/),
   [Neon — Cloudflare Hyperdrive docs](https://developers.cloudflare.com/hyperdrive/examples/connect-to-postgres/postgres-database-providers/neon/)
   (accessed 2026-07-23)
10. Cloudflare Hyperdrive full supported-provider list (15 providers
    including Neon, Supabase) —
    [Postgres database providers — Cloudflare Hyperdrive docs](https://developers.cloudflare.com/hyperdrive/examples/connect-to-postgres/postgres-database-providers/)
    (accessed 2026-07-23)
