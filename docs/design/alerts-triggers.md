# Alerts & triggers: user-defined notifications on telemetry/device state (task #32)

Status: design doc, no code changes. Scope: let a dashboard user define a condition
("battery_mv < 3300", "this pigeon has been Offline for 30 minutes", "no report in
2 hours") and get notified — email first, delivery kept pluggable so other channels
(webhook, SMS, push) are additive later, not a rewrite.

This is the concrete answer to the "alarms+notifications" gap in PidgeIoT's
ThingsBoard-parity list (product-strategy notes, 2026-07-20) — see §5 for how it
relates to the also-planned Workers-for-Platforms rule engine (referred to
throughout as "task #12"; note up front, from auditing this repo while writing this
doc, that no task #12 exists anywhere in dovecote/fancier/capsules source, git
history, or `docs/` — it is not yet a filed, numbered task in this codebase, only a
direction captured in the user's own planning notes. Treated here as a forward
reference, not prior art).

Grounded in: `dovecote/src/queue.rs`, `dovecote/src/helpers/telemetry.rs` +
`greptime.rs`, `dovecote/src/objects/pigeons.rs` (telemetry write paths),
`dovecote/src/helpers/auth.rs`, `fancier/src/helpers/connection_state.rs`,
`capsules/src/lib.rs`, `init-db.sql`, `dovecote/wrangler.toml`, and
`docs/design/tenancy-isolation.md` (the only other doc in `docs/design/`, used here
as the house style/structure to match).

## 0. Terminology note

Fancier already has a generic UI toast component named `Alert`
(`fancier/src/components/alert.rs`, `AlertVariant` in `fancier/src/models/`) —
unrelated to IoT alerting, purely a dismissible banner. Every domain type
introduced below is named `AlertDefinition`/`AlertCondition`/`AlertEvent` etc., not
bare `Alert`, specifically so `use crate::models::AlertVariant` and a future
`use capsules::AlertDefinition` never collide by import name in the same fancier
file.

## 1. Alert definition model

### 1.1 Condition types

| Condition | Inputs | Evaluated from |
|---|---|---|
| Threshold | telemetry `key`, comparator (`>`, `<`, `>=`, `<=`, `==`), numeric `value` | the metric just written (needs `value_num`, i.e. `TelemetryHistoryPoint`'s existing numeric-parse convention — `capsules::TelemetryHistoryPoint::value_num`, already `None` for non-numeric values per `helpers/telemetry.rs::write_telemetry_history`) |
| Device-state | target state (`Offline`, `Stale`), optional minimum duration in that state | `fancier::helpers::connection_state::classify` — see §1.3, this needs to move |
| Rate-of-change | telemetry `key`, delta threshold, window (e.g. "dropped more than 500 in 5 minutes") | current value vs. the previous reported value for the same key |
| Missing-report / heartbeat | optional specific `key` (defaults to "any telemetry from this pigeon") | absence of a signal within an expected window — cannot be ingest-triggered, see §2.2 |

All four share one shape: a boolean predicate over "this pigeon's (or this flock's)
observable state," differing only in what data they read and when they can be
evaluated (§2). `capsules` sketch:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AlertCondition {
  Threshold { key: String, comparator: Comparator, value: f64 },
  DeviceState { state: ConnectionStateKind, min_duration_secs: Option<i64> },
  RateOfChange { key: String, delta: f64, window_secs: i64 },
  MissingReport { key: Option<String>, window_secs: i64 },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Comparator { Gt, Gte, Lt, Lte, Eq }

// Mirrors fancier::helpers::connection_state::ConnectionState today, minus
// Unknown (an alert on "we've never heard from this pigeon" is really
// MissingReport, not DeviceState — see §1.3).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum ConnectionStateKind { Offline, Stale }
```

### 1.2 Scope: per-pigeon vs. per-flock

Both, via an `enum AlertScope { Pigeon(String), Flock(Uuid) }` on the definition
(mutually exclusive, mirrors how `Connector`/`TelemetryEndpoint` are already
per-pigeon while `FirmwareImage` is already per-flock in this same codebase — scope
is not a new concept here). A flock-scoped alert (e.g. "any pigeon in this flock
drops offline") evaluates once per relevant event across every pigeon currently in
`flocks.pigeon_ids`, firing/clearing independently per pigeon it applies to (one
`alert_state` row per `(definition_id, pigeon_id)`, §2.3) — not one combined
state for the whole flock, so five pigeons going offline is five clear
notifications, not one ambiguous one. This matches how `query_telemetry_history_for_flock`
already treats a flock as "the set of pigeons owned under it," not a single entity
with its own telemetry.

### 1.3 `connection_state::classify` needs to move to `capsules`

`fancier/src/helpers/connection_state.rs`'s `classify`/`telemetry_interval_secs`/
`format_last_seen` (lines 97–178) have **zero Dioxus or `web_sys` dependencies
today** — only `serde_json`, `time::OffsetDateTime`, and `capsules::{JsonString,
TelemetryHistoryPoint}`. It is already, structurally, shared-logic-shaped; it just
lives in the wrong crate for a backend evaluator (`objects/pigeons.rs`'s scheduled
sweep, §2.2) to reuse it. `capsules` is explicitly documented (root `CLAUDE.md`) as
"free of Worker- or Dioxus-specific dependencies" — the natural home. Recommend
moving `classify`/`telemetry_interval_secs`/`format_last_seen`/`latest_of` (and
their 15 existing unit tests) into `capsules`, with `fancier`'s module becoming a
thin re-export (`pub use capsules::connection_state::*;` plus its own
`badge_class`/`status_class`/`label` methods on `ConnectionState`, which *do* stay
in `fancier` since those return DaisyUI class strings — a UI concern). One
function, two callers (the badge and the alert evaluator), not two forks of the
same threshold math drifting apart over time.

Note `classify`'s `Unknown` variant is deliberately **not** reused for the
`DeviceState` alert condition (§1.1) — "never seen" is exactly what
`MissingReport` already models, and it needs different semantics anyway (an
`Unknown` pigeon has no `interval_secs` to compute an age against; a `MissingReport`
alert's `window_secs` is user-set, not derived from `telemetry_interval`).

### 1.4 Persistence: Postgres-only, not DO-mirrored

Following the reasoning `docs/design/tenancy-isolation.md` already applied to
`FirmwareImage` (§1.1 there: "Firmware images are shared across every pigeon in a
flock... flocks already have no DO of their own... this catalog lives purely in
Postgres") — the same argument applies near-verbatim here, and more strongly:

- A flock-scoped alert has no DO to live in at all (flocks have none).
- The dashboard's natural view is "list every alert for this flock" or "list every
  alert for this pigeon" — cross-pigeon querying, `tenancy-isolation.md`'s own
  definition of "Postgres territory."
- Unlike `pigeon_shadow`/`pigeon_acl`/`connector` (per-pigeon *device* config the
  DO must own so device round-trips never leave its own SQLite), an alert
  definition is dashboard-authored config with no device-facing counterpart — no
  device ever reads it, so there's no "must be available even if Postgres/Hyperdrive
  is down" requirement forcing DO-authoritative storage the way device config has.

Recommend a new Postgres table (add to `init-db.sql`, same idempotent
`CREATE TABLE IF NOT EXISTS` + trigger convention every other table there uses):

```sql
CREATE TABLE IF NOT EXISTS alert_definitions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL,               -- owner, same convention as flocks.user_id
  flock_id UUID REFERENCES flocks(id) ON DELETE CASCADE,
  pigeon_id TEXT REFERENCES pigeons(id) ON DELETE CASCADE,
  -- exactly one of flock_id/pigeon_id is set (CHECK constraint), mirrors
  -- AlertScope being an enum, not two independent optional fields
  name TEXT NOT NULL,
  condition JSONB NOT NULL,            -- serialized AlertCondition
  severity TEXT NOT NULL DEFAULT 'warning',  -- 'warning' | 'critical', see §2.3
  channel JSONB NOT NULL,              -- serialized AlertChannel, see §3
  enabled BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_alert_definitions_pigeon ON alert_definitions (pigeon_id) WHERE pigeon_id IS NOT NULL;
CREATE INDEX idx_alert_definitions_flock ON alert_definitions (flock_id) WHERE flock_id IS NOT NULL;
```

`condition`/`channel` as `JSONB` (not columns per condition-type field) matches
this codebase's existing convention for polymorphic config (`connector`,
`telemetry_endpoint` are both stored the same way — serialize the enum, deserialize
on read, no per-variant columns). `AlertDefinitionRow`/`AlertDefinition` in
`capsules` would follow the same `*Row`-with-i64-timestamps /
public-with-`OffsetDateTime` split every other entity in that crate already uses.

## 2. Evaluation point

### 2.1 The task's framing needs a correction, grounded in the actual code

The task description (and the natural first instinct) is "hook into `queue.rs`,
it already sees every telemetry write." **That premise doesn't hold** — audited
every telemetry ingestion path while writing this doc:

- `dovecote/wrangler.toml`'s `[env.dev]` block has **no `[[queues.producers]]` /
  `[[queues.consumers]]` at all** — only prod and `[env.staging]` do. Dev never
  goes through `queue.rs`.
- Even where a queue *is* bound (staging/prod), the WebSocket `telemetry` frame
  (`objects/pigeons.rs::handle_ws_telemetry`, the task #33-landed device path,
  hardware-verified 2026-07-20) writes directly — it never touches
  `TELEMETRY_QUEUE` at all.
- The direct-HTTP fallback (`report_telemetry_device`, used whenever no queue is
  bound for that environment) also never touches the queue.

So `queue.rs::dispatch_to_do` is the queue-consumer path for exactly one of three
ingestion routes, and it's the *only one of the three unavailable in dev*. The
actual convergence point — confirmed by reading the code, not assumed — is one
function level lower: all three routes call `upsert_telemetry` inside the `Pigeons`
DO (`write_telemetry_device` for the queue-consumer's DO-internal write,
`report_telemetry_device` for the no-queue HTTP fallback, `handle_ws_telemetry` for
the WS frame), and all three then call `write_telemetry_default(env, pigeon_id,
&metrics, reported_at_ms)` immediately after — whose own doc comment already says
this explicitly: *"Shared by all three 'no per-pigeon `telemetry_endpoint`
override' write sites... so all three keep behaving identically, which was the
whole point of task #17."* That sentence is describing exactly the choke point an
alert evaluator needs, already built and already proven stable across all three
paths — it just doesn't do alert evaluation yet.

### 2.2 Recommendation: threshold/rate-of-change eval alongside `write_telemetry_default`, not inside `queue.rs`

Add a sibling call, `check_telemetry_alerts(env, pigeon_id, &metrics,
reported_at_ms)`, at the same three call sites `write_telemetry_default` already
has (`queue.rs::dispatch_to_do`, `objects/pigeons.rs::report_telemetry_device`,
`objects/pigeons.rs::handle_ws_telemetry`), rather than only inside `queue.rs`.
This gets WS + direct-HTTP + queued coverage for free, and dev-environment parity
(no separate "dev has no alerts" carve-out) — matching this codebase's existing
precedent of literally sharing `write_telemetry_default` across the same three
sites for the same reason.

Two of the three call sites are already inside the `Pigeons` DO (`report_telemetry_device`,
`handle_ws_telemetry`), and `write_telemetry_history`'s successful Postgres access
from inside the DO (`pigeons.env` passed straight into `get_db_client(env)`,
already working in production for the PG-history fallback) confirms the DO's
`env` already carries the Hyperdrive binding — so **no new DO-to-Postgres plumbing
is needed**; a `SELECT * FROM alert_definitions WHERE pigeon_id = $1 OR flock_id =
$2 AND enabled` from inside the DO (or from `queue.rs` at the top level, for that
third call site) is the same kind of round-trip this codebase already makes at
this exact point in the request lifecycle.

Cost/latency: `report_telemetry_device` and `handle_ws_telemetry`'s response to
the device is a plain `Response::from_json`/frame ack that doesn't currently wait
on `write_telemetry_default`'s *result* (errors are logged, not surfaced) — extending
that same "fire, log on failure, never block/fail the primary write" convention
(this codebase's one universal rule for cross-store sync, per root `CLAUDE.md`) to
alert evaluation keeps the device's own request latency unaffected; the extra
Postgres round-trip only delays how quickly the *notification* goes out, never the
device's own ack.

**Rate-of-change's "previous value" is the one wrinkle**: at the queue-consumer
call site (top-level Worker, no direct DO SQL cursor), the only value in hand is
the metrics map just written — the *previous* latest value isn't available without
another read. Recommend sourcing it the same way `GET /pigeons/:id/telemetry/history`
already does: one more read against whichever store is authoritative for this
pigeon (Greptime if `GREPTIMEDB_ENDPOINT` is configured, else
`pigeon_telemetry_history` — the existing `query_greptime_history_for_pigeon`/
`query_telemetry_history_for_pigeon` machinery, just called for one key with a
`LIMIT 2`-shaped query instead of a full range). This is strictly a secondary,
best-effort read for one alert *type*, not a requirement on every telemetry write —
threshold and device-state conditions need no such lookup.

### 2.3 Debounce/hysteresis + fired-state tracking (mirrors ThingsBoard alarm semantics)

Per the product-strategy notes' framing of ThingsBoard's "alarms" (raise/clear
lifecycle, severity) as the gap being closed: a condition crossing `true` once
must not re-notify on every subsequent evaluation while it stays true, and a
resolution should itself notify once. Add `alert_state`:

```sql
CREATE TABLE IF NOT EXISTS alert_state (
  alert_definition_id UUID NOT NULL REFERENCES alert_definitions(id) ON DELETE CASCADE,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'ok',      -- 'ok' | 'firing'
  first_true_at TIMESTAMPTZ,               -- when the condition first became true this episode
  last_notified_at TIMESTAMPTZ,
  PRIMARY KEY (alert_definition_id, pigeon_id)
);
```

State machine, evaluated on every check: `ok → firing` only once the condition has
been continuously true for the definition's own debounce window (reuse the
existing `telemetry_interval`-relative multiplier idea from
`connection_state::classify` rather than a fixed constant, so a fast-reporting
pigeon debounces faster than a slow one) — send the "fired" email exactly on that
transition, not again while still firing (optional periodic re-notify after a
configurable cooldown, off by default, for a "still down after N hours" nag).
`firing → ok` sends a single "cleared" email on the reverse transition. `severity`
(`warning`/`critical` on `AlertDefinition`, §1.4) is carried through to the email
subject/badge color, reusing the same `badge-warning`/`badge-error` visual
language `connection_state::badge_class` already established rather than inventing
new color semantics.

### 2.4 Missing-heartbeat needs a separate scheduled evaluator

Absence-of-data cannot be triggered by an ingest event by definition — nothing
arrives to trigger it. This needs a Cloudflare Cron Trigger (`[triggers] crons =
[...]` in `wrangler.toml` + a new `#[event(scheduled)]` handler) — **confirmed
this doesn't exist anywhere in this codebase yet** (no `[triggers]` block in
`dovecote/wrangler.toml` today), so it's new infrastructure, not an extension of
an existing cron.

Design: run every few minutes, query Postgres for every `alert_definitions` row
with a `MissingReport` condition (`enabled = true`), resolve each to its
scope's pigeon(s), and check "last seen" the same way the flock pigeon-list view
already does — `query_telemetry_history_for_flock` (or a new equivalent scoped
query) feeding `capsules::connection_state::latest_seen_by_pigeon` (post-move,
§1.3) — rather than fanning out to every pigeon's own DO individually (expensive
at fleet scale, and exactly the kind of per-DO fan-out `tenancy-isolation.md`
already flagged as a cost worth avoiding for the analogous per-flock-database
Greptime option). Feed the result through the same `alert_state`
transition/debounce logic as §2.3 — a missing-heartbeat alert is a `DeviceState`-
shaped condition operationally, just polled on a timer instead of triggered on
write.

## 3. Email delivery decision

### 3.1 Options considered

| Option | Mechanism from a Worker | Verdict |
|---|---|---|
| **Cloudflare Email Routing send API** | `send_email` binding (`[[send_email]]` in `wrangler.toml`) | Before onboarding a sending domain, can only send to pre-verified destination addresses in the account — not arbitrary end-user emails, which is exactly what alert recipients are. After onboarding a sending domain it can send anywhere, but it's a **binding**, declared in `wrangler.toml`, not a runtime secret — swapping providers later means editing deploy config and code together, not just a secret, which cuts against "delivery kept pluggable." Also entangles "send alert email" with the zone's actual inbound Email Routing config, a separate concern. |
| **Resend** | Plain `fetch()` POST to `api.resend.com`, `Authorization: Bearer <key>` | Purpose-built transactional email HTTP API; Cloudflare's own current Workers docs recommend it by name for exactly this integration. Free tier: 100/day, 3,000/month — ample for a debounced alert feature. One secret, no binding, no zone entanglement. |
| **SendGrid** | `fetch()` to SendGrid's HTTP API, API-key header | Viable, same shape as Resend, heavier product surface (marketing/campaign features unneeded here). |
| **AWS SES** | HTTP API needs SigV4-signed requests (no Rust SigV4 crate is currently a dependency anywhere in this workspace) — SMTP is an alternative but see the "no SMTP client from Workers" note below | Doable but the extra signing-machinery cost buys nothing over a plain bearer token for this use case. |
| **MailChannels** | Was free for Cloudflare Workers | Free tier for Workers ended — no longer a zero-cost default; not evaluated further. |

### 3.2 Recommendation: Resend, called via a plain HTTP secret, following the existing `greptime.rs` pattern exactly

```rust
// helpers/alerts.rs — new, mirrors greptime.rs's secret-reading shape verbatim
fn resend_api_key(env: &Env) -> Option<String> {
  env.secret("RESEND_API_KEY").ok().map(|v| v.to_string()).filter(|s| !s.trim().is_empty())
}
```

Set via `wrangler secret put RESEND_API_KEY --env <env>`, documented in
`wrangler.toml`'s comment block the same way `GREPTIMEDB_AUTH_TOKEN`/
`GREPTIMEDB_ACCESS_CLIENT_*` already are — never a `[vars]` entry, same rule this
codebase already enforces for every credential. Delivery stays pluggable by
putting the actual send behind a small trait/enum (`AlertChannel::Email { .. }`
today; `AlertChannel::Webhook { .. }` etc. later are new variants, not a rewrite),
matching how `TelemetryEndpoint` already lets a per-pigeon URL swap out the
platform's own Greptime target without touching the write-path shape.

### 3.3 Overlap with task #33 (Kratos branded email templates) — same provider, deliberately separate sending path

Audited the in-flight #33 work (commit `a13cfb6`, currently only on
`worktree-agent-acff60bd411a45ee9`, not yet merged to `main` or present in this
worktree): it reskins **Kratos's own courier** — recovery/verification flow emails,
templated via `schemas/kratos/courier-templates/v1/.../file://` URIs wired into
`kratos.yml`'s `courier.templates.*`. Kratos's courier is **SMTP-only**
(`courier.smtp.connection_uri`, `smtps://test:test@mailslurper:1025` in dev) and
scoped entirely to Kratos's own self-service flows — there is no generic "send
arbitrary email content" endpoint on Kratos a Worker could call to reuse that path
for alert notifications. Sharing the literal sending *path* isn't feasible without
building bespoke Kratos webhooks, which is real scope creep for no benefit here.

Recommend instead: **share the provider, not the path or credential.** Point
Kratos's `courier.smtp.connection_uri` at Resend's SMTP relay
(`smtp.resend.com:587`, one Resend SMTP credential) for its own recovery/
verification emails, while dovecote's alert code calls Resend's HTTP API directly
with its own separate `RESEND_API_KEY` Worker secret. One provider account, one
verified sending domain (`pidgeiot.com`), one SPF/DKIM/DMARC setup and one place to
check deliverability — but two independent credentials, so a compromised or
rate-limited Worker secret can't affect Kratos's courier and vice versa (same
blast-radius-scoping principle this codebase already applies to per-purpose
secrets like the Greptime Access service-token pair being separate from
`GREPTIMEDB_AUTH_TOKEN`). Prod has no SMTP relay configured for Kratos yet either
(confirmed: only dev's `mailslurper` exists anywhere in this repo) — so this is a
recommendation for whoever finishes #33's prod rollout to land alongside it, not a
retrofit of something already decided.

### 3.4 Recipient resolution: no plumbing for this exists today — needs a small, specific addition

Audited this directly: `dovecote/src/helpers/auth.rs::authenticate_browser` calls
only Kratos's browser/self-service `to_session` (`frontend_api`), never the admin
API. `require_auth` (`lib.rs:68`) resolves a session down to `identity.id` and
**discards `identity.traits` entirely** — the email is fetched on every
authenticated request already and thrown away. `flocks`/`capsules::Flock` have no
email column (`init-db.sql:27-34`, confirmed no `email` anywhere in that file or
in `capsules::Flock`). No `KRATOS_ADMIN_URL` exists in `wrangler.toml`, and the
Kratos admin API (port 4434) is reachable only within the dev docker-compose
network today, not something a Cloudflare Worker (running at Cloudflare's edge,
not on the user's own network) could currently reach for staging/prod without new
tunnel/Access infrastructure mirroring what `GREPTIMEDB_ENDPOINT` already needed.

Recommend **denormalizing email onto `flocks` (a new `owner_email TEXT` column),
populated from data the request handler already has in hand** — no new admin-API
call, no new tunnel: extend `require_auth` (or add a sibling) to also return
`identity.traits`'s email (`session.identity.traits` is `Option<serde_json::Value>`,
already deserialized off the wire on every authenticated request; only the `.id`
field is kept today) and opportunistically upsert it onto that user's flock(s) —
cheapest hook is at flock creation (`create_user_flock`, `helpers/flocks.rs:50`,
already takes `user_id_str`; extend to take the email too) plus a periodic
refresh-on-any-authenticated-request to tolerate the user changing their email
later via Kratos's own settings flow. This accepts short-lived staleness (bounded
by how often that user's session gets validated again) rather than adding new
attack surface (an admin API reachable from the edge) for what's a rare, low-risk
staleness window. If staleness ever proves to be a real problem, a just-in-time
admin-API lookup at alert-fire time (rare event, unlike every telemetry write) is
the natural upgrade path later — but it is not needed to ship v1.

## 4. `fancier` UI sketch

New route, following the existing `Route` enum's nesting convention
(`fancier/src/lib.rs`): `#[route("/flocks/:flock_id/pigeons/:pigeon_id/alerts")]`
for per-pigeon alerts, plus a flock-level alerts tab alongside the existing
`/flocks/:flock_id/pigeons` list for flock-scoped ones. Both list views are just
another `LocalSession`-cached fetch (`api/alerts.rs`, one file per resource, same
shape as `api/pigeons.rs`/`api/flocks.rs`) — no new state-management pattern
needed.

- **List**: table of `AlertDefinition`s (name, condition summary, severity badge
  reusing `connection_state`'s `badge-warning`/`badge-error` classes, enabled
  toggle, last-fired timestamp from `alert_state`). Empty/loading states follow
  the same convention as the telemetry graph section (`views/pigeon.rs`'s
  Live/Empty/Preview pattern).
- **Create/edit**: a form, not a native `<dialog>` — given `EditShadowModal`'s
  precedent of using a signal-gated conditional-render instead of `<dialog>` for
  anything holding reset-sensitive state, this form (condition type + its
  type-specific fields + channel) qualifies the same way. Condition-type picker
  drives which fields render (`Threshold` shows key+comparator+value;
  `DeviceState` shows a state dropdown + optional duration; `RateOfChange` shows
  key+delta+window; `MissingReport` shows optional key + window) — a Dioxus
  `match` over `AlertCondition`'s variant, structurally identical to how
  `firmware_modal.rs` already conditionally renders based on which optional
  shadow key is present.
- **Channel**: v1 is just an email field defaulting to the flock owner's stored
  address (§3.4) with an override input for a different recipient, modeled as
  `AlertChannel::Email { to: Option<String> }` (`None` = use the resolved owner
  email) so the pluggability seam (§3.2) is visible in the UI from day one even
  though only one variant exists yet.
- **Delete**: reuse the `DeletePigeonModal` typed-confirm pattern only if the doc
  authors want extra friction here — arguably overkill for a config object with no
  data-loss blast radius the way deleting a pigeon has (device deauthorization,
  irreversible token loss); a plain confirm is probably sufficient, callout for
  whoever implements this to decide.

## 5. Relationship to the Workers-for-Platforms rule engine ("task #12")

Frame this feature as the first concrete, shippable slice of "user-defined logic
over telemetry," not a competing design. The seam is already visible in §2.2: the
recommended hook point (`check_telemetry_alerts`, called alongside
`write_telemetry_default` at the same three converged call sites) is exactly the
same injection point the product-strategy notes already identify for a future
user-worker dispatch ("dovecote's telemetry queue consumer is the injection
point... a user-worker dispatch becomes a third branch, or the generalization of
both"). Concretely:

- Today: `write_telemetry_default` branches PG-history vs. line-protocol-forward
  per pigeon. This doc adds a third, parallel best-effort branch:
  `check_telemetry_alerts`, evaluating a small, fixed set of condition types
  against a Postgres-stored definition.
- Later (task #12, if/when filed): a Workers-for-Platforms dispatch namespace
  would let a user author arbitrary logic in that same slot — of which "evaluate
  a threshold and call an email API" is one specific, hardcoded instance. The
  fixed `AlertCondition` enum (§1.1) is deliberately a **closed, structured**
  subset of what a general rule engine would allow (arbitrary code) — this is
  intentional narrowing, not a smaller version of the same abstraction, so #12
  can either subsume alerts entirely (a user-authored worker that reimplements
  §1's four condition types plus arbitrary others) or leave this feature as the
  "batteries-included, no-code" tier sitting alongside a "bring-your-own-logic"
  tier once #12 exists — either way, nothing here needs to be torn out, because
  the fixed conditions and the general dispatch hook occupy the same seam without
  one being built as a special case of the other's internals.
- `alert_definitions`/`alert_state` (§1.4, §2.3) are new Postgres tables uninvolved
  in dispatch-namespace mechanics either way — they'd remain the storage/state
  layer for the no-code tier regardless of whether #12 ships.

## 6. Summary of recommendations

1. **Definitions**: Postgres-only (`alert_definitions` + `alert_state`), not
   DO-mirrored — same reasoning `tenancy-isolation.md` already applied to
   `FirmwareImage`. `capsules::AlertCondition`/`AlertScope`/`AlertChannel` enums,
   JSONB-serialized, matching the `connector`/`telemetry_endpoint` convention.
2. **Evaluation point**: NOT `queue.rs` alone (it misses dev entirely and misses
   WS-telemetry always) — hook `check_telemetry_alerts` alongside
   `write_telemetry_default` at all three of its existing call sites instead.
   Missing-heartbeat needs a genuinely separate, new Cron-Trigger-driven scheduled
   evaluator (no cron infrastructure exists in this codebase today).
3. **`connection_state::classify` and friends move to `capsules`** — already
   dependency-clean, needed by both the fancier badge and the new backend
   evaluator/scheduled sweep.
4. **Email provider: Resend**, called via plain `fetch()` + a `RESEND_API_KEY`
   Worker secret, following the `greptime.rs` secret pattern exactly — not
   Cloudflare's own Email Routing send binding (verified-destination-only until a
   sending domain is onboarded, and a deploy-config binding rather than a runtime
   secret, cutting against pluggability). Share the **provider** with task #33's
   Kratos courier (both on Resend, one verified sending domain) but not the
   sending path or credential — Kratos's courier is SMTP-only and flow-scoped, no
   arbitrary-send endpoint exists to share.
5. **Recipient email**: no existing plumbing (`identity.traits` is fetched then
   discarded today, no admin API call anywhere in dovecote) — denormalize onto
   `flocks.owner_email`, populated from the session data `require_auth` already
   has in hand rather than adding a new Kratos-admin-API dependency for v1.
6. **Relationship to #12**: this is the closed-condition-set, no-code tier; the
   evaluation hook point is the same seam a future user-worker dispatch would use,
   so neither design blocks or duplicates the other.

---

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
