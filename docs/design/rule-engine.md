# User-authored rule engine on Workers for Platforms (task #12)

Status: design doc, no code changes. Scope: let a user author their own data-processing
logic against their pigeons' telemetry (and, later, shadow/lifecycle events), executed as
isolated Workers via Cloudflare **Workers for Platforms** (dispatch namespaces, user
Workers, outbound Workers, custom limits) — PidgeIoT's answer to ThingsBoard's Rule Engine,
differentiated by running real user code at the edge instead of a visual chain editor.

Grounded in: `dovecote/src/queue.rs`, `dovecote/src/helpers/alerts.rs`,
`dovecote/src/objects/pigeons.rs` (telemetry write/alert-evaluation call sites),
`dovecote/src/objects/ws.rs`, `capsules/src/lib.rs` (the `Alert*`/`Firmware*` families),
`infra/init-db.sql`, `dovecote/wrangler.toml`, `docs/api.md`, and both existing docs in
`docs/design/` (`alerts-triggers.md`, `tenancy-isolation.md`), used here as the house
style/structure to match. `alerts-triggers.md` §5 already anticipated this doc directly and
is treated as prior art, not a competing design (see §0 below). Cloudflare product details
are cited inline with access dates (2026-07-24); no fact about Workers for Platforms below
is asserted from memory alone.

## 0. Relationship to the shipped alerts feature

Task #32's alerts feature (`docs/design/alerts-triggers.md`, now fully shipped through task
#41 — `capsules::AlertCondition`/`AlertScope`/`AlertChannel`, `alert_definitions`/
`alert_state` in Postgres, `check_telemetry_alerts`/`evaluate_scheduled_alerts` in
`dovecote/src/helpers/alerts.rs`) is a **closed, structured subset** of what this doc
proposes: four fixed condition types (`Threshold`, `RateOfChange`, `DeviceState`,
`MissingReport`), evaluated by hand-written Rust match arms, no user-authored code at all.
That doc's own §5 predicted this one almost exactly: "the recommended hook point... is
exactly the same injection point... a user-worker dispatch becomes a third branch, or the
generalization of both." Confirmed by re-reading the current code: that "hook point" is now
`store_and_alert` (`queue.rs:354`) plus its two no-queue-fallback mirrors inside
`objects/pigeons.rs` (`report_telemetry_device:1716`, `handle_ws_telemetry`'s no-queue
branch:~1530) — the exact three call sites `check_telemetry_alerts` already runs from today.
Nothing here proposes replacing that machinery; §1 below adds a **parallel, best-effort
branch at the same seam**, and §3 reuses (not duplicates) its Postgres conventions,
Resend-email plumbing, and per-pigeon-vs-per-flock scoping model wherever the shape matches.

## TL;DR

1. **Hook point**: a new best-effort branch alongside `check_telemetry_alerts` at its
   existing three call sites, not a new consumer — dispatch to a user's Worker via
   `env.DISPATCHER.get(...)`, read its `Response` body as the output contract (derived
   telemetry + optional alert), never block the device's own ack.
2. **Cost floor is real and matters pre-revenue**: Workers for Platforms is a flat
   **$25/month** platform fee (20M requests + 60M CPU-ms + 1,000 scripts included; overage
   $0.30/M requests, $0.02/M CPU-ms, $0.02/script) before a single user rule ever runs —
   see §6.4. A **Phase 0 hosted-expression-language tier** (no WfP purchase at all,
   Rust-evaluated JSON expressions, same shape as `AlertCondition`) is a defensible interim
   step if this ships before the product has paying customers.
3. **MVP needs no outbound Worker at all**: setting `subRequests: 0` in the dispatch call's
   custom limits (§4.2) makes "no egress" free — a rule's only output channel is its
   `Response` body, which the *dispatch* Worker (not the user Worker) reads; the user
   Worker never needs its own `fetch()` capability to emit a derived value or an alert.
   Outbound Workers only become necessary once Phase 2 wants user-controlled egress.
4. **Rules never gate ingestion** — same "best-effort, log, never fail the primary write"
   convention this codebase already applies everywhere (root `CLAUDE.md`), already proven
   at this exact seam by `check_telemetry_alerts` itself.
5. **Recommended MVP**: one JS rule per **flock** (script-count scales with tenant count,
   not device count — see §6.4's script-overage math), transform (derived telemetry,
   Postgres-history-only, §1.3) + fire-and-forget alert email only, `subRequests: 0`, no
   egress, no shadow write-back, no dry-run UI yet. Effort: comparable to the FOTA (#23) or
   WS-endpoint (#32) bodies of work combined — new Cloudflare product integration, new
   Postgres schema, new dovecote routes, new fancier CRUD UI (§7).

## 1. Trigger/data-flow architecture

### 1.1 Where a rule hooks into the existing pipeline

The task's own framing — "the telemetry queue consumer is the injection point" — needs the
same correction `alerts-triggers.md` §2.1 already made for alerts, for the identical reason:
`queue.rs::dispatch_to_do` only ever runs in queue-bound environments (staging/prod), and
even there it's one of three ingestion paths (HTTP-sourced, WS-sourced, and the two
no-queue direct-write fallbacks used by `dev`). The real convergence point, reused verbatim
from the alerts work, is `store_and_alert` (`queue.rs:354`) and its two mirrors inside
`objects/pigeons.rs` — every one of the three paths ends by calling
`write_telemetry_default` then `check_telemetry_alerts` with the same `(env, pigeon_id,
metrics, previous_values, reported_at_ms)` tuple in hand. Add a sibling call,
`dispatch_rule(env, pigeon_id, metrics, previous_values, reported_at_ms)`, at the same three
sites — this is a **fourth branch alongside `check_telemetry_alerts`**, not a replacement,
matching how the alerts doc itself framed the relationship (§0 above).

### 1.2 Event shape a rule receives

The dispatch Worker (dovecote's own top-level Worker/DO code) builds a synthetic `Request`
and calls `userWorker.fetch(request)` via the dispatch namespace binding — the request body
is the event:

```json
{
  "event": "telemetry",
  "event_id": "0199a1b2-...",
  "pigeon_id": "01978f...",
  "flock_id": "3fae21e0-...",
  "reported_at_ms": 1721838421000,
  "metrics": { "battery_mv": "3300", "rssi": "-71" },
  "previous": { "battery_mv": { "value": "3400", "reported_at_ms": 1721838120000 } }
}
```

`metrics`/`previous` mirror exactly what `check_telemetry_alerts` already receives (same
`HashMap<String, String>` / `PreviousTelemetryValue` shapes from `objects/pigeons.rs`) — no
new data model to invent for the input side. `event_id` is new: **none of the three
ingestion paths generate one today** (confirmed — `TelemetryMessage`, `PreviousTelemetryValue`,
and the direct-write fallbacks carry no stable per-report identifier). Recommend adding one
at first ingestion (a UUID v4, threaded through `TelemetryMessage`/the no-queue call
signatures the same way `previous_values_json` was added for task #41) specifically so a
rule author can de-duplicate a retried delivery (§1.4) — this is new plumbing, not reused
from the alerts feature, since alerts' own debounce/hysteresis state (`alert_state`) doesn't
need a stable event identity the way arbitrary user code doing its own idempotency might.
`event: "telemetry"` is a discriminant left room to grow — `"shadow_report"` / `"lifecycle"`
are Phase 3 additions (§8), not built now.

### 1.3 What a rule can do

The **output contract** is the dispatch Worker's own read of the invoked Worker's
`Response` body — a rule "does" something purely by returning JSON, never by making its own
subrequest (this is what makes `subRequests: 0` viable for MVP, §4.2):

```json
{
  "derived": { "battery_pct": "82.5" },
  "alert": { "severity": "warning", "message": "battery below 20% for the first time" }
}
```

| Capability | MVP (Phase 1) | Later |
|---|---|---|
| **Derived telemetry** (`derived`) | Written to `pigeon_telemetry_history` only (Postgres), tagged `source='rule'` (new nullable column, default `'device'`) — reuses `write_telemetry_history`'s insert shape verbatim, just a second caller. **Not** upserted into the DO's own live `pigeon_telemetry` table in MVP (see the punt list, §8) — avoids a DO schema change and a new DO-internal route for v1, at the cost of "latest telemetry" dashboard views not showing rule-derived values yet, only history/graphs (which already read `pigeon_telemetry_history`, unmodified). | DO-mirrored, so derived keys show up identically to device-reported ones everywhere, including the live shadow-adjacent "latest telemetry" view. |
| **Emit alerts** (`alert`) | Fire-and-forget: call the *same* Resend-send + `resolve_alert_recipient` helpers `helpers/alerts.rs` already has (shared **function**, not a shared **table** — no `alert_definitions` row is synthesized for a rule's alert output) — no debounce/hysteresis state. Acceptable for MVP because, unlike `AlertCondition`'s closed set, a rule has full code and can implement its own debounce inline if the author cares to. | A `rule_alert_state` table, structurally identical to `alert_state`, if unmoderated repeat-fire turns out to be a real problem in practice — don't build ahead of that evidence. |
| **Forward to a user endpoint** | Redundant with what `telemetry_endpoint` line-protocol forwarding already does, and with what a rule could do itself once it has `fetch()` (Phase 2, §8) — not a separate capability to design; note the overlap so nobody builds it twice. | A rule with egress *is* the general form of today's `telemetry_endpoint` forward — could eventually be reimplemented as a "default" platform-authored rule, but that's a migration to consider later, not now. |
| **Write back shadow values** | **Not allowed.** See §1.3.1. | Phase 3, heavily guarded. |

#### 1.3.1 Why shadow write-back is deliberately excluded from MVP

A rule mutating `target_config` races the dashboard's own `PUT /pigeons/:id/shadow` (both
would bump `target_version` through the same DO route with no coordination), and creates an
obvious feedback loop: rule reads telemetry → writes shadow → device applies and reports
`current_config` → that report re-triggers telemetry evaluation → rule fires again. None of
the four `AlertCondition` variants can do this today (they're read-only evaluators by
construction); a rule engine's whole point is running arbitrary code, so this has to be an
explicit, argued exclusion rather than an accident of scope. If ever added: route through the
existing `update_shadow`/`report_shadow_device` paths unchanged (no new shadow-write
mechanism), and gate behind a hard per-rule rate limit plus a same-report re-entrancy guard
(a rule invoked as a direct result of its own prior shadow write must not write again without
an intervening device-initiated report) — real design work, correctly deferred.

### 1.4 Guarantees

- **Delivery**: at-least-once, inherited from Cloudflare Queues' own documented guarantee
  ("Queues provides at least once delivery... in rare occasions, may be delivered more than
  once" — [Cloudflare Queues: Delivery guarantees](https://developers.cloudflare.com/queues/reference/delivery-guarantees/),
  accessed 2026-07-24) wherever the report arrived via the queue-bound path. The two
  no-queue fallbacks (dev) call `dispatch_rule` synchronously, once, per report — no
  retry there today, matching `check_telemetry_alerts`'s existing behavior at those same
  call sites. Net effect: a rule **may** see the same `event_id` twice; author guidance
  (docs, not enforcement) should say so plainly, which is exactly why §1.2 adds `event_id`.
- **Ordering**: none, across pigeons or within one pigeon's own reports — Cloudflare Queues
  "does not guarantee that messages will be delivered to a consumer in the same order in
  which they are published" (same source). This is not a new risk this design introduces;
  `check_telemetry_alerts`'s `RateOfChange` evaluation already lives with the same lack of
  ordering guarantee today (its "previous value" comes from a synchronous read-before-write
  in the DO, not from queue order) — rules inherit an already-accepted property, not a new one.
- **Latency / never blocking the device's own ack — a real asymmetry found in the current
  code**: in the **queue-bound** path, `store_and_alert` (and therefore `check_telemetry_alerts`
  today, and `dispatch_rule` tomorrow) already runs *after* `message.ack()` — fully decoupled
  from any device-facing response, since the queue consumer has no HTTP/WS response to
  protect in the first place. But in the **no-queue fallback** path (`report_telemetry_device`,
  `objects/pigeons.rs:1716`), `check_telemetry_alerts` is `.await`ed **before** the function
  returns `Response::from_json(&metrics)` — it is on the device's synchronous request path
  today, accepted because a Postgres alert-definitions lookup is cheap. A full user-Worker
  dispatch is not guaranteed cheap (arbitrary code, even under a CPU-ms cap) — recommend
  `dispatch_rule` specifically use `Context`/DO-hibernation-safe fire-and-forget scheduling
  (`wait_until`-equivalent) rather than copying `check_telemetry_alerts`'s synchronous-await
  pattern verbatim, so a slow rule never adds latency to a device's HTTP ack even in the
  no-queue (dev) environment. **Needs one empirical check before committing to this**: it's
  unconfirmed whether a detached future survives long enough inside a Durable Object's own
  request-handling context (as opposed to the top-level Worker `fetch` handler, which has a
  well-documented `ctx.waitUntil`) — verify against `wrangler dev` before relying on it,
  the same "verify, don't assume" discipline `tenancy-isolation.md` §2.2 already applied to
  the Hyperdrive-pooling question.

## 2. Authoring model

### 2.1 Language: plain JavaScript (Workers module syntax), not a DSL, for MVP

Matches Cloudflare's own authoring ergonomics directly — a user Worker is created the
normal way (`npm create cloudflare@latest ... --type=hello-world`) and uploaded into a
dispatch namespace ([Workers for Platforms: get started](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/get-started/configuration/),
accessed 2026-07-24) — dovecote doesn't need to invent a sandboxed language or a
JS-subset parser; Cloudflare's own dispatch-namespace isolation *is* the sandbox (§4). A
constrained visual DSL (closer to ThingsBoard's own rule-chain editor) is real,
user-requested product surface eventually (§8, Phase 3) but is strictly additive — it would
compile down to the same JS module + output contract this doc defines, not a second
execution path.

### 2.2 Upload/versioning/deploy

Cloudflare's dispatch-namespace script upload is its own API call (`wrangler
dispatch-namespace create <name>` once per environment, then a script upload per rule —
[How Workers for Platforms works](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/how-workers-for-platforms-works/),
accessed 2026-07-24: "your platform takes the code your customers write, and then makes an
API request to deploy that code as a user Worker to a namespace"). Recommend dovecote proxy
this exactly the way it already proxies a firmware upload:

- `POST /pigeons/:id/rules` / `POST /flocks/:id/rules` (owner-gated, mirrors
  `AlertScope`'s dual per-pigeon/per-flock model) takes raw JS source text, computes its
  sha256 server-side (reuse `helpers/firmware.rs::sha256_hex`, not a new hash routine), and:
  1. Stores the source in R2, content-addressed (`rules/<sha256>.js`) — **identical pattern
     to `FirmwareImage`**, including its own dedupe-by-hash behavior (a resubmission of
     identical source under a new "version" label updates the catalog row, no duplicate
     R2 object).
  2. Uploads it to the dispatch namespace as a script named `<pigeon_id>` or
     `flock-<flock_id>` via the Cloudflare API (a `CLOUDFLARE_API_TOKEN`-scoped secret, new
     — the first Worker secret in this codebase whose purpose is calling the Cloudflare API
     itself rather than a third-party service).
  3. Records a `rule_definitions` Postgres row (§3) pointing at both the R2 object and the
     dispatch-namespace script name.
- **No native rollback/version history to lean on**: Workers for Platforms dispatch scripts
  don't get the preview-URL/gradual-rollout machinery standalone Workers get (this codebase
  already knows that pattern doesn't apply to DO-backed Workers either — see
  `wrangler.toml`'s own comment on why `dovecote-staging` exists as a separate script rather
  than a preview alias). Recommend dovecote keep its **own** version history the same way it
  already does for firmware: every re-upload creates a new content-addressed R2 object and a
  new `rule_definitions` row version; "rollback" is "redeploy an older stored source" from
  dovecote's own catalog, not anything WfP provides natively.

### 2.3 `fancier` editing UX (sketch)

New route mirroring the alerts feature's own routing convention:
`#[route("/flocks/:flock_id/rules")]` plus a per-pigeon variant. A plain `<textarea>`-based
editor is sufficient for MVP — a real code-editor widget (syntax highlighting, inline
errors) is real UX value but a heavier lift for a Dioxus/WASM SPA specifically because it
means embedding a JS-authored editor component (e.g. CodeMirror) inside a Rust/WASM app; the
existing lazy-loaded WASM-split work already in this codebase (bundle code-splitting so a
heavy dependency loads only on the route that needs it) is the natural mechanism to bring
that in later **without bloating every other route's bundle** — call this out as the
concrete follow-up rather than a vague "nice to have."

### 2.4 Testing/dry-run against recent real telemetry

Recommend a `POST /flocks/:id/rules/:rule_id/test` route: takes the *unsaved* candidate
source directly in the request body (not the stored/deployed version), pulls the flock's
own recent telemetry via the **already-existing** `GET /flocks/:id/telemetry/history`
machinery as the input corpus, invokes the candidate source as a **scratch, unnamed**
dispatch-namespace upload (uploaded, invoked once per sampled event, then left in place —
Cloudflare has no "ephemeral eval" primitive for dispatch scripts; a stale test script
sitting unused in the namespace is a cheap, ignorable side effect, not a resource leak worth
engineering around at MVP), and returns the output contract for each sampled event without
persisting anything. UI-side, this is the same Live/Empty/Preview state pattern the
telemetry-graph section of `views/pigeon.rs` already established — not a new state-rendering
convention.

## 3. Storage model

Follows `alert_definitions`' own reasoning near-verbatim (`tenancy-isolation.md`'s
Postgres-vs-DO argument, already applied twice — once to `FirmwareImage`, once to
`AlertDefinition`): a rule definition is dashboard-authored config with no device-facing
counterpart, a flock-scoped rule has no DO to live in, and the dashboard's natural view
("every rule for this flock/pigeon") is inherently cross-entity — Postgres territory.

```sql
CREATE TABLE IF NOT EXISTS rule_definitions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL,
  flock_id UUID REFERENCES flocks(id) ON DELETE CASCADE,
  pigeon_id TEXT REFERENCES pigeons(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  script_sha256 TEXT NOT NULL,        -- R2 key: rules/<sha256>.js
  dispatch_script_name TEXT NOT NULL, -- name registered in the WfP dispatch namespace
  enabled BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT rule_definitions_scope_check CHECK (
    (flock_id IS NOT NULL AND pigeon_id IS NULL) OR
    (flock_id IS NULL AND pigeon_id IS NOT NULL)
  )
);

-- Execution health/error visibility (no existing "feed_runs" table exists anywhere in
-- this codebase to reuse -- grepped for it; it doesn't exist. This is a new, small,
-- capped log, closer in spirit to the device log ring buffer's "rolling debug data, not
-- a durable store" framing than to alert_state's indefinite per-pair row.)
CREATE TABLE IF NOT EXISTS rule_runs (
  id BIGSERIAL PRIMARY KEY,
  rule_definition_id UUID NOT NULL REFERENCES rule_definitions(id) ON DELETE CASCADE,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  event_id UUID NOT NULL,
  status TEXT NOT NULL,          -- 'ok' | 'error' | 'timeout' | 'limit_exceeded'
  duration_ms INTEGER,
  error TEXT,
  ran_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

`rule_runs` is capped per `rule_definition_id` (recommend last 50, pruned by the existing
missing-heartbeat Cron Trigger's `#[event(scheduled)]` handler — reuse that infra, don't
stand up a second cron) rather than growing unbounded, mirroring `PigeonLogChunk`'s
"200-chunk ring, oldest pruned" convention rather than `pigeon_telemetry_history`'s
indefinite-retention one — this is diagnostic/debugging data about the rule's own health,
not billing or business data. `capsules::RuleDefinition`/`RuleRun` would follow the same
`*Row`-with-native-`OffsetDateTime` pattern `AlertDefinition`/`FirmwareImage` already use
(Postgres hands back `TIMESTAMPTZ` natively, no epoch-float conversion needed).

## 4. Isolation, safety, and limits

### 4.1 Per-tenant Worker in a dispatch namespace

One dispatch namespace per environment (`dovecote-rules`, `dovecote-rules-staging`,
`dovecote-rules-dev` — matching the existing per-env queue/R2-bucket naming convention in
`wrangler.toml`), one script per `rule_definitions` row. Isolation is Cloudflare's, not
dovecote's own: user Workers run in "untrusted mode" by default, giving "complete isolation
between customer Workers, preventing any potential cross-tenant data access," each with its
own isolated cache and no access to `request.cf`
([Worker Isolation](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/platform/worker-isolation/),
accessed 2026-07-24) — this is a first-party, supported isolation boundary, not something
this design has to build or reason about at the V8-isolate level itself (contrast §6.1).

### 4.2 Custom limits — the mechanism that makes an outbound Worker unnecessary for MVP

The dynamic dispatch Worker (dovecote's own code, calling `env.DISPATCHER.get(...)`) sets
per-invocation limits directly: `{ limits: { cpuMs: <n>, subRequests: <n> } }` — exceeding
either "immediately throw[s] an exception" in the user Worker
([Custom limits](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/configuration/custom-limits/),
accessed 2026-07-24). Recommend MVP values: **`cpuMs` small** (a rule's job is threshold/
arithmetic-scale work, not heavy compute — start conservative, e.g. double-digit
milliseconds, and tune empirically once real rules exist rather than asserting a final
number here) and **`subRequests: 0`**. The second value is the load-bearing one: since a
rule's only output channel is its own `Response` body (§1.3) — read by the *dispatch*
Worker, which is a separate hop, not a subrequest *from* the user Worker — a rule genuinely
needs zero outbound capability to transform telemetry or emit an alert. This gets "no
egress" in MVP for free, with no outbound Worker component to build, configure, or reason
about the security of yet.

### 4.3 Outbound Worker — deferred to Phase 2, egress allowlisting

Once a rule needs to call the user's *own* endpoint (their webhook, their own SaaS API),
`subRequests: 0` has to relax, and an outbound Worker becomes the actual safety boundary —
it sits between every user Worker's `fetch()` and the public Internet
([Outbound Workers](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/configuration/outbound-workers/),
accessed 2026-07-24). Configuration is a `dispatch_namespaces.outbound` block naming a
service Worker and a `parameters` list; the dispatch call then passes tenant context through
an `outbound: { <param>: <value> }` object matching those declared names, which the outbound
Worker's own `fetch(request, env, ctx)` receives in `env` — exactly the mechanism to tag
every outbound request with `rule_definition_id`/`user_id` and enforce a per-tenant
allowlist (or simply log for abuse investigation). Recommend the allowlist model
specifically (a user must declare which hostnames their rule may reach, checked in the
outbound Worker) rather than a denylist — "can't reach dovecote's own infra" (`api.pidgeiot.com`,
the Hyperdrive/GreptimeDB-tunnel origins) is then true **by construction**, not something a
denylist has to remember to include.

### 4.4 Secrets for user rules

Deferred alongside egress (§4.3) — no reason to design credential storage for endpoints a
rule can't yet reach. When it lands: a `rule_secrets` table, write-once-return-never on
reads, exactly like `Connector`/device tokens' existing convention in this codebase, with
values attached to the user Worker as plaintext env vars or Worker secrets at upload time
(same upload-time API call §2.2 already makes) — never round-tripped back to the browser
after the initial write.

### 4.5 Preventing rules from hammering dovecote's own API

Answered structurally, not by a new guard: MVP's `subRequests: 0` makes this impossible by
construction (§4.2); Phase 2's allowlist-model outbound Worker (§4.3) keeps it impossible by
construction even once egress opens, rather than becoming a denylist entry someone has to
remember to add.

## 5. Failure semantics

- **A rule throwing, timing out, or hitting a custom limit must never affect telemetry
  ingestion or the device's own response** — the single universal convention this codebase
  already applies to every cross-store sync (root `CLAUDE.md`: "best-effort/fire-and-log,
  never blocking or failing the primary request"), and the exact behavior
  `check_telemetry_alerts` already has in production at this identical seam. `dispatch_rule`
  catches/logs (`console_error!`) any error from the dispatch `fetch()` call — including the
  "immediately throws an exception" case custom-limit violations produce — and moves on.
- **No retry, no dead-letter queue for v1.** `store_and_alert`'s existing calls
  (`write_telemetry_default`, `check_telemetry_alerts`) already fail silently-logged with no
  retry mechanism of their own — `dispatch_rule` should match that convention exactly rather
  than introduce a new, inconsistent retry/DLQ story for only this one branch. A rule's own
  `rule_runs` row (§3) is the durable record of "this ran and failed," which is what
  operator-facing health visibility needs; it is not a queue.
- **Per-rule health visibility in `fancier`**: the team's own framing was "something like
  the alerts/`feed_runs` precedent" — worth stating plainly that no `feed_runs` table exists
  anywhere in this codebase today (grepped for it directly); `rule_runs` (§3) is a new,
  small design built to fill that role, not a reuse of something that already exists. Render
  it in the rule editor route the same way the device log viewer renders its own ring
  buffer — last N runs, status badge (reusing `connection_state`'s `badge-warning`/
  `badge-error` classes, same as alerts), duration, and truncated error text.

## 6. Alternatives considered

### 6.1 Server-side WASM plugins evaluated inside the pigeon's own Durable Object

Run user-supplied WASM (via an embedded runtime like `wasmtime`) directly inside the
`Pigeons` DO. Rejected: (a) real technical risk, unverified — `workerd` (the runtime
dovecote itself already runs inside) is itself a sandboxed Wasm/V8 environment; embedding a
second, general-purpose Wasm *engine* (which typically wants its own JIT/mmap) inside that
first sandbox is unproven and would need a build spike before it could even be evaluated,
unlike Workers for Platforms, which is a first-party Cloudflare product built for exactly
this use case. (b) Worse isolation story even if it worked: a plugin running inside a
pigeon's own DO shares that DO's execution context with the *actual device's* real requests
— a runaway rule could measurably slow down or exhaust CPU for that same pigeon's real
shadow/telemetry traffic, directly violating "rules never gate ingestion" (§5) in a way a
genuinely separate WfP user Worker cannot.

### 6.2 Embedded QuickJS (e.g. `rquickjs`) compiled into dovecote itself

Run small JS snippets synchronously inside dovecote's own Worker binary, no dispatch
namespace, no Cloudflare product purchase. Same two objections as §6.1: unverified whether a
C-dependent JS engine compiles cleanly to the `wasm32-unknown-unknown` target `worker-build`
already produces for this codebase (a real build spike, not a known quantity), and — even if
it compiled — every invocation would share the CPU-ms budget of whichever Worker isolate
happens to be handling it, which Cloudflare may reuse across unrelated pigeons' requests at
its own discretion. Weaker isolation than a dedicated per-tenant WfP Worker, for no cost
savings over the Phase 0 option below once the risk of the build spike is priced in.

### 6.3 Hosted expression language, evaluated in dovecote, no Workers for Platforms at all

The same shape as `AlertCondition` today — a small, closed grammar (arithmetic, comparators,
simple key-to-key mapping) stored as JSONB and evaluated by hand-written Rust, hooked into
the exact same `store_and_alert` seam. **This is not rejected — it's recommended as Phase 0**
(§7) precisely because it's cheap, fully sandboxed by construction (no arbitrary code, ever),
and ships fast by copying a pattern this codebase has already built and proven twice
(`AlertCondition`, then its `RateOfChange` extension). Its ceiling is real: no loops, no
third-party calls, no shadow logic more complex than the grammar anticipates — the entire
reason task #12 exists is that ThingsBoard's own visual rule chains hit exactly this kind of
ceiling and PidgeIoT's stated differentiator is *real code* instead. Recommend framing this
as a genuine phase (with a real user-facing UI), not a throwaway prototype — see §7.

### 6.4 Why Workers for Platforms wins for anything past Phase 0 — cost finding

$25/month flat platform fee, 20M requests + 60M CPU-ms + 1,000 scripts included, overage at
$0.30/million requests, $0.02/million CPU-ms, $0.02/additional script
([Workers for Platforms: Pricing](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/reference/pricing/),
accessed 2026-07-24). Two implications worth being explicit about:

- **The $25/month floor is a real, non-negotiable cost the moment this feature exists at
  all**, independent of usage — it's not a per-tenant marginal cost the product can defer
  until a rule is actually invoked. This is the concrete argument for **not** enabling any
  paid-tier rule-engine functionality pre-revenue without a deliberate call that the cost is
  worth it (§9, decision list).
- **Script count, not request volume, is the more interesting constraint for this product
  shape specifically**: 1,000 scripts included, $0.02/script beyond that. A **per-pigeon**
  scoping model (one script per device) hits that ceiling as soon as one flock has 1,000+
  pigeons; a **per-flock** model (one script shared by every pigeon in a flock, mirroring
  `FirmwareImage`'s existing per-flock scoping, not per-pigeon) scales with *tenant* count
  instead, which is the right axis to optimize for — this is the concrete reasoning behind
  recommending flock-first scoping in §7, not device-first, even though `AlertScope` offers
  both and nothing here forces the same choice.
- **Billing is chained but counted once per request**: "Workers for Platforms only charges
  for 1 request across the chain of dispatch Worker -> user Worker -> outbound Worker," but
  CPU time is aggregated across all three ([How Workers for Platforms works](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/how-workers-for-platforms-works/),
  accessed 2026-07-24) — a slow rule directly consumes dovecote's *own* CPU-ms allotment,
  reinforcing why a tight `cpuMs` custom limit (§4.2) is a cost control, not just a safety one.

## 7. Phasing

| Phase | Scope | Effort (rough) |
|---|---|---|
| **0 (optional, pre-WfP)** | Hosted expression-language tier (§6.3): `rule_definitions`-lite (condition as JSONB, no script/R2/dispatch fields), Rust evaluator, hooked into `store_and_alert` as today's fourth branch pattern. No JS, no arbitrary code, no WfP purchase. | Similar size to the `RateOfChange` alert extension (task #39) — a few focused days, mostly copying an already-proven pattern. |
| **1 (MVP)** | Purchase WfP. One dispatch namespace per environment. `rule_definitions`/`rule_runs` Postgres tables (§3). Script upload route (owner-gated, R2 content-addressed, §2.2). `dispatch_rule` as a fourth branch at the three existing ingestion call sites (§1.1), `wait_until`-scheduled per §1.4's caveat. Custom limits: small `cpuMs`, `subRequests: 0` (§4.2) — **no outbound Worker built yet**. Output contract: `derived` (Postgres history only, §1.3) + fire-and-forget `alert` email. `fancier`: plain-textarea CRUD, no dry-run yet. Scope: per-flock (§6.4). | Comparable to FOTA (#23) or the WS device endpoint (#32) — a new Cloudflare product integration plus new schema, routes, and UI, done together. |
| **2** | Outbound Worker + per-tenant egress allowlist (§4.3), `rule_secrets` (§4.4), dry-run/test route against real telemetry history (§2.4), `rule_runs` surfaced in `fancier` (§5), shared debounce state for rule-emitted alerts if repeat-fire proves to be a real problem (§1.3). | Similar scale to Phase 1 again — a second, smaller integration effort layered on working infrastructure. |
| **3 (full vision)** | Shadow write-back (§1.3.1, heavily guarded/rate-limited/loop-detected), non-telemetry triggers (`shadow_report`/lifecycle/shell-adjacent events, extending `event` in §1.2), a constrained visual DSL/wizard authoring mode for non-coders (compiling to the same output contract, per §2.1), DO-mirrored derived telemetry (closing the §1.3 MVP punt), per-tenant CPU/cost visibility in the dashboard (mirroring how Cloudflare itself meters WfP). | Genuinely open-ended — scope each item independently once Phase 1/2 usage data exists. |

**Deliberately punted out of MVP** (stated plainly, not left implicit): shadow write-back,
any non-telemetry trigger, DO-mirrored derived telemetry (latest-value dashboard view lags
behind history/graphs for rule-derived keys), dry-run/testing UI, egress of any kind, a
visual/DSL authoring mode, shared alert-debounce state for rule output.

## 8. Decision points only Justin can make

1. **Ship Phase 0 (expression-language, no WfP) before Phase 1 (real WfP) at all**, given
   the $25/month floor is a real pre-revenue cost (§6.4), or go straight to WfP MVP and
   accept that cost now? This is the single highest-leverage timing call in this doc.
2. **Rule scope: flock-first (recommended, §6.4's script-count argument), pigeon-first, or
   both from day one** (mirroring `AlertScope`'s existing dual-scope precedent)? Per-pigeon
   is more natural for firmware-bring-up-style single-device experimentation; per-flock is
   cheaper at fleet scale.
3. **Should the whole feature be gated behind a paid plan from day one**, given Workers for
   Platforms' flat monthly fee has no free-tier-shaped on-ramp, or is a small included quota
   in a hypothetical free tier worth eating the $25/month for regardless of adoption?
4. **Authoring language for MVP**: plain JS only (recommended, §2.1), or insist on a
   constrained DSL/wizard from day one for non-coding users (closer to TB parity, slower to
   ship)?
5. **Rule-emitted alerts: fire-and-forget from day one (recommended, §1.3), or require
   shared debounce state (`rule_alert_state`) before shipping at all**, trading a slower
   MVP for guaranteed no-spam behavior from the first release?
6. **Timing relative to the rest of the in-flight roadmap** — is task #12 meant to start
   now, alongside the many other concurrently in-flight efforts across this codebase, or is
   this doc meant to sit as a reference until a dedicated work block opens up?

## 9. Summary of recommendations

1. **Hook point**: a new best-effort `dispatch_rule` branch alongside `check_telemetry_alerts`
   at its existing three call sites (`store_and_alert` + two no-queue fallbacks) — not a new
   consumer, not a `queue.rs`-only design (§1.1).
2. **MVP needs no outbound Worker**: `subRequests: 0` in the dispatch call's custom limits
   makes "no egress" free, since a rule's only output channel is its `Response` body, read
   by the dispatch Worker itself (§1.3, §4.2).
3. **Storage**: Postgres-only (`rule_definitions` + `rule_runs`), R2 content-addressed
   script storage mirroring `FirmwareImage` exactly, dovecote-owned version history since
   WfP provides none natively (§2.2, §3).
4. **Scope rules per-flock by default**, for script-count/cost reasons specific to this
   product's device-per-tenant ratio (§6.4), not per-pigeon.
5. **Failure semantics inherit the codebase's one universal convention**: best-effort,
   logged, never blocking ingestion or the device's own response — already proven at this
   exact seam by the shipped alerts feature (§5).
6. **A genuine Phase 0 (hosted expression language, no WfP purchase) is worth taking
   seriously**, not just as a rejected alternative, given the $25/month platform fee is a
   real pre-revenue cost with no usage-based on-ramp (§6.3, §6.4, §9 item 1).
7. **Shadow write-back, non-telemetry triggers, and a visual DSL are all explicitly Phase 3
   or later** — arbitrary user code writing device config is a real, argued exclusion from
   MVP (§1.3.1), not an oversight.

---

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
