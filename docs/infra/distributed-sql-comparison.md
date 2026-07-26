# Distributed SQL comparison: is YugabyteDB still the right pick for the
# HA plan's database slot?

Researched 2026-07-26. This doc feeds
[`production-ha-plan.md`](./production-ha-plan.md) — the shelved 3-node RF3
plan whose database slot currently says "YugabyteDB." It does **not**
contradict [`postgres-consolidation.md`](./postgres-consolidation.md)'s
near-term direction (collapse single-node Yugabyte to plain PostgreSQL 18,
$0/mo, pre-revenue) — it assumes that consolidation happens, and answers
the question that world creates: **when the HA moment arrives, what do we
scale plain Postgres INTO?** Pricing/versions/licenses are cited inline
and will drift — treat every claim as "as of this date," not permanent.

## TL;DR

- **Verdict: no — YugabyteDB is no longer the best occupant of the HA
  plan's database slot for this specific case (3 nodes, solo operator,
  "survive one node failure," boxes shared with GreptimeDB + Kratos).
  Swap in: plain PostgreSQL + Patroni (etcd colocated ×3) with one
  synchronous standby, fronted by HAProxy routing on Patroni's REST
  health endpoints.** The HA plan's actual requirement is failover, not
  horizontal write scaling — and at that requirement, Postgres-native HA
  beats every true distributed SQL engine surveyed on compatibility
  (it *is* Postgres — zero dialect risk for Kratos, Hyperdrive, or our
  schema), resource footprint (~1-2 cores vs. Yugabyte's documented
  4-vCPU production floor per node), and migration cost from the
  consolidation posture (add replicas to the running instance; no
  dump/restore, no dialect re-audit).
- **The Kratos finding is close to dispositive on its own.** Ory
  officially supports exactly PostgreSQL, MySQL, CockroachDB (all "fully
  supported"), and SQLite (non-production) [[1]](#sources). YugabyteDB is
  **not on that list**, and the real-world reports that exist are
  failures: Kratos's own tracker has a closed-unresolved issue where
  migrations die on an `ALTER TABLE ... ALTER COLUMN TYPE` Yugabyte
  couldn't run [[2]](#sources), with the same class of failure reported
  against Ory Hydra [[3]](#sources). Current Yugabyte (PG15 rebase) has
  since gained table-rewrite `ALTER TYPE` [[6]](#sources), so today's
  migrations *may* now pass — but "may now pass, unsupported upstream,
  re-verified by us at every Kratos upgrade" is a standing tax the
  officially-supported options don't charge.
- **CockroachDB is the only distributed SQL engine Kratos officially
  supports, and its PG compatibility now covers our actual schema**
  (row-level PL/pgSQL triggers, JSONB, partial indexes,
  `gen_random_uuid()`) — but the November 2024 license change makes it a
  worse strategic hold than Yugabyte: no open-source core anymore, free
  only under $10M annual revenue with an annual eligibility check, and
  **telemetry is mandatory on the free tier** [[7]](#sources). Its
  resource floor (4 vCPU minimum, 8 recommended, 4GiB RAM/vCPU)
  [[9]](#sources) is also no lighter than Yugabyte's. A lateral move, not
  an upgrade.
- **TiDB is out on arrival**: MySQL wire protocol against a
  Postgres-flavored codebase (every dovecote query, `init-db.sql`, and
  Hyperdrive binding is Postgres-dialect), and its own documented
  production topology (3× PD + 3× TiKV + 2× TiDB, 8+ cores and 48-64GB+
  RAM per component) is an order of magnitude past these boxes
  [[10]](#sources). Apache 2.0 licensing is its only win here.
- **YugabyteDB itself is healthier than ever as a product** — v2025.1
  rebased YSQL from PG 11.2 to PG 15, v2025.2 LTS continues that line,
  core remains 100% Apache 2.0 [[5]](#sources)[[6]](#sources) — and it
  stays this doc's **conditional fallback**: if the requirement ever
  genuinely becomes horizontal write scaling / multi-region rather than
  single-failure survival, Yugabyte re-enters as the
  best-licensed, most-PG-compatible distributed engine. Even then, Kratos
  itself can simply stay on the Patroni Postgres cluster — nothing forces
  the identity store and the dovecote mirror onto the same engine.
- **Bonus consequence for the HA plan's budget**: the plan's
  SYS-3-over-SYS-1 verdict hinged specifically on Yugabyte's 4-core
  production floor eating a 6-core box. Postgres+Patroni's ~1-2-core
  footprint re-opens the ~$33/mo SYS-1 tier as plausibly adequate —
  roughly **$80/mo (~$960/yr) of potential savings** the ha-plan should
  re-derive if/when it comes off the shelf. Not re-litigated here; just
  flagged.

## What the slot actually has to do

Grounding, from `production-ha-plan.md` and the codebase:

- **Requirement**: survive one node failure across 3 co-located boxes
  (8c/32GB class, shared with GreptimeDB roles + Kratos), operated by one
  person. Not horizontal write scaling, not multi-region, not
  petabyte-scale.
- **Consumers**: (1) Ory Kratos — its DSN is a Postgres URL; identity
  data correctness is the crown jewels. (2) dovecote via Cloudflare
  Hyperdrive — a **generic Postgres-wire TCP client**; vendor
  cluster-aware smart drivers are impossible by construction (Hyperdrive
  *is* the driver), so any candidate is reached through a dumb
  LB/proxy either way (already established in `production-ha-plan.md`'s
  Hyperdrive section).
- **Schema** (`infra/init-db.sql` + runtime `ensure_*` DDL): JSONB
  columns, `gen_random_uuid()` PK defaults, row-level `BEFORE UPDATE`
  PL/pgSQL triggers (`trigger_set_timestamp`,
  `trigger_prevent_immutable_updates`), partial indexes
  (`WHERE pigeon_id IS NOT NULL`), `ON DELETE CASCADE`. No
  `CREATE INDEX CONCURRENTLY` anywhere (the one Yugabyte txn-block
  incident we hit lived in a hand-applied migration since deleted — see
  `postgres-consolidation.md` §2.1). Postgres is a **best-effort mirror**
  of the Durable Objects, except for the tables that are primary here
  (telemetry history, alerts, firmware catalog, Kratos's own schema).
- **Write volume**: hobby-to-small-IoT — a 5-minute cron, per-device
  telemetry at minutes-cadence, dashboard CRUD. Nothing that stresses a
  single modern Postgres core, let alone three.

## Kratos compatibility, per candidate

The task's first judging criterion, answered explicitly:

| Candidate | Ory official support | Real-world evidence |
|---|---|---|
| PostgreSQL | **Fully supported** [[1]](#sources) | Our own dev stack has run Kratos on `postgres:18-alpine` continuously (`infra/docker-compose.yml`) — zero issues |
| CockroachDB | **Fully supported** [[1]](#sources) | First-class in Ory's docs/config; the only distributed engine on the list |
| MySQL (→ TiDB) | MySQL fully supported; **TiDB itself never named** [[1]](#sources) | TiDB rides the MySQL wire but is not what Ory tests against; no positive reports found |
| YugabyteDB | **Not supported** — absent from Ory's list [[1]](#sources) | kratos#715: migrations fail on unsupported `ALTER TABLE`, closed without resolution [[2]](#sources); hydra#2156 same class [[3]](#sources). Yugabyte's PG15-era `ALTER TYPE` table-rewrite support [[6]](#sources) plausibly clears the specific statement that failed, but nobody upstream is testing it |

Honest caveat on the incumbent: this repo does not record whether
staging/prod Kratos currently runs against the single-node Yugabyte or
its own Postgres (flagged as an open question in
`postgres-consolidation.md` §2.2). If it *is* on Yugabyte and its
migrations ran clean, that's a real existence proof for the current
Kratos version — but it would still be an unsupported configuration that
every future `kratos migrate sql` run re-gambles on. The officially
supported list is the durable fact; confirm the node-1 reality before the
consolidation cutover regardless.

## The candidates

### YugabyteDB (incumbent)

- **PG parity**: v2025.1 STS rebased YSQL's PostgreSQL fork from 11.2 to
  15.0 — the largest single compatibility jump in the product's history —
  and v2025.2 LTS continues the PG15 line; upstream markets 2026.1 as
  current [[5]](#sources)[[6]](#sources)[[15]](#sources). YSQL reuses
  the actual PG query layer, so day-to-day SQL compatibility is the best
  of the distributed field. Gaps remain at the storage-integration edges:
  `ALTER COLUMN TYPE` rewrites are now supported but carry their own
  exclusion list (partitioned tables, tables with rules, CDC/xCluster
  caveats) [[6]](#sources), and DDL behaves differently in transactions
  (the "even plain `CREATE INDEX` builds online, so no index creation
  inside an explicit transaction" quirk we hit — yugabyte-db#6240,
  detailed in `postgres-consolidation.md` §2.1).
- **Licensing**: the whole database, including formerly-enterprise
  features, is Apache 2.0; only the `-managed` service binaries carry the
  Polyform Free Trial license [[5]](#sources). Best-in-field. No revenue
  thresholds, no telemetry mandate, no eligibility checks.
- **Single-node viability**: runs fine at RF1 — that's today's setup —
  but as `postgres-consolidation.md` argues, RF1 pays distributed-DB
  operational overhead for zero HA payoff.
- **RF3 resource weight**: Yugabyte's own production guidance is 3 nodes
  at 4-8 vCPU each [[4]](#sources) — the single number that drove the
  ha-plan's per-box sizing and its SYS-3-over-SYS-1 verdict. On a shared
  8-core box, Yugabyte alone budgets half the machine.
- **Kratos**: unsupported; see table above. This is the incumbent's
  biggest concrete defect for *this* stack.
- **Verdict**: a good database that answers a question this platform
  isn't asking. Keep as the fallback for a real scale-out future.

### CockroachDB

- **Kratos**: officially fully supported [[1]](#sources) — the unique
  selling point among distributed engines here.
- **PG compat vs. our actual schema — checked hard, as asked**: current
  stable supports row-level `BEFORE`/`AFTER` PL/pgSQL triggers
  (`FOR EACH ROW` mandatory; statement-level, `INSTEAD OF`,
  `UPDATE OF` column lists, and the `REFERENCING` clause are not
  supported) [[8]](#sources). **All four of our triggers are row-level
  `BEFORE UPDATE` — inside the supported subset.** UDFs and stored
  procedures in PL/pgSQL went GA in v24.1 [[11]](#sources). JSONB,
  partial indexes, `gen_random_uuid()` are long-standing. Residual
  dialect drift is real but small for us (1-based `TG_ARGV`, no
  statement-level `TRUNCATE` triggers, distributed-DDL semantics)
  [[8]](#sources). Of the true distributed engines, this would be the
  smallest migration.
- **Licensing — the disqualifier**: as of November 18, 2024 the
  open-source/BSL Core edition is retired; everything is the proprietary
  (source-available) CockroachDB Enterprise license. Self-hosting is free
  only for individuals and organizations under **$10M annual revenue**,
  via an annually-renewed, eligibility-checked free license — and on the
  free (and trial) tiers, **telemetry cannot be opted out of**
  [[7]](#sources). For a $0-target, sovereignty-minded self-host stack,
  that's three separate strings attached where Yugabyte has none — and a
  built-in future bill exactly when the platform succeeds.
- **Resource weight**: 4 vCPU minimum, 8 vCPU recommended per node, 4GiB
  RAM per vCPU, 3-node minimum [[9]](#sources) — same class as Yugabyte;
  no relief on the shared boxes.
- **Verdict**: the best distributed-SQL fit for Kratos specifically, but
  it trades Yugabyte's licensing cleanliness for official support while
  keeping the same resource bill. A lateral move that adds a landlord.
  Not recommended.

### TiDB

- **Wire/dialect**: MySQL protocol and dialect. Kratos supports MySQL
  (though Ory never names TiDB) [[1]](#sources) — but dovecote's entire
  SQL surface (`init-db.sql`, `tokio-postgres`-style queries via
  Hyperdrive's Postgres binding, JSONB idioms, PL/pgSQL triggers) is
  Postgres-flavored. This isn't a config repoint; it's a rewrite of the
  data layer plus a schema re-port.
- **Footprint**: documented minimum production topology is 3× PD + 3×
  TiKV + 2× TiDB (8 nodes of roles), with 8+ cores per component and
  48GB+ (TiDB) / 64GB+ (TiKV) RAM recommendations [[10]](#sources) —
  designed for a fleet, not for three shared hobby boxes.
- **Licensing**: Apache 2.0, genuinely open [[12]](#sources).
- **Verdict**: out. Wrong wire protocol for this codebase and the
  heaviest footprint surveyed.

### Postgres-native HA (the honest alternative) — **recommended**

The suspicion the task asked to confirm or refute: **confirmed.** At
3-node/solo-operator/failover-only scale, a streaming-replication
Postgres cluster with automated failover beats every distributed SQL
engine above on every judging criterion except "any node accepts
writes" — which this workload doesn't need.

- **Patroni (the pick)**: the de-facto standard template — one Python
  agent per Postgres node, consensus via etcd (colocatable on the same 3
  boxes), automatic leader election and failover. Actively maintained and
  current: 4.x releases through mid-2026 (4.1.4 as of May 2026, with
  releases tracking new etcd security fixes) [[13]](#sources). Each
  agent exposes a REST API (`:8008`) purpose-built for dumb-LB routing:
  `GET /primary` returns 200 only on the leader, 503 elsewhere — HAProxy
  health-checks that endpoint and always forwards port-5432 traffic to
  the current primary, shifting automatically on failover
  [[14]](#sources). That is exactly the "generic wire protocol client
  behind an LB" shape Hyperdrive already forces on us — except here the
  health endpoint is a first-class, role-aware primitive rather than a
  TCP liveness guess against a tserver.
  - With `synchronous_mode` and one synchronous standby, a failover
    loses no acknowledged writes — the guarantee that matters for
    Kratos identity data. Cost: writes stall if *both* standbys are
    down, and failover itself is a brief (seconds-to-tens-of-seconds)
    write outage. For a best-effort mirror + a 5-minute cron + human
    dashboard traffic, that's an invisible blip; the DO layer is
    authoritative for device state throughout.
  - Honest ops caveat: Patroni is powerful but "one of the most
    difficult to properly deploy and debug" among Postgres failover
    tools per EDB's own assessment, and etcd quorum loss can block
    failover or demote a primary [[16]](#sources). Three colocated etcd
    members on the same 3 boxes (sub-1-core, ~1GB each — the ha-plan
    already budgeted this line for Greptime's metasrv question) is the
    standard answer at this scale. This is real complexity — but it is
    *less* total complexity than operating a distributed SQL engine,
    which contains an equivalent consensus subsystem *plus* a
    distributed storage layer, sharding/rebalancing, and its own
    dialect drift.
- **pg_auto_failover (runner-up)**: simpler mental model (a monitor node
  instead of a DCS); alive but slower-moving — v2.2 (April 2025) added
  PG17 and dropped 11/12, with ~17-month gaps between releases
  [[17]](#sources). Its monitor is a coordination single-point (the
  cluster serves traffic if the monitor dies, but failover orchestration
  stops). Credible fallback if Patroni's etcd dependency ever feels like
  too much; not the primary pick given the slower cadence and smaller
  community.
- **Ruled out quickly**: **CloudNativePG** — excellent, but a Kubernetes
  operator; this stack is Proxmox+LXC, not k8s. **Stolon** — dormant
  (v0.17.0, no releases or images in years) [[18]](#sources).
  **Spilo/Autobase** — packaging/automation *around* Patroni, worth a
  look at deploy time, not separate candidates. **pgEdge** — went fully
  open source under the PostgreSQL License in September 2025 (Spock
  multi-master extension included) [[19]](#sources); interesting
  velocity, but async multi-master conflict resolution is the wrong
  complexity class for a single-writer workload — nothing here needs
  active-active.
- **Resource footprint**: Postgres at this write volume idles in ~1 core
  / 2-4GB; Patroni agent + etcd member add well under a core combined.
  Per shared box, the database slot drops from Yugabyte's 4-core floor
  to ~1.5-2 cores — the single biggest headroom win available to the
  ha-plan, and (flagged in the TL;DR) potentially a hardware-tier win
  too.
- **Compatibility**: total, by identity. Kratos: fully supported
  [[1]](#sources). Our schema: already running on plain PG continuously
  in dev. Triggers, JSONB, partial indexes, `CONCURRENTLY` semantics:
  vanilla. Hyperdrive: the exact configuration Cloudflare documents.
  Zero dialect risk is not a small criterion at solo-operator scale —
  every compat quirk is a page *you* get.

## Judging matrix

| Criterion | Yugabyte RF3 | CockroachDB | TiDB | **PG + Patroni** |
|---|---|---|---|---|
| Kratos official support | No [[1]](#sources) | **Yes** | Via MySQL only, untested | **Yes (first-class)** |
| Full PG compat for our schema | High (PG15 fork; DDL edge cases) | Good-with-asterisks (dialect drift) | No (MySQL) | **Total** |
| Hyperdrive/generic-wire + LB | OK (dumb LB over tservers) | OK (dumb LB) | OK (MySQL binding, code rewrite) | **Best (role-aware `/primary` health endpoint)** [[14]](#sources) |
| Ops weight, solo @ 3 nodes | High (distributed store + consensus + rebalancing) | High | Highest | **Moderate (Patroni+etcd, canonical pattern)** |
| Footprint on shared 8c/32GB | 4-8 vCPU/node floor [[4]](#sources) | 4-8 vCPU/node, 4GiB/vCPU [[9]](#sources) | 8+ cores, 48-64GB+/role [[10]](#sources) | **~1.5-2 cores/node** |
| License / $0 self-host | **Apache 2.0, clean** [[5]](#sources) | Proprietary; free <$10M rev, mandatory telemetry [[7]](#sources) | Apache 2.0 [[12]](#sources) | **PostgreSQL license, clean** |
| Community / bus factor | Single vendor, healthy | Single vendor, license risk shown | Single vendor + CNCF-adjacent | **Broadest in databases; Patroni multi-vendor** |
| Migration from today (via consolidation) | Re-migrate back out of PG; re-audit quirks | Dump/restore + dialect re-audit | Full SQL-layer rewrite | **None — grow the running instance into a cluster** |

## Migration-path asymmetry (why consolidation-first locks this in)

- **Interim plain PG → Patroni cluster**: install Patroni around the
  existing instance, `pg_basebackup` two replicas onto the new boxes,
  wire etcd + HAProxy, point Hyperdrive/Kratos at the routed endpoint.
  Same binaries, same data directory format, no dump/restore, no dialect
  changes, rollback = turn Patroni off. This is the "what do we scale PG
  into" answer the consolidation doc's framing asks for.
- **Interim plain PG → Yugabyte RF3** (the current ha-plan text):
  a second full migration (dump/restore), re-accepting the DDL quirks we
  just escaped, re-gambling Kratos's unsupported status — paying the
  consolidation migration *backwards* plus the distributed tax.
- **Skipping consolidation and going single-node-Yugabyte → RF3
  directly**: the smoothest path *for Yugabyte* (add nodes, raise RF) —
  but it keeps the unsupported-Kratos posture and 4-core floor forever,
  and the consolidation decision has already been made on its own
  merits.

## What would change this answer

- **A real horizontal write-scaling or multi-region requirement** —
  observed, not hypothesized (the same discipline the sibling docs
  apply): telemetry-history write volume an in-box Postgres primary
  can't sustain, or a contractual multi-region latency/residency need.
  Then Yugabyte re-enters as the pick for the *dovecote mirror* slot
  (Apache 2.0 + PG15 fork beats CockroachDB's licensing for a self-host
  stack), while **Kratos stays on the Patroni Postgres cluster** — the
  two consumers were never obligated to share an engine.
- **Ory adding YugabyteDB to its supported list** (worth a glance at
  [[1]](#sources) whenever the HA plan is revived) — that would remove
  Yugabyte's biggest concrete defect here, though not the footprint gap.
- **CockroachDB re-licensing** toward something with no revenue
  threshold/telemetry mandate — would make it a genuine contender given
  its Kratos support; the 2024 direction of travel was the opposite way
  [[7]](#sources).
- **Kubernetes adoption on these boxes** (not currently planned) — would
  put CloudNativePG ahead of raw Patroni as the delivery mechanism for
  the same Postgres-native recommendation.

## Sources

1. Ory self-hosted deployment — officially supported databases
   (PostgreSQL, MySQL, CockroachDB "fully supported"; SQLite
   non-production; no YugabyteDB) —
   [Deployment — Ory docs](https://www.ory.com/docs/self-hosted/deployment)
   (accessed 2026-07-26)
2. Kratos-on-YugabyteDB migration failure (`ALTER TABLE
   "courier_messages" ALTER COLUMN "body" TYPE text, ... SET NOT NULL` →
   "This ALTER TABLE command is not yet supported"); closed without
   resolution —
   [Make it compatible with Yugabyte database — ory/kratos#715](https://github.com/ory/kratos/issues/715)
   (accessed 2026-07-26)
3. Same failure class against Ory Hydra —
   [Migrations fail for YugabyteDB — ory/hydra#2156](https://github.com/ory/hydra/issues/2156)
   (accessed 2026-07-26)
4. YugabyteDB production cluster guidance (3 nodes, 4-8 vCPU/node) —
   [Plan your cluster — YugabyteDB Docs](https://docs.yugabyte.com/stable/yugabyte-cloud/cloud-basics/create-clusters-overview/)
   (accessed 2026-07-26; same source as `production-ha-plan.md` cite 21)
5. YugabyteDB licensing — 100% Apache 2.0 including former enterprise
   features; `-managed` binaries under Polyform Free Trial —
   [yugabyte-db LICENSE.md](https://github.com/yugabyte/yugabyte-db/blob/master/LICENSE.md),
   [Why We Changed YugabyteDB Licensing to 100% Open Source](https://www.yugabyte.com/blog/why-we-changed-yugabyte-db-licensing-to-100-open-source/)
   (accessed 2026-07-26)
6. YugabyteDB v2025.1/v2025.2 — PG fork rebase 11.2 → 15.0; current
   `ALTER TABLE` support incl. table-rewrite `ALTER COLUMN TYPE` and its
   exclusions —
   [What's new in v2025.1 STS](https://docs.yugabyte.com/stable/releases/ybdb-releases/v2025.1/),
   [What's new in v2025.2 LTS](https://docs.yugabyte.com/stable/releases/ybdb-releases/v2025.2/),
   [ALTER TABLE — YSQL docs](https://docs.yugabyte.com/stable/api/ysql/the-sql-language/statements/ddl_alter_table/)
   (accessed 2026-07-26)
7. CockroachDB November 2024 license change — Core retired, single
   proprietary Enterprise license, free <$10M annual revenue with annual
   eligibility renewal, mandatory telemetry on free/trial tiers —
   [CockroachDB retires self-hosted Core offering — SD Times](https://sdtimes.com/os/cockroachdb-retires-self-hosted-core-offering-makes-enterprise-version-free-for-companies-under-10m-in-annual-revenue/),
   [Concerns Rise in Open-Source Community — InfoQ](https://www.infoq.com/news/2024/09/cockroachdb-license-concerns/),
   [Licensing FAQs — CockroachDB docs](https://www.cockroachlabs.com/docs/stable/licensing-faqs)
   (accessed 2026-07-26)
8. CockroachDB trigger support and limitations (row-level BEFORE/AFTER
   only; no statement-level/`INSTEAD OF`/`UPDATE OF`/`REFERENCING`;
   1-based `TG_ARGV`) —
   [Triggers — CockroachDB docs](https://www.cockroachlabs.com/docs/stable/triggers),
   [PostgreSQL Compatibility — CockroachDB docs](https://www.cockroachlabs.com/docs/stable/postgresql-compatibility)
   (accessed 2026-07-26)
9. CockroachDB production hardware guidance (4 vCPU minimum / 8
   recommended per node, 4GiB RAM per vCPU, 3-node minimum) —
   [Production Checklist — CockroachDB docs](https://www.cockroachlabs.com/docs/stable/recommended-production-settings)
   (accessed 2026-07-26)
10. TiDB minimum production topology (3× PD, 3× TiKV, 2× TiDB) and
    hardware recommendations (8+ cores; 48GB+/64GB+ RAM) —
    [TiDB Software and Hardware Requirements](https://docs.pingcap.com/tidb/stable/hardware-and-software-requirements/),
    [TiDB Deployment FAQs](https://docs.pingcap.com/tidb/stable/deploy-and-maintain-faq/)
    (accessed 2026-07-26)
11. CockroachDB stored procedures + PL/pgSQL UDFs GA in v24.1 —
    [Introducing CockroachDB 24.1](https://www.cockroachlabs.com/blog/introducing-cockroachdb-24-1-fully-managed-enterprise-database-experience/),
    [Stored Procedures — CockroachDB docs](https://www.cockroachlabs.com/docs/stable/stored-procedures)
    (accessed 2026-07-26)
12. TiDB licensing (Apache 2.0, all features) —
    [pingcap/tidb LICENSE](https://github.com/pingcap/tidb/blob/master/LICENSE)
    (accessed 2026-07-26)
13. Patroni current releases (4.1.x through mid-2026; etcd
    security-fix tracking) —
    [Release notes — Patroni docs](https://patroni.readthedocs.io/en/latest/releases.html),
    [patroni — PyPI](https://pypi.org/project/patroni/)
    (accessed 2026-07-26)
14. Patroni REST API role-aware health endpoints (`/primary` 200 on
    leader only) and the canonical HAProxy routing pattern —
    [Patroni REST API docs](https://patroni.readthedocs.io/en/latest/rest_api.html),
    [HAProxy-Patroni Setup Using Health Check Endpoints — Percona](https://www.percona.com/blog/haproxy-patroni-setup-using-health-check-endpoints-and-debugging/)
    (accessed 2026-07-26)
15. YugabyteDB 2026.1 marketed as latest release —
    [Latest release — yugabyte.com](https://www.yugabyte.com/latest-release/)
    (accessed 2026-07-26)
16. Patroni ops-difficulty and etcd-quorum caveats —
    [The Do's and Don'ts of Postgres High Availability Part 3 — EDB](https://www.enterprisedb.com/blog/dos-donts-postgres-high-availability-pt-3-tools-rules),
    [PostgreSQL HA: Patroni, Replication and Failover Patterns — DEV](https://dev.to/philip_mcclarence_2ef9475/postgresql-high-availability-patroni-replication-and-failover-patterns-4f6k)
    (accessed 2026-07-26)
17. pg_auto_failover status (v2.2 April 2025 — PG17 support, drops
    11/12; maintained by the Citus team, slower cadence) —
    [pg_auto_failover releases — GitHub](https://github.com/hapostgres/pg_auto_failover/releases),
    [pg_auto_failover documentation](https://pg-auto-failover.readthedocs.io/en/main/)
    (accessed 2026-07-26)
18. Stolon dormancy (v0.17.0; no recent releases/images) —
    [sorintlab/stolon — GitHub](https://github.com/sorintlab/stolon)
    (accessed 2026-07-26)
19. pgEdge full open-sourcing under the PostgreSQL License (September
    2025; Spock/Snowflake/lolor re-licensed) —
    [pgEdge Goes Open Source Under the Postgres License](https://www.pgedge.com/blog/pgedge-goes-open-source),
    [pgEdge Announces pgEdge Enterprise Postgres — PR Newswire](https://www.prnewswire.com/news-releases/pgedge-announces-pgedge-enterprise-postgres-alongside-full-commitment-to-open-source-302552043.html)
    (accessed 2026-07-26)
