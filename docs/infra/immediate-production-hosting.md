# Immediate production hosting: the fastest credible path off the home lab

Researched 2026-07-26. This doc completes a trilogy and contradicts neither
sibling — it's the **bridge** between them:

- [`postgres-consolidation.md`](./postgres-consolidation.md) is the **$0/mo
  home-lab posture** (fold GreptimeDB into Postgres, self-host plain PG 18 on
  node 1). Its two load-bearing findings — Neon's free tier fails this
  workload's 24/7 traffic shape, and the Greptime fold-in is a strict
  simplification — both carry forward here unchanged. What it deliberately
  did *not* evaluate was **paid** managed tiers; that's this doc's job.
- [`production-ha-plan.md`](./production-ha-plan.md) is the **endgame**: 3
  co-located US-East bare-metal nodes, HA failover, ≈$185-215/mo — shelved
  until revenue. This doc does not touch that plan.
  [`distributed-sql-comparison.md`](./distributed-sql-comparison.md)
  (landed in parallel with this doc) re-decides that plan's database slot
  (Postgres + Patroni over YugabyteDB) and independently reinforces this
  doc's Postgres direction: Ory officially supports exactly PostgreSQL —
  not Yugabyte — so a plain managed-Postgres target is the zero-dialect-risk
  home for the Kratos DSN both now and at the HA endgame.
- **This doc**: what to sign up for **today** to make the self-hosted backend
  services production-ready **now** — vendor backups/PITR, DC power and
  network, no residential single point — at the lowest defensible cost,
  optimizing each service independently. Splitting services across providers
  and hosting types is explicitly acceptable "for now."

**Out of scope, deliberately**: the edge and the dashboards are **already
production-grade on Cloudflare** and do not move. `dovecote` (Workers +
Durable Objects + Queues + R2 + Hyperdrive, `api.pidgeiot.com`) and `fancier`
(static assets on Workers, `pidgeiot.com`) are serverless on Cloudflare's
paid plan; jes-bid-screener is likewise all-Cloudflare. Crucially, the
DO-authoritative model means most *pigeon* state is already durably hosted in
Cloudflare's infrastructure. What lives **only** on the home-lab box today —
and is therefore the actual single point of failure this doc eliminates — is:
Kratos identities (real user accounts), the `flocks` table, the Postgres
mirror + telemetry history, alert definitions/state, and the firmware
catalog rows (image bytes are already in R2).

Pricing is cited inline with access dates and will drift — treat every
number as "roughly this, as of this date," not a quote.

## TL;DR

- **Recommended mix (≈ $15/mo, ~6-8 focused hours to migrate):**
  1. **PostgreSQL → Crunchy Bridge Hobby-0, ≈ $10/mo** ($9/mo machine +
     $0.10/GB-mo storage) [[7]](#sources) — a dedicated, always-on managed
     Postgres with daily backups **and continuous WAL archiving / restore to
     any point in time included at the hobby tier**, hourly-billed, on AWS
     us-east-1. One cluster hosts **both** the `dovecote` database (repoint
     Hyperdrive) and the `kratos` database (repoint DSN) — same
     two-databases-one-instance shape dev's `docker-compose.yml` already
     runs. This is the answer to the "does a managed PG under $20 with PITR
     that Hyperdrive likes exist?" question: **yes.**
  2. **Kratos → self-host on one small US-East VPS, ≈ $4.50-6/mo** (OVH
     VPS-1, 2 vCPU/4GB, Vint Hill VA, $4.54/mo [[12]](#sources); or a $6
     DigitalOcean droplet). Kratos is a light stateless Go binary; its state
     moves into the managed Postgres above, so the VPS itself becomes
     disposable — re-provisionable in under an hour with zero data loss.
     **Ory Network was evaluated seriously and loses on price**: its free
     tier has no production environment and no custom domain, and the
     cheapest production-usable plan is **$770/yr ≈ $64/mo** [[4]](#sources)
     — 13× the VPS, for a pre-revenue platform. Verdict and migration-path
     details in §2; revisit at revenue, where "identity patched by the
     vendor" genuinely earns its premium.
  3. **GreptimeDB → fold into Postgres, $0** — exactly
     `postgres-consolidation.md` §3's finding, which gets *stronger* here:
     with PG managed, folding in means one less thing to host **anywhere**.
     Config-level change (dovecote's `write_telemetry_default` PG fallback
     already exists). Runner-up: GreptimeCloud's free Hobby tier genuinely
     covers this workload (40 RCU/s, 20 WCU/s, 5GB, 3-month retention ≈ our
     existing 90-day TTL) [[5]](#sources) — see §3 ranking.
- **Budget-min variant (≈ $8.50/mo)**: one OVH VPS-2 (4 vCPU/8GB, $8.50/mo
  [[12]](#sources)) running PG 18 + Kratos together, Greptime folded. Honest
  caveat: this is "off the residential ISP," not "production-ready" — you're
  back to self-managed backups and a single box; it buys DC power/network
  and nothing else the recommended mix buys.
- **Comfort variant (≈ $21-22/mo)**: DigitalOcean Managed PG $15.15/mo (on
  Cloudflare's *documented* Hyperdrive provider list, with a first-party
  setup guide) + $6 droplet for Kratos + GreptimeCloud Hobby free. A
  "fully-managed-identity comfort+" (swap Kratos → Ory Network Production)
  lands at ≈ $85/mo.
- **Time-to-production**: sign up for Crunchy Bridge + an OVH VPS today;
  Postgres dump/restore + Hyperdrive/DSN repoint ~3-4h (staging first),
  Kratos VPS stand-up + tunnel move ~2-3h, Greptime fold-in config change
  ~1h. **~6-8 hours total**, sequenced PG → Kratos → Greptime, with the
  home lab kept warm as instant rollback for 1-2 weeks.
- **What this concretely buys over today's home lab**: vendor daily backups
  + PITR on the one store that holds user identities (there is currently no
  vendor safety net at all), DC-grade power/network/uptime instead of one
  residential ISP + no generator, and the home-lab box demoted to
  dev/staging — the same demotion `production-ha-plan.md` already argued
  node 1 deserves, arriving early.

## 1. PostgreSQL — the critical one

Both Kratos (DSN) and dovecote (Hyperdrive binding) depend on this. Hard
requirements: generic Postgres wire protocol over TLS reachable from
Cloudflare Hyperdrive (Hyperdrive speaks ordinary Postgres wire over a TCP
socket — its per-provider docs are recipes, not an allowlist
[[6]](#sources)); **direct** connection semantics, not a provider's
transaction-mode pooler stacked under Hyperdrive's own pooling (Cloudflare's
own guidance for pooled providers, and a prerequisite for
`docs/design/tenancy-isolation.md`'s session-`SET` RLS design); always-on
friendliness (the `*/5 * * * *` alert cron in prod **and** staging plus live
telemetry writes defeat scale-to-zero — the consolidation doc's core
finding); US-East for Worker/Hyperdrive proximity; real backups/PITR.

### Contenders, sized to this footprint (tiny data, 24/7-active)

| Option | $/mo at our size | Always-on model | Backups / PITR | Hyperdrive fit | Verdict |
|---|---|---|---|---|---|
| **Crunchy Bridge Hobby-0** | **≈ $10** ($9 machine, 2 cores/0.5GB + $0.10/GB-mo storage, hourly billed) [[7]](#sources) | Dedicated instance, no compute metering | Daily backups + **continuous WAL archiving, restore to any point in time — included** [[7]](#sources) | Generic PG + TLS; direct connection (built-in pgBouncer exists but is optional — connect direct) | **Top pick** — cheapest real PITR surveyed, from a Postgres-specialist shop, AWS us-east-1 |
| **DigitalOcean Managed PG** | **$15.15** (1GB/1vCPU/10GiB; +$0.215/GiB extra storage) [[8]](#sources) | Dedicated instance | Daily backups, 7-day retention, WAL maintained for point-in-time restore within the window [[9]](#sources) | **On Cloudflare's documented provider list with a setup guide** [[6]](#sources) | Close second — $5/mo more buys 2× RAM and the lowest-integration-risk path |
| Neon Launch | ≈ **$20** (0.25 CU always-on = 180 CU-hr × $0.106 ≈ $19.1 compute + $0.35/GB storage + $0.20/GB-mo restore history) [[1]](#sources) | Metered CU-hours; scale-to-zero *can* be disabled on Launch [[1]](#sources) | Instant-restore history up to 7 days (Launch) [[1]](#sources) | Documented provider [[6]](#sources) | Viable, but answering the task's question directly: **no — Launch's metered pricing does not beat flat-rate rivals for an always-on ~0.25 CU**; you pay a serverless premium for a workload that never idles |
| Supabase Pro | ≈ **$29** ($25/mo incl. $10 compute credit covering Micro, + IPv4 add-on ~$4/mo) [[2]](#sources), [[3]](#sources) | Dedicated instance | Daily backups, 7 days; PITR is a **$100/mo** add-on [[2]](#sources) | Documented provider; **the consolidation doc's IPv4 blocker is solved at Pro** — the IPv4 add-on is available Pro-and-up and attaches a v4 address to the Direct connection [[3]](#sources) | Works now, but dominated: $29 buys auth/storage/functions this stack already has (Kratos, R2, Workers) |
| AWS RDS t4g.micro | ≈ **$15-20** ($11.68 instance + gp3 $0.115/GB + backup storage + **$0.09/GB egress**) [[10]](#sources) | Dedicated instance | Automated backups + PITR included | Documented provider [[6]](#sources) | Not recommended — the egress meter is the only one in this table (everyone else: $0), and AWS account/IAM/VPC ops overhead is the highest here for zero advantage at this scale. (12-month free tier exists if you want a throwaway experiment.) |
| Fly.io Managed Postgres | **$38+** (Basic, shared-2x/1GB + $0.28/GB-mo storage) [[11]](#sources) | Dedicated | "Automatic backups and recovery"; security patches/version upgrades still "under development" [[11]](#sources) | Documented provider [[6]](#sources) | Honest read: newest managed offering of the lot, priciest of the lot, patching story not finished — no |
| Render PG | ≈ $7-20 (cheapest paid instance ~$7 + storage; PITR advertised on paid instances) [[13]](#sources) | Dedicated | Daily + PITR on paid (per third-party comparisons — **not first-party-verified this pass**) | Generic PG | Plausible sleeper; left unranked because its exact tier/PITR pairing wasn't verified first-party in this pass |
| Railway PG | ~$10-40 usage-based [[13]](#sources) | Usage-billed | Volume snapshots, not real PITR | Generic PG | No — least predictable bill of the table, weakest backup story |
| Vultr Managed PG | Unverified this pass (announcement page only; catalog pricing didn't surface) [[14]](#sources) | — | — | — | Not rankable on evidence gathered; DO occupies the same niche with verified numbers |
| Hetzner | n/a | — | — | — | Hetzner has no managed database product, and its US cloud line was repriced sharply upward in June 2026 (`production-ha-plan.md` [[7]](./production-ha-plan.md#sources)) |
| **Baseline: one small VPS running PG 18** | **$8.50** (OVH VPS-2, 4 vCPU/8GB, Vint Hill VA) [[12]](#sources) | Always-on | **Self-managed** — pgBackRest/`pg_dump` to R2 on a cron you write and test yourself | Generic PG behind a Cloudflare Tunnel (today's pattern) or direct TLS | The honest yardstick: cheapest, but the entire point of this doc is paying ~$1.50-7/mo more to make backups/PITR someone's SLA instead of your discipline |

### Recommendation: Crunchy Bridge Hobby-0, DigitalOcean as the safe swap

Crunchy wins on the merits: cheapest verified option with *continuous* PITR
(DO's is bounded to a 7-day window; Supabase's costs $100/mo; a VPS's is
whatever you build), Postgres-specialist operator, us-east-1, hourly billing
with no metering games. The one thing DO has over it is being on Cloudflare's
documented Hyperdrive provider list with a first-party guide — Crunchy is
generic-PG-over-TLS, which is exactly what Hyperdrive consumes, but if the
first connection attempt turns up any friction, the fallback is "create a DO
cluster instead and lose nothing but $5/mo and some PITR depth." Either
choice clears the bar this doc exists to clear.

Two sizing notes, both cheap to handle up front:

- **Connection budget**: small managed tiers have small connection limits,
  and this stack brings exactly two clients — Hyperdrive (which pools
  aggressively on its own) and Kratos. Dev's DSN carries
  `max_conns=20&max_idle_conns=4` (`infra/docker-compose.yml`); tune that
  down to ~5 for the managed target — prod Kratos traffic is nowhere near 20
  connections, and on a 0.5-1GB instance those idle slots are real memory.
- **RAM**: 0.5GB (Crunchy) is genuinely enough for this footprint — the
  whole dataset is megabytes and the telemetry-history table is the only
  thing that grows. If the fold-in (§3) ever makes history writes heavy
  enough to notice, Crunchy's next tiers are a slider, not a migration.

## 2. Kratos — Ory Network vs. a $5 VPS

Identity is the one service where "managed by the vendor" has outsized
value: the vendor who wrote Kratos also patches it, and an auth CVE window
on a self-hosted box is the scariest single risk in this stack. That's why
Ory Network got a full evaluation rather than a reflexive "self-host is
cheaper." The pricing reality, though ([[4]](#sources), accessed
2026-07-26):

| Ory Network plan | Price | Production-usable? |
|---|---|---|
| Developer (free) | $0 | **No** — 2 development environments only, no production environment, no custom domain |
| **Production** | **$770/yr ≈ $64/mo**, incl. a $21/mo usage credit (≈150 aDAU at $0.14/aDAU/mo overage) | Yes — 1 custom domain, 1 production + 3 staging environments; no SLA at this tier |
| Growth | $9,350/yr ≈ $779/mo | B2B features, still no SLA |
| Enterprise | Custom | 99.99% SLA lives here |

**Migration path, if/when taken, is genuinely good** — worth recording so
the revisit-at-revenue decision is cheap:

- **Identity export/import works, hashes included**: Ory's admin API
  supports bulk identity import (`PATCH /admin/identities`) carrying
  existing password hashes — bcrypt and argon2 (Kratos's own formats) are
  both supported, so users would not be forced through password resets
  [[15]](#sources).
- **`fancier`'s flows keep working**: Ory Network *is* hosted Kratos —
  same self-service flow API that `ory-kratos-client-wasm` +
  `ory_form_builder` already speak, served from a custom domain
  (`auth.pidgeiot.com`-style) so the session cookie stays first-party on
  the same cookie domain as today.
- **Branded emails survive**: custom email gateway/templates are supported
  (even the free tier lists a custom email gateway), so the existing
  branded self-service emails + useSend SMTP carry over.

**Verdict: self-host, co-located on a small VPS — Ory Network loses on
price alone, not on capability.** $64/mo is 13× the VPS and roughly 4× the
entire recommended mix, at pre-revenue. The honest counterweight — vendor
security patching — is mitigated meaningfully by the move this doc already
makes: with Kratos's DSN pointed at managed Postgres, the Kratos VPS holds
**zero state**, so "patch Kratos" becomes "rebuild a disposable box from a
script," and a compromise of the box does not directly expose the identity
store's backups. Concretely:

- **Where**: OVH VPS-1 (2 vCPU/4GB, Vint Hill VA, $4.54/mo)
  [[12]](#sources) — same US-East corridor as everything else; a $6 DO
  droplet (NYC) is an equally fine answer if consolidating billing with a
  DO Postgres pick. VPS-2 ($8.50) only if co-locating PG too (budget-min
  variant).
- **What changes**: Kratos's `dsn` → the managed PG cluster's `kratos`
  database; `kratos migrate sql -e --yes` once against the new target (the
  same command dev's `kratos-migrate` service already runs); SMTP
  (useSend) unchanged; cookie/CORS domain unchanged — the existing auth
  hostname just gets served by a `cloudflared` tunnel from the new VPS
  instead of the home lab. Zero `fancier` changes.
- **Ory Network trigger to revisit**: real revenue, or the first
  security-sensitive customer conversation — at that point $64/mo buys
  vendor-patched identity and the migration path above is already mapped.

## 3. GreptimeDB — fold it into Postgres (again, and more so)

Ranked, per the task's three options:

1. **Fold into Postgres entirely — recommended.**
   `postgres-consolidation.md` §3 already made this case at length
   (`pigeon_telemetry_history` + the existing `write_telemetry_default`
   fallback serve today's exact read shapes; a retention `DELETE` in the
   existing 5-minute scheduled handler replaces the 90-day TTL;
   user-configured `telemetry_endpoint` line-protocol forwarding is
   untouched). Every argument strengthens in this doc's context: with PG
   managed, folding in doesn't just consolidate two self-hosted services —
   it removes a service from needing *any* host, tunnel, Access policy, or
   secret set. Config-level today (unset `GREPTIMEDB_ENDPOINT` and the
   fallback path takes over); the fuller dead-code removal is the
   consolidation doc's §4 item 4, on its own schedule.
2. **GreptimeCloud Hobby (free) — genuinely covers this workload, ranked
   second only because option 1 removes the service instead of rehoming
   it.** Hobby: up to 3 services/team, 40 RCU/s + 20 WCU/s per service, 5GB
   storage, 3-month retention [[5]](#sources) — the retention even matches
   the 90-day TTL `init-greptime.sh` already picked, and prod + staging fit
   inside "3 services" as separate databases/services. Hobby-scale
   telemetry (a handful of pigeons at minutes-cadence) sits far under those
   rate caps. Compat: dovecote already sends `Authorization: Token <...>`
   (`helpers/greptime.rs`), which is GreptimeDB's own HTTP auth format —
   the switch is `GREPTIMEDB_ENDPOINT` → the GreptimeCloud host,
   `GREPTIMEDB_AUTH_TOKEN` → the service credential, and **deleting** the
   two `GREPTIMEDB_ACCESS_CLIENT_*` secrets (no more Cloudflare Access in
   front). Verify the token format against the live service before
   committing (their `<user>:<password>` token form vs. our stored single
   value), and note the paid cliff is steep and enterprise-shaped (managed
   plans "from $290/mo" [[5]](#sources)) — fine for a free tier, not a
   growth path. This is the right pick only if keeping Greptime semantics
   (or task #35's per-flock Greptime isolation design) turns out to matter
   sooner than expected.
3. **Small VPS self-host (LXC pattern ported) — not recommended.** ~$5-9/mo
   to keep operating a service whose entire current function Postgres
   already duplicates, plus the tunnel/Access/secret surface the other two
   options delete. No case for it at this scale.

## 4. Cross-cutting: cutover mechanics, sequencing, totals

### Networking / secret changes

Managed services replace the Cloudflare-Tunnel pattern with authed TLS
endpoints; the tunnel pattern survives only where something self-hosted
remains (the Kratos VPS).

| Item | Change |
|---|---|
| Hyperdrive configs (prod + staging — staging shares the prod mirror per `wrangler.toml`) | Update origin connection string to the managed cluster (`wrangler hyperdrive update`, or new config `id`s in all `[[hyperdrive]]` blocks). Direct TLS — no tunnel in the DB path anymore |
| Kratos DSN | → managed cluster's `kratos` DB (with `max_conns` tuned down, §1); run `kratos migrate sql` once |
| Kratos tunnel | `cloudflared` moves to the new VPS, same hostname — cookie domain and `fancier` config untouched |
| `GREPTIMEDB_ENDPOINT` / `GREPTIMEDB_DB` vars | Removed (fold-in) — or repointed to GreptimeCloud in the §3.2 variant |
| `GREPTIMEDB_AUTH_TOKEN`, `GREPTIMEDB_ACCESS_CLIENT_ID/SECRET` secrets | Deleted (`wrangler secret delete`, both envs) — GreptimeCloud variant keeps `AUTH_TOKEN` only |
| `telemetry.pidgeiot.com` tunnel + Access policy, DB tunnel | Retired |
| Home-lab box | Demoted to dev/staging + rollback target; nothing production points at it after the grace period |

**Latency**: everything lands US-East (Crunchy us-east-1 / DO NYC / OVH
Vint Hill) — inside the same corridor `production-ha-plan.md` already
established as fine for Worker/Hyperdrive proximity; Hyperdrive's edge
pooling + query caching absorbs the rest.

### Sequencing, effort, rollback

1. **Postgres first (~3-4h)** — both other services depend on it. Create
   cluster, run `infra/init-db.sql` + create the `kratos` database,
   dump/restore from the home lab (the consolidation doc's §2.2 steps apply
   verbatim, including `ysql_dump` from Yugabyte and its §2.3 verification
   checklist — run staging against the new cluster first). Repoint
   Hyperdrive, `wrangler deploy` both envs. The dual-persistence model makes
   the window forgiving: DOs stay authoritative throughout; a mirror-sync
   failure during cutover is logged and skipped by design.
2. **Kratos next (~2-3h)** — provision VPS, install Kratos + `cloudflared`,
   DSN → step 1's cluster, migrate, move the tunnel hostname, run the
   login/registration/recovery smoke tests.
3. **Greptime fold-in last (~1h config-level)** — remove vars/secrets, let
   the PG fallback take the default write path; schedule the dead-code
   removal + retention-DELETE commit separately (consolidation doc §4.4).

**Rollback** at every step is the consolidation doc's §2.4 mechanism in
reverse: the home-lab instances keep running untouched for 1-2 weeks;
reverting is repointing the Hyperdrive config / DSN / vars back and
redeploying. Take a final `pg_dump` of the managed cluster before ever
decommissioning anything.

### Monthly totals

| Variant | Postgres | Kratos | Greptime | Total/mo |
|---|---|---|---|---|
| **Recommended** | Crunchy Bridge Hobby-0 ≈ $10 | OVH VPS-1 $4.54 | Folded, $0 | **≈ $15** |
| Budget-min | Self-hosted PG 18 on the same VPS | OVH VPS-2 $8.50 (shared box) | Folded, $0 | **≈ $8.50** — DC power/network only; backups/PITR back on you |
| Comfort | DO Managed PG $15.15 | DO droplet $6 | GreptimeCloud Hobby $0 | **≈ $21** |
| Comfort+ (managed identity) | DO Managed PG $15.15 | Ory Network Production ≈ $64 | GreptimeCloud Hobby $0 | **≈ $79-85** |

All variants sit an order of magnitude under the shelved HA plan's
$185-215/mo, which stays the number that real revenue reactivates.

### What would change this recommendation

- **Hyperdrive-to-Crunchy friction on first contact** → swap to
  DigitalOcean ($15.15), the documented-provider path; nothing else in the
  plan moves.
- **Render's ~$7 tier verifying first-party as PITR-included** → it would
  undercut Crunchy as top PG pick; worth 15 minutes of checking before
  signup day.
- **Revenue / first security-conscious customer** → flip Kratos to Ory
  Network Production ($64/mo) using §2's mapped migration path, and start
  the `production-ha-plan.md` conversation.
- **Telemetry volume outgrowing a 0.5-1GB PG instance** (observed, not
  hypothesized) → GreptimeCloud Hobby is the free pressure valve before
  any paid step.

## Sources

1. Neon paid plans (Launch $0.106/CU-hr, no monthly minimum, $0.35/GB-mo
   storage, restore history up to 7 days at $0.20/GB-mo, scale-to-zero
   configurable/disable-able) —
   [Neon plans — Neon Docs](https://neon.com/docs/introduction/plans)
   (accessed 2026-07-26); always-on CU math per
   [`postgres-consolidation.md`](./postgres-consolidation.md) §1.1
   (accessed 2026-07-23)
2. Supabase Pro pricing ($25/mo incl. $10 compute credit ≈ Micro; daily
   backups 7 days; PITR $100/mo add-on; spend caps) —
   [Supabase Pricing](https://supabase.com/pricing) (accessed 2026-07-26)
3. Supabase IPv4 add-on availability (Pro+, attaches IPv4 to the Direct
   connection; Supavisor pooler is IPv4 by default) —
   [Dedicated IPv4 Address — Supabase Docs](https://supabase.com/docs/guides/platform/ipv4-address)
   (accessed 2026-07-26); ~$0.0055/hr ≈ $4/mo price per
   [`postgres-consolidation.md`](./postgres-consolidation.md) [[7]]
   (accessed 2026-07-23)
4. Ory Network plans (Developer free: 2 dev envs, no production env / no
   custom domain; Production $770/yr with $21/mo credit, $0.14/aDAU/mo
   overage, 1 custom domain; Growth $9,350/yr; SLA only at Enterprise) —
   [Ory Pricing](https://www.ory.com/pricing) (accessed 2026-07-26)
5. GreptimeCloud Hobby plan (3 services/team, 40 RCU/s + 20 WCU/s + 5GB per
   service, 3-month retention) and managed pricing floor ("from $290/mo") —
   [Hobby Plan — GreptimeDB Docs](https://docs.greptime.com/greptimecloud/usage-&-billing/hobby/),
   [Greptime Pricing](https://greptime.com/pricing) (accessed 2026-07-26)
6. Cloudflare Hyperdrive Postgres provider docs (15 documented providers
   incl. Neon, Supabase, DigitalOcean, AWS RDS/Aurora, Fly; generic
   Postgres-wire support) —
   [Postgres database providers — Hyperdrive Docs](https://developers.cloudflare.com/hyperdrive/examples/connect-to-postgres/postgres-database-providers/)
   (accessed 2026-07-26)
7. Crunchy Bridge pricing (Hobby-0 $9/mo, 2 cores/0.5GB; $0.10/GB storage;
   hourly billing; no-cost daily backups + continuous WAL archiving /
   point-in-time recovery) —
   [Crunchy Data Pricing](https://www.crunchydata.com/pricing)
   (accessed 2026-07-26)
8. DigitalOcean Managed PostgreSQL pricing (cheapest $15.15/mo,
   1GiB/1vCPU/10-30GiB, $0.215/GiB-mo additional storage) —
   [DigitalOcean Managed Databases Pricing](https://www.digitalocean.com/pricing/managed-databases)
   (accessed 2026-07-26)
9. DigitalOcean Managed PG backup/PITR behavior (daily backups, 7-day fixed
   retention, WAL maintained for point-in-time restore within the window) —
   [Worry-Free Managed PostgreSQL Hosting — DigitalOcean](https://www.digitalocean.com/products/managed-databases-postgresql),
   [DigitalOcean managed DB retention — SimpleBackups](https://simplebackups.com/blog/digitalocean-managed-db-retention-expired)
   (accessed 2026-07-26)
10. AWS RDS db.t4g.micro pricing ($0.016/hr ≈ $11.68/mo single-AZ; gp3
    $0.115/GB-mo; $0.09/GB egress; 12-month free tier) —
    [AWS RDS PostgreSQL Pricing 2026 — InfraTally](https://infratally.com/articles/aws-rds-pricing-explained-2026/),
    [AWS RDS Cost 2026 — selfhost.dev](https://selfhost.dev/blog/aws-rds-cost-breakdown-2026/)
    (accessed 2026-07-26)
11. Fly.io Managed Postgres (Basic $38/mo shared-2x/1GB, $0.28/GB-mo
    storage, patches/upgrades "under development") —
    [Fly.io Managed Postgres Docs](https://fly.io/docs/mpg/)
    (accessed 2026-07-26)
12. OVH VPS lineup incl. Vint Hill VA availability (VPS-1 2 vCPU/4GB
    $4.54/mo; VPS-2 4 vCPU/8GB $8.50/mo) — per
    [`production-ha-plan.md`](./production-ha-plan.md) [[24]],
    [OVHcloud VPS](https://us.ovhcloud.com/vps/) (accessed 2026-07-23)
13. Render/Railway Postgres pricing shape (Render cheapest paid ~$7/mo with
    PITR advertised on paid instances; Railway usage-based ~$10-40/mo) —
    third-party comparisons, not first-party-verified:
    [Railway vs Render 2026 — selfhost.dev](https://selfhost.dev/blog/railway-vs-render-pricing-2026-verdict/),
    [Railway Pricing 2026 — srvrlss.io](https://www.srvrlss.io/provider/railway/)
    (accessed 2026-07-26)
14. Vultr Managed PostgreSQL (existence confirmed; current catalog pricing
    did not surface in this pass) —
    [Managed PostgreSQL now available at Vultr — Vultr Blog](https://blogs.vultr.com/Managed-PostgreSQL-now-available-at-Vultr)
    (accessed 2026-07-26)
15. Ory identity bulk import with existing password hashes (bcrypt, argon2,
    PBKDF2, scrypt, et al.; webhook-based gradual migration alternative) —
    [Import user accounts / identities — Ory Docs](https://www.ory.com/docs/kratos/manage-identities/import-user-accounts-identities)
    (accessed 2026-07-26)
