# Second Proxmox node — hosting recommendation

> **For the real 3-node HA case (YugabyteDB RF3 + GreptimeDB cluster +
> Kratos, co-located on 3 US-East boxes near Springfield, MA), see
> [`production-ha-plan.md`](./production-ha-plan.md) instead** — it
> supersedes this doc's recommendation for that scenario (notably, the
> Hetzner Server Auction pick below is EU-only and doesn't apply to a
> US-East requirement). This doc's RF2-vs-RF3 quorum math and GreptimeDB
> clustering-architecture explainer are still accurate and referenced from
> the new doc rather than repeated there.

Researched 2026-07-22. Pricing below is cited inline and will drift — treat
numbers as "roughly this, as of this date," not a quote.

## TL;DR

- **Top pick: a Hetzner Server Auction ("Server Börse") dedicated box.** Real
  bare metal (no nested-virtualization question at all — it's the same
  situation as node 1 today), cheapest hardware-per-euro of anything
  surveyed, and small/mid boxes (4-8 cores, 32-64 GB RAM) go for roughly
  €30-50/mo — several times the headroom the current GreptimeDB LXC actually
  uses (2 cores / 2 GB RAM, `infra/proxmox-greptimedb-lxc.sh:46-47`).
- **Runner-up: Contabo VDS** (not their base "Cloud VPS" — see the
  nested-virt caveat below) if auction inventory doesn't have anything in the
  right location, or if fixed/known specs matter more than auction-roulette:
  ~€34-45/mo for 3-4 cores / 24-32 GB RAM.
- **The bigger issue isn't which host — it's that "add one 2nd node" doesn't
  actually get YugabyteDB to real fault tolerance**, and GreptimeDB clustering
  is a much bigger lift than "add a node." Both are covered in detail below.
  Read those sections before buying anything.

## Grounding: what's actually running today

- `infra/proxmox-greptimedb-lxc.sh` provisions GreptimeDB standalone as an
  **unprivileged LXC**: 2 cores, 2048 MB RAM, 8 GB root disk + 32 GB data
  disk (all configurable, but those are the defaults actually deployed).
  Tiny footprint.
- YugabyteDB's own provisioning script isn't in this repo (`infra/` only has
  the GreptimeDB one) — it's reached purely via the `YugabyteDB` Hyperdrive
  binding (`dovecote/wrangler.toml`) pointing at some connection string. Its
  current node's CPU/RAM/disk footprint and **location are unknown to this
  doc** — see Open Questions.
- Both are reached from Cloudflare Workers only through a Cloudflare Tunnel
  (GreptimeDB's tunnel is documented end-to-end in the LXC script; presumably
  Yugabyte's Hyperdrive connection goes through an equivalent tunnel/Access
  setup, though that isn't in this repo either).
- Officially, YugabyteDB's docs recommend 16 vCPU / 16 GB+ per node for
  "production." That's enterprise-scale guidance and clearly isn't what
  node 1 is running today (a single self-hosted node sized to a hobby-scale
  IoT platform) — sized the *second* node to match node 1's real footprint,
  not the official minimum, once that footprint is known.

## Provider comparison

| Provider | Option | Cost (as of Jul 2026) | Proxmox-capable | Locations |
|---|---|---|---|---|
| **Hetzner** | Server Auction (dedicated, refurbished) | ~€30-50/mo for 4-8 cores / 32-64 GB RAM (e.g. AX41/AX42-class boxes; auction inventory rotates) [[1]](#sources) | Yes — real bare metal, no nested-virt question | Falkenstein/Nuremberg (DE), Helsinki (FI) |
| Hetzner | Cloud CCX (dedicated vCPU) | No longer competitive — CCX13 went from €15.99 to €42.99/mo, CCX63 from €374 to €853/mo in the June 15 2026 repricing (up to 176% on some tiers) [[2]](#sources) | Nested virt not exposed on shared-vCPU lines; unclear/inconsistent on CCX even where "dedicated" | DE, FI, US |
| **Contabo** | VDS (Virtual Dedicated Server) | ~€34.40/mo (3 core / 24 GB) to €82.40/mo (8 core / 64 GB) [[3]](#sources) | Yes, explicitly — Contabo documents nested virt as VDS/Dedicated-only | EU + US |
| Contabo | Cloud VPS (base tier) | From ~$4.95/mo | **No** — Contabo explicitly disables nested virt on plain VPS (HA-migration reasons); confirmed via their own KB [[4]](#sources) | EU + US |
| Contabo | Bare metal dedicated | From ~$99/mo (1-yr term) | Yes, real bare metal | EU + US |
| **OVH Eco range** (Kimsufi/SoYouStart/Rise, now one unified "Eco" tier) | Kimsufi (Eco Light) | From ~$11/mo | Yes, bare metal, but old desktop-class CPUs, forum-only support, 100 Mbps port cap [[5]](#sources) | FR, CA, + others |
| | SoYouStart (Eco Essentials) | ~$30-40/mo | Yes, bare metal, Xeon E5/Ryzen, ticketed support | FR, CA, + others |
| | Rise (Eco Advanced) | Up to ~$80/mo | Yes, bare metal, NVMe, full 1 Gbps | FR, + others |
| **Netcup** | RS (root server) G12 | €8.74/mo (RS 1000, 4 cores/8 GB) to ~€27/mo (RS 4000-class) [[6]](#sources) | **Uncertain** — Netcup's "root server" line is KVM-based, not bare metal; whether nested virt is enabled/available on request wasn't confirmed by an official source in this pass. Ask their support before buying if this is the pick. | DE |
| **Scaleway** | Elastic Metal (Aluminium tier) | From €27.99/mo (or €0.077/hr) [[7]](#sources) | Yes, real bare metal | FR, NL, PL |

## Why Hetzner Server Auction

- It's the only option where "Proxmox-capable" is a non-question — it's
  physical hardware, exactly like (presumably) node 1 already is. No nested-
  virt flag to confirm, no hypervisor-vendor policy to work around.
- Cheapest hardware-per-euro of everything surveyed: a €30-50/mo auction box
  typically has 4-8x the cores and 16-32x the RAM the current GreptimeDB LXC
  actually uses. There's ample room for a 2nd Yugabyte node *and* a 2nd
  Greptime node on one box, with headroom to spare.
- Auction inventory is a lottery (specific listings rotate/sell out — this
  doc can't cite "the exact box available today," only that boxes in this
  class are consistently available in this price range). If nothing suitable
  is listed when you're ready to buy, Contabo VDS is a same-tier fallback
  with fixed, always-orderable specs instead of auction roulette.
- **Location matters more than price here** — see the Yugabyte latency
  section below. If node 1 already lives at Hetzner (Falkenstein/Nuremberg/
  Helsinki), buying the 2nd node in the *same* location keeps inter-node
  latency in the sub-millisecond-to-low-single-digit-ms range. If node 1 is
  elsewhere entirely, Hetzner is still the cheapest bare metal available, but
  every write now pays a cross-provider/cross-datacenter round trip — worth
  weighing against just adding capacity to node 1's existing provider instead
  (a question this doc can't resolve without knowing where node 1 lives).

## The YugabyteDB caveat: one 2nd node isn't real HA

This needs to be said plainly, because "add a 2nd node for HA" is a
reasonable-sounding request that doesn't quite hold up:

- YugabyteDB is Raft-based: a write is only acknowledged once a **majority**
  of replicas have it. Fault tolerance comes from replication factor (RF),
  not node count alone.
- **RF3 (3 nodes) tolerates 1 node failure** — majority is 2 of 3, so losing
  one node still leaves a quorum. This is YugabyteDB's own recommended
  minimum for any real production fault tolerance.
- **RF2 (2 nodes) does not give you that.** Majority of 2 is 2 — losing
  *either* node loses quorum, meaning a 2-node cluster has exactly the same
  write-availability-during-a-failure story as 1 node: none. What 2 nodes
  *do* buy you is a live, queryable replica of the data (useful for read
  scaling or as a warmer failover target you cut over to manually) — just
  not automatic continuous availability through a node failure.
- So: adding **one** 2nd node is a real, worthwhile step (data redundancy,
  read capacity, a faster disaster-recovery story than restoring from
  backup) — just don't describe it as "HA" internally, and know that getting
  to actual automatic-failover HA means budgeting for a **3rd** node down the
  line, not stopping at two.
- **Latency**: every write's latency becomes bounded by the round-trip to
  the farthest replica in the write quorum, because consensus needs that
  replica's ack before the client gets a response. Yugabyte's own guidance:
  keep replicas as close as possible to minimize this, and cross-region
  synchronous replication can add tens to hundreds of ms per write [[8]](#sources).
  For a 2-node RF2 setup specifically, this means the 2nd node should be in
  the **same metro/datacenter** as node 1, not a different country — the
  latency cost of a distant node buys you write-availability, but it buys it
  at every single write's expense, whether or not the distant node is ever
  needed for failover that day.

## The GreptimeDB caveat: standalone → cluster is not "add a node"

The current instance is `greptime standalone start` — a single binary
serving HTTP/gRPC/MySQL/Postgres-wire, no external dependencies
(`infra/proxmox-greptimedb-lxc.sh`). GreptimeDB's actual distributed/cluster
mode is a different architecture, not a 2nd copy of the same binary:

- **Metasrv** — the metadata/routing control plane (table schemas, region
  placement, scheduling). Needs its own metadata backend (etcd, or
  Postgres/MySQL) to store that metadata durably.
- **Frontend** — stateless protocol/query layer. Cheap to add, holds no data.
- **Datanode** — actually stores and serves table regions, and in cluster
  mode is expected to flush to **object storage** (S3-compatible), not local
  disk, so that region data is shared/rebalance-able across datanodes
  without bulk data movement [[9]](#sources).

In other words, a real 2-node-HA GreptimeDB setup means standing up metasrv +
frontend + datanode roles (at least 3 logical services, however many
physical nodes they land on) *and* an S3-compatible object store *and* a
metadata store — a meaningfully bigger operational surface than the current
single systemd unit.

**Is it worth it right now?** Probably not yet, for two reasons specific to
this stack:
1. Telemetry already has a graceful degradation path: `dovecote`'s
   `write_telemetry_default` (`dovecote/src/helpers/greptime.rs`) already
   falls back to writing Postgres's `pigeon_telemetry_history` when the
   Greptime write fails — a Greptime outage today degrades telemetry history
   to "goes to Postgres instead," not "is lost." That's a real, working
   soft-HA story already, at zero extra infrastructure cost.
2. Retention is only 90 days by design (`init-greptime.sh`), and this is a
   hobby-scale IoT telemetry store, not a system anything else depends on
   for correctness.

**Simpler alternatives that get most of the practical benefit:**
- **Backup-to-object-storage**: point GreptimeDB's own storage backend (or a
  periodic snapshot/`pg_dump`-equivalent) at an S3-compatible bucket —
  Cloudflare R2 is already in the stack for FOTA firmware images
  (`pidgeiot-firmware`/`-staging` buckets), so the credential/tooling
  plumbing to talk to R2 already exists. This gets you "can rebuild from a
  recent backup" without a live 2nd node at all.
- **Cold/warm standby**: a 2nd GreptimeDB *standalone* instance (not a
  cluster) that periodically syncs the data directory or replays recent
  writes, promoted manually if the primary dies. Simpler than clustering,
  though not synchronous/automatic.
- Only reach for real metasrv/frontend/datanode clustering if telemetry ever
  becomes something other agents/pages actually depend on for real-time
  correctness (it currently isn't, per the fallback above).

## Cloudflare Tunnel implications

- Today: one tunnel, one hostname (`telemetry.pidgeiot.com`) → one origin.
  Adding a 2nd GreptimeDB node (in whatever topology — standby or true
  datanode) means either:
  - **Separate hostname per node** (e.g. `telemetry-2.pidgeiot.com`) with its
    own tunnel — simplest, but `dovecote` would need to know about both
    explicitly (e.g. to read from a healthy one, or as a manual DR
    switchover target).
  - **One Cloudflare Load Balancer in front of both**, each node as a pool
    origin reached via its own tunnel — gives health-checked automatic
    failover, but is real additional Cloudflare configuration (Load
    Balancer is a paid feature above a free allowance) and needs each
    tunnel's origin configured consistently (same path/port shape) [[10]](#sources).
- Yugabyte's Hyperdrive path is different in kind: Hyperdrive pools
  connections to **one** connection string per binding today
  (`[[hyperdrive]] binding = "YugabyteDB"`, `dovecote/wrangler.toml`).
  A 2-node Yugabyte cluster is usually reached via a *single* smart-driver-
  aware connection string that already knows about all cluster nodes (that's
  how Yugabyte's Postgres-wire compatibility + cluster-aware drivers
  normally work) — so this likely doesn't need a 2nd Hyperdrive binding or a
  2nd tunnel hostname at all, just the existing tunnel's origin pointing at
  a cluster-aware endpoint. Worth confirming against Hyperdrive's own
  cluster-awareness docs before assuming either way.

## Open questions for the user

1. **Where does node 1 live today** (which provider, which city/datacenter)?
   This drives the whole recommendation — if it's already at Hetzner, buy
   the 2nd node in the *same* Hetzner location; if it's somewhere else, the
   latency/provider-consistency tradeoff above needs a real answer, not an
   assumption.
2. **What RF/HA level are you actually targeting?** Per the Yugabyte section
   above: is the goal "a live replica + faster DR" (2 nodes, achievable now)
   or "true automatic-failover HA" (needs 3 nodes — budget accordingly, this
   doc's "2nd node" framing doesn't get you there alone)?
3. **Budget ceiling** — the comparison table spans roughly $10/mo (Kimsufi,
   with real caveats) to $80+/mo (Rise/Scaleway). Where's the ceiling?
4. **Is real GreptimeDB clustering actually wanted**, given the existing
   Postgres-fallback softens the urgency — or would a simpler backup/
   warm-standby approach satisfy the actual goal here?

## Sources

1. Hetzner Server Auction / AX-line pricing and specs — [Hetzner Server Auction](https://www.hetzner.com/sb/), [AX42 press release](https://www.hetzner.com/pressroom/new-ax42/), [AX Server configurations](https://docs.hetzner.com/robot/dedicated-server/server-lines/ax-server/), [Achromatic Hetzner comparison](https://www.achromatic.dev/blog/hetzner-server-comparison) (accessed 2026-07-22)
2. Hetzner June 2026 price increase — [Hetzner's official price-adjustment notice](https://docs.hetzner.com/general/infrastructure-and-availability/price-adjustment/), [webhosting.today breakdown](https://webhosting.today/2026/06/18/hetzners-price-increases-reached-209-the-30-headline-applied-to-a-different-tier/), [wz-it.com analysis](https://wz-it.com/en/blog/hetzner-price-increase-june-2026-cpx-ccx-alternatives/) (accessed 2026-07-22)
3. Contabo VDS pricing — [onedollarvps.com Contabo pricing](https://onedollarvps.com/pricing/contabo-pricing) (accessed 2026-07-22)
4. Contabo nested virtualization policy — [Contabo KB: "Can I Setup Nested Virtualization On My Server?"](https://contabo.com/blog/kb/103000271595-can-i-setup-nested-virtualization-on-my-server/) (accessed 2026-07-22)
5. OVH Eco range (Kimsufi/SoYouStart/Rise) restructure and pricing — [valebyte.com breakdown](https://valebyte.com/en/blog/ovh-soyoustart-vs-kimsufi-vs-eco-where-the-budget-dedicated-servers-moved/), [Kimsufi official](https://www.kimsufi.com/en/ks/) (accessed 2026-07-22)
6. Netcup RS G12 pricing — [Netcup Voucher Blog G12 first look](https://netcupvoucher.com/blog/netcup-root-server-g12-is-out), [Netcup root server product page](https://www.netcup.com/en/server/root-server) (accessed 2026-07-22)
7. Scaleway Elastic Metal pricing — [Scaleway Elastic Metal Pricing](https://www.scaleway.com/en/pricing/elastic-metal/) (accessed 2026-07-22)
8. YugabyteDB multi-region latency and RF guidance — [Synchronous multi region (3+ regions)](https://docs.yugabyte.com/stable/explore/multi-region-deployments/synchronous-replication-ysql/), [Multi-Region Database Deployment Best Practices](https://www.yugabyte.com/blog/multi-region-database-deployment-best-practices/), [YugabyteDB Anywhere hardware requirements](https://docs.yugabyte.com/stable/yugabyte-platform/prepare/server-nodes-hardware/) (accessed 2026-07-22)
9. GreptimeDB distributed architecture (metasrv/frontend/datanode, object storage) — [GreptimeDB Architecture docs](https://docs.greptime.com/user-guide/concepts/architecture/), [Metasrv overview](https://docs.greptime.com/contributor-guide/metasrv/overview/) (accessed 2026-07-22)
10. Cloudflare Load Balancer + Tunnel — [Cloudflare Load Balancer with Cloudflare Tunnel](https://nyan.im/p/cloudflare-load-balancer-tunnel), [Cloudflare Load Balancing docs](https://docs.cloudflare.com/load-balancing) (accessed 2026-07-22)
