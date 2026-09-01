# Dovecote API Reference

`dovecote` is PidgeIoT's edge router (Cloudflare Workers + Durable Objects). This document
covers its entire HTTP surface: the **dashboard API**, used by `fancier` (and anything else
acting on a human's behalf), and the **device API**, used by pigeons (embedded devices) to
report in and pull configuration.

Every route on this page is derived directly from `dovecote/src/lib.rs` (the gateway router)
and `dovecote/src/objects/pigeons.rs` (the Durable Object it proxies to). Request/response
shapes reference the shared types in `capsules/src/lib.rs` — that crate is the single source of
truth for wire formats; this document just explains how they're used over HTTP.

- **Base URL (production):** `https://api.pidgeiot.com`
- **Base URL (staging):** `https://api-staging.pidgeiot.com`
- **Base URL (local dev):** `http://127.0.0.1:8787`

All examples below use placeholder IDs and credentials — `<pigeon_id>`, `<flock_id>`,
`<device_token>`, etc. Never substitute real secrets into a shared document or commit history.

## Routes at a glance

Every route in this document, in the order it appears below. The **Auth** column uses the
vocabulary defined in [Two audiences, two auth models](#two-audiences-two-auth-models): a
dashboard route's marker names the role it needs on top of a valid session, and
`device token` means a per-pigeon Ed25519 bearer token instead of a session.

| Route | Auth | What it does |
|---|---|---|
| [`GET /flocks`](#get-flocks) | session | List the flocks the caller can see |
| [`POST /flocks`](#post-flocks) | session | Create a personal flock |
| [`DELETE /flocks/:flock_id`](#delete-flocksflock_id) | flock: manage | Delete a flock that holds no pigeons |
| [`POST /flocks/:flock_id/transfer`](#post-flocksflock_idtransfer) | flock owner + target org owner/admin | Move a personal flock into an organization |
| [`POST /orgs`](#post-orgs) | session | Create an organization, caller as its owner |
| [`GET /orgs`](#get-orgs) | session | List the caller's organization memberships |
| [`GET /orgs/:org_id`](#get-orgsorg_id) | any member | Read one org with its members and pending invites |
| [`PUT /orgs/:org_id`](#put-orgsorg_id) | owner/admin | Rename an org, set its timezone, or both |
| [`DELETE /orgs/:org_id`](#delete-orgsorg_id) | owner | Delete an org that owns no flocks |
| [`PUT /orgs/:org_id/members/:user_id`](#put-orgsorg_idmembersuser_id) | owner | Change a member's role |
| [`DELETE /orgs/:org_id/members/:user_id`](#delete-orgsorg_idmembersuser_id) | owner/admin, or self | Remove a member, or leave the org |
| [`POST /orgs/:org_id/invites`](#post-orgsorg_idinvites) | owner/admin | Invite an email address at a role |
| [`GET /orgs/:org_id/invites`](#get-orgsorg_idinvites) | owner/admin | List pending invites |
| [`DELETE /orgs/:org_id/invites/:invite_id`](#delete-orgsorg_idinvitesinvite_id) | owner/admin | Revoke a pending invite |
| [`POST /invites/accept`](#post-invitesaccept) | session | Join an org by redeeming an invite token |
| [`GET /orgs/:org_id/business-details`](#get-orgsorg_idbusiness-details) | any member | Read the invoicing name and tax registration |
| [`PUT /orgs/:org_id/business-details`](#put-orgsorg_idbusiness-details) | owner/admin | Replace the invoicing name and tax registration |
| [`GET /orgs/:org_id/billing`](#get-orgsorg_idbilling) | org: member | Read plan, entitlement and usage against allowance |
| [`POST /orgs/:org_id/billing/checkout`](#post-orgsorg_idbillingcheckout) | org: manage | Mint a Stripe Checkout URL for a paid tier |
| [`POST /orgs/:org_id/billing/portal`](#post-orgsorg_idbillingportal) | org: manage | Mint a Stripe billing portal URL |
| [`PUT /orgs/:org_id/billing/plan`](#put-orgsorg_idbillingplan) | org: manage | Move a live subscription to another tier |
| [`POST /billing/webhook`](#post-billingwebhook) | Stripe signature required | Stripe's event sink; not a dashboard route |
| [`POST /flock/pigeons`](#post-flockpigeons) | flock: manage | Provision a pigeon and mint its device credentials |
| [`GET /pigeons/:pigeon_id`](#get-pigeonspigeon_id) | member | Read one pigeon, connector secrets stripped |
| [`GET /pigeons/:pigeon_id/detail`](#get-pigeonspigeon_iddetail) | member | Read one pigeon plus the caller's ACL row and shadow |
| [`PUT /pigeons/:pigeon_id`](#put-pigeonspigeon_id) | member | Partially update a pigeon |
| [`PUT /pigeons/:pigeon_id/flock`](#put-pigeonspigeon_idflock) | owner + destination flock: manage | Move a pigeon into another flock of the same owner |
| [`DELETE /pigeons/:pigeon_id`](#delete-pigeonspigeon_id) | owner | Deprovision a pigeon and wipe its storage |
| [`POST /pigeons/batch`](#post-pigeonsbatch) | member (per pigeon) | Fetch up to 48 pigeons by id in one request |
| [`POST /pigeons/:pigeon_id/token/refresh`](#post-pigeonspigeon_idtokenrefresh) | owner | Mint a new keypair, revoking the current token |
| [`POST /pigeons/:pigeon_id/shell`](#post-pigeonspigeon_idshell) | owner | Run one diagnostic command on a connected device |
| [`GET /pigeons/:pigeon_id/acl`](#get-pigeonspigeon_idacl) | owner | List every ACL entry on a pigeon |
| [`POST /pigeons/:pigeon_id/acl`](#post-pigeonspigeon_idacl) | owner | Grant or change one ACL entry |
| [`GET /pigeons/:pigeon_id/shadow`](#get-pigeonspigeon_idshadow) | member | Read the desired/reported config pair |
| [`PUT /pigeons/:pigeon_id/shadow`](#put-pigeonspigeon_idshadow) | member | Set a new target config; also assigns firmware |
| [`POST /flocks/:flock_id/firmware`](#post-flocksflock_idfirmware) | flock: manage | Upload an image into the flock's firmware catalog |
| [`GET /flocks/:flock_id/firmware`](#get-flocksflock_idfirmware) | flock: view | List the flock's firmware catalog |
| [`GET /pigeons/:pigeon_id/telemetry`](#get-pigeonspigeon_idtelemetry) | member | Read the latest value per telemetry key |
| [`GET /pigeons/:pigeon_id/telemetry/history`](#get-pigeonspigeon_idtelemetryhistory) | member | Query one pigeon's telemetry history |
| [`GET /flocks/:flock_id/telemetry/history`](#get-flocksflock_idtelemetryhistory) | flock: view | Query telemetry history across a whole flock |
| [`PUT /pigeons/:pigeon_id/telemetry-endpoint`](#put-pigeonspigeon_idtelemetry-endpoint) | member | Set or clear a line-protocol forwarding target |
| [`GET /pigeons/:pigeon_id/logs`](#get-pigeonspigeon_idlogs) | member | Download the stored device log chunks |
| [`PUT /pigeons/:pigeon_id/log-dictionary`](#put-pigeonspigeon_idlog-dictionary) | member | Upload the firmware's log dictionary |
| [`GET /pigeons/:pigeon_id/log-dictionary`](#get-pigeonspigeon_idlog-dictionary) | member | Read the stored log dictionary |
| [`DELETE /pigeons/:pigeon_id/log-dictionary`](#delete-pigeonspigeon_idlog-dictionary) | member | Remove the stored log dictionary |
| [`POST /pigeons/:pigeon_id/alerts`](#post-pigeonspigeon_idalerts) | member | Create an alert scoped to one pigeon |
| [`GET /pigeons/:pigeon_id/alerts`](#get-pigeonspigeon_idalerts) | member | List a pigeon's own alert definitions |
| [`GET /pigeons/:pigeon_id/alerts/state`](#get-pigeonspigeon_idalertsstate) | member | Read fired/cleared state for a pigeon's alerts |
| [`POST /flocks/:flock_id/alerts`](#post-flocksflock_idalerts) | flock: manage | Create an alert covering a whole flock |
| [`GET /flocks/:flock_id/alerts`](#get-flocksflock_idalerts) | flock: view | List a flock's alert definitions |
| [`GET /flocks/:flock_id/alerts/state`](#get-flocksflock_idalertsstate) | flock: view | Read fired/cleared state per pigeon in a flock |
| [`PUT /alerts/:alert_id`](#put-alertsalert_id) | alert owner | Update an alert definition |
| [`DELETE /alerts/:alert_id`](#delete-alertsalert_id) | alert owner | Delete an alert definition |
| [`POST /feedback`](#post-feedback) | no auth required (optionally authenticated) | Send the in-app feedback form |
| [`POST /contact`](#post-contact) | no auth required (optionally authenticated) | Send the public contact form |
| [`POST /errors`](#post-errors) | no auth required (identity only on the manual JSON path) | Ingest a crash report, or a note about one |
| [`DELETE /errors`](#delete-errors) | session required | Erase the caller's identified error reports |
| [`GET /dashboard-state/:scope_key`](#get-dashboard-statescope_key) | session | Read the caller's saved document for one scope |
| [`PUT /dashboard-state/:scope_key`](#put-dashboard-statescope_key) | session | Replace the caller's document for one scope |
| [`DELETE /dashboard-state/:scope_key`](#delete-dashboard-statescope_key) | session | Drop the caller's document for one scope |
| [`GET /demo/pigeons/:pigeon_id/telemetry`](#get-demopigeonspigeon_idtelemetry) | none | Latest values for the public demo pigeon |
| [`GET /demo/pigeons/:pigeon_id/telemetry/history`](#get-demopigeonspigeon_idtelemetryhistory) | none | Telemetry history for the public demo pigeon |
| [`GET /demo/pigeons/:pigeon_id/alerts`](#get-demopigeonspigeon_idalerts) | none | The alert rules the demo page draws its lines from |
| [`GET\|HEAD /.well-known/api-catalog`](#gethead-well-knownapi-catalog) | no auth required | RFC 9727 linkset for capability discovery |
| [`GET /device/pigeons/:pigeon_id/shadow`](#get-devicepigeonspigeon_idshadow) | device token | Device reads the config it is meant to apply |
| [`POST /device/pigeons/:pigeon_id/shadow`](#post-devicepigeonspigeon_idshadow) | device token | Device reports the config it has applied |
| [`POST /device/pigeons/:pigeon_id/telemetry`](#post-devicepigeonspigeon_idtelemetry) | device token | Device reports readings, flat or batched |
| [`POST /device/pigeons/:pigeon_id/logs`](#post-devicepigeonspigeon_idlogs) | device token | Device uploads one binary log chunk |
| [`GET /device/pigeons/:pigeon_id/firmware`](#get-devicepigeonspigeon_idfirmware) | device token | Device downloads its assigned image, Range-aware |
| [`GET /device/pigeons/:pigeon_id/ws`](#get-devicepigeonspigeon_idws) | device token | Device opens its persistent WebSocket |
| [`GET /internal/device-psk/:pigeon_id`](#get-internaldevice-pskpigeon_id) | service secret required | Terminator resolves an identity to its PSK and token |
| [`POST /internal/consent`](#post-internalconsent) | service secret required | Kratos reports a marketing-consent change |

## Two audiences, two auth models

| | Dashboard API | Device API |
|---|---|---|
| Who calls it | `fancier`, or any browser-based client acting for a human | Pigeons (embedded devices) |
| Path prefix | `/flocks`, `/pigeons/*` | `/device/pigeons/*` |
| Credential | Ory Kratos session cookie | Per-pigeon Ed25519-signed bearer token |
| Sent as | `Cookie` header (`credentials: include` in `fetch`) | `Authorization: Bearer <token>` header |
| Identity granularity | One Kratos identity, scoped per-pigeon by an ACL | One keypair per pigeon; the token proves control of *that* pigeon and nothing else |

(There is also exactly one [service-internal route](#service-internal-api) — the CoAP
terminator's PSK lookup — authenticated by a shared service secret, fitting neither column.)

### Dashboard authentication (Kratos session cookie)

Dashboard routes call `require_auth` (`dovecote/src/lib.rs`), which validates the request's
`Cookie` header against Ory Kratos (`authenticate_browser`, `dovecote/src/helpers/auth.rs`) and
resolves it to a Kratos identity ID. That ID is forwarded to the owning pigeon's Durable Object
as an internal `X-User-Id` header — the DO never talks to Kratos itself; it just checks that ID
against its own local **ACL table** (`pigeon_acl`, one per pigeon, living inside that pigeon's
Durable Object — not a global table).

Every ACL row is `{ entity_id: <user UUID>, role: <string> }`. Only the literal role value
`"owner"` is special-cased server-side (`is_owner` in `objects/pigeons.rs`); any other role
string is accepted but is currently only meaningful as "has access" (`is_authorized` doesn't
distinguish between non-owner roles). A pigeon's creator is inserted as `"owner"` automatically
on creation. Routes below are marked **owner** (must hold the `"owner"` role) or **member**
(any ACL row for that pigeon is enough).

A request with no valid session cookie gets `401 Unauthorized`. A valid session with no ACL row
for the target pigeon gets `403 Forbidden`.

**Organizations (task #12).** Alongside the per-user ACL rows, a pigeon may carry an ACL row
whose `entity_id` is an **organization id** — that's how org-shared access works (no new
table). After validating the session, the gateway loads the caller's org memberships in one
Postgres query (`require_principal`, `dovecote/src/lib.rs`) and forwards them to the Durable
Object as an internal `X-Org-Roles` header (compact JSON, `[{"id":"<org uuid>","role":"owner"}]`)
alongside `X-User-Id`. The DO's single authorization helper
(`objects/pigeons.rs::authorize_dashboard`) then treats the caller as the principal set
`{user_id} ∪ {org ids}`: a per-user ACL row behaves exactly as before, and an org ACL row
grants rights capped by the caller's role *in that org* — see the
[permission matrix](#organizations) below. If the org-membership load fails (Postgres blip),
the request degrades to personal-only access rather than failing.

Flocks have no separate ACL table. A flock is **exactly one** of:

- **personal** (`org_id` null): governed by `flocks.user_id` alone, as before; or
- **org-owned** (`org_id` set): governed by the caller's role in that org —
  `flocks.user_id` becomes provenance (who created/transferred it), not an access grant.

Every gateway flock check funnels through one helper (`helpers/orgs.rs::authorize_flock`), so
routes below are marked **flock: view** (personal owner, or any org role) or **flock: manage**
(personal owner, or org owner/admin).

### Device authentication (bearer token)

Device routes (`/device/pigeons/:pigeon_id/*`) carry **no Kratos session at all** — a device has
no Kratos identity. Instead, each pigeon gets its own **Ed25519 keypair**, generated fresh
inside that pigeon's Durable Object on `POST /flock/pigeons` (create) and again on
`POST /pigeons/:pigeon_id/token/refresh`. Only the *public* key is ever persisted (in that DO's
own SQLite `pigeons.device_public_key` column — never mirrored to Postgres, never returned by
any API response). The private key signs one token and is discarded immediately.

**The token is not a JWT.** It's a 69-byte binary blob:

```
byte 0        version (currently always 1)
bytes 1..5    expires_at — u32, little-endian, unix seconds
bytes 5..69   Ed25519 signature over bytes 0..5
```

That blob is base64url-encoded (no padding) for transport and sent as
`Authorization: Bearer <token>`. Notably, **the token carries no subject/pigeon-id claim** — it
doesn't say which pigeon it belongs to. The binding comes entirely from *which pigeon's Durable
Object you send it to*: `verify_device_token` (`dovecote/src/objects/helpers.rs`) checks the
token's signature against that specific pigeon's stored public key. The same bytes mean nothing
against any other pigeon's DO.

**Refreshing a pigeon's token revokes the previous one.** `token/refresh` mints an entirely new
keypair and overwrites `device_public_key`, so the old token's signature can never verify again
— regardless of its own embedded `expires_at`. There's no separate revocation list; overwriting
the verification key *is* the revocation mechanism.

The token is returned in a pigeon's `connector.Https.token` (or `connector.Coap.token` /
`connector.Mqtt.token`, each alongside its `tls_psk_secret`) field, and **only** in the response to the route that just minted it — pigeon
create (`POST /flock/pigeons`) or token refresh (`POST /pigeons/:pigeon_id/token/refresh`).
Every other route that returns a `Pigeon` (`GET /pigeons/:id`, `GET /pigeons/:id/detail`,
`PUT /pigeons/:id`, `POST /pigeons/batch`) strips it to an empty string first
(`strip_secrets`, `objects/pigeons.rs`) — treat that field as write-once, read-never after the
initial mint.

A missing/malformed/expired/wrong-pigeon token gets `401 Unauthorized`, and so does a token for
a pigeon that has since been **deleted** — the Durable Object stays addressable with its tables
empty, and answers device routes as an unknown pigeon rather than as a fault, so a device (or a
protocol terminator holding a session for one) reads deprovisioning as permanent instead of
retrying through it.

> **Troubleshooting: `403` with an HTML body.** If a device request gets `403` and the body is
> HTML (e.g. "Just a moment...") instead of plain text, the request was stopped by edge
> security *before* reaching the API — your token was never even checked. Common triggers:
> requests from datacenter IP ranges, or a client `User-Agent` on bot blocklists (Python's
> default `Python-urllib/x.y` is a known offender). Set a distinctive `User-Agent` (e.g.
> `my-gateway/1.0`) and retry; if it persists from your network, report it — the device routes
> are meant to be exempt from browser-oriented challenges.

## CORS

Every route is wrapped in a per-request CORS response computed from the incoming `Origin`
header against that environment's `ROOT_URL` config var (`build_cors`, `dovecote/src/lib.rs`).
If `Origin` matches `ROOT_URL` exactly, that origin is echoed back with
`Access-Control-Allow-Credentials: true`; otherwise the response carries `ROOT_URL` as an inert
value that won't match the disallowed origin. `ROOT_URL` is `https://pidgeiot.com` in
production, the local `dx serve` address in dev, and the staging `fancier` preview URL in
staging. This only matters for browser callers — a non-browser client like `curl` or a device
firmware ignores CORS headers entirely.

Staging additionally sits behind a Cloudflare Access gate (`verify_cf_access`,
`dovecote/src/helpers/access.rs`) when `CF_ACCESS_AUD`/`CF_ACCESS_CERTS_URL` are configured —
requests without a valid `Cf-Access-Jwt-Assertion` header get `403 Forbidden` before the router
even runs. This is environment perimeter security, unrelated to the dashboard/device auth
models above; dev and production don't set these vars, so it's a no-op there.

## Error conventions

- Success responses are JSON (except `DELETE /pigeons/:pigeon_id`, which returns an empty body,
  and the device log-chunk POST, which returns an empty body).
- Error responses are **plain text**, not JSON — read `response.text()`, not
  `response.json()`, when handling a non-2xx status.
- Status codes used throughout: `400` (malformed JSON, missing/invalid path param, empty or
  over-cap telemetry report, empty log chunk), `401` (missing/invalid session cookie or device
  token),
  `403` (authenticated but not authorized — wrong ACL role, CF Access rejection on staging, or a
  per-tier limit reached: see [Per-tier limits](#per-tier-limits)),
  `404` (no matching route), `413` (log chunk or telemetry batch over the size cap), `500` (internal error — DB
  connection failure, Durable Object dispatch failure, etc).
- A deleted pigeon's Durable Object is never actually destroyed (Cloudflare DOs have no
  "delete yourself" API — see `objects/pigeons.rs`'s `delete` handler) — its tables are just
  emptied. A `GET` against a deleted pigeon therefore returns `403 Forbidden` (no ACL rows left
  to authorize against), not `404`.
- `GET /device/pigeons/:pigeon_id/ws` is the one exception to "error responses are plain text
  HTTP status codes": a rejected upgrade (bad `Upgrade` header, bad token) is still a normal HTTP
  error response (`400`/`401`/`429`), but a problem discovered *after* the socket is open (oversize
  frame, malformed frame, frame flood, spent message allowance, token rotated, pigeon deleted)
  has no HTTP status to report, so it's a WebSocket close code instead (`4001`-`4009` and `4029`;
  see that route's own section for the full list).

## Rate & size limits

There is no general-purpose rate limiting in `dovecote` today (beyond whatever Cloudflare
applies at the platform level). The routes that carry a real limiter carry it individually,
through Cloudflare's rate-limiter binding: the two public unauthenticated forms (`POST /errors`,
`POST /contact`, keyed per IP) and the device surfaces that are neither billed nor counted
(shadow polls and FOTA chunk downloads, keyed per pigeon, plus a per-IP budget on requests that
fail device authentication). Every one of them shares the same properties, and they are worth
stating once rather than in each row: the counters are approximate and roughly per-colo, so a
limit bounds runaway volume rather than gating precisely; over the limit answers `429` with
`Retry-After` and **never** `401`, which the dashboard treats as a sign-out signal; and each
check fails **open** on a binding fault, because a limiter outage taking a fleet offline would
be worse than a window of unthrottled traffic. The limits that do exist are:

| Limit | Value | Where |
|---|---|---|
| `POST /pigeons/batch` — pigeon IDs per request | 48 | `lib.rs` (Workers subrequest budget) |
| `POST /device/pigeons/:id/logs` — bytes per chunk | 16 KiB (`capsules::MAX_LOG_CHUNK_BYTES`) | `objects/pigeons.rs::report_logs_device`, `413` over the cap |
| Stored log chunks per pigeon | 200 (oldest silently pruned, not an error) | `objects/pigeons.rs::MAX_STORED_LOG_CHUNKS` |
| Stored latest-value telemetry keys per pigeon | 128 (`capsules::MAX_TELEMETRY_KEYS`) — least-recently-reported keys silently evicted, not an error | `helpers/telemetry_latest.rs`, see [Telemetry](#telemetry) |
| Telemetry keys per report | 128 (`capsules::MAX_TELEMETRY_KEYS`) | `helpers/telemetry_latest.rs`, `400` over the cap (nothing applied) |
| Telemetry key / value length | 128 / 1024 bytes (`capsules::MAX_TELEMETRY_KEY_BYTES`/`MAX_TELEMETRY_VALUE_BYTES`) | `helpers/telemetry_latest.rs`, `400` over the cap (nothing applied) |
| Readings per batched telemetry report | 64 (`capsules::MAX_TELEMETRY_BATCH_READINGS`) | `helpers/telemetry_batch.rs`, `400` over the cap (nothing applied) |
| Distinct keys across a whole batch | 128 (`capsules::MAX_TELEMETRY_KEYS`) — the union, not each reading's own count | `helpers/telemetry_batch.rs`, `400` over the cap (nothing applied) |
| Bytes per batched telemetry report | 16 KiB (`capsules::MAX_TELEMETRY_BATCH_BYTES`) — batch form only; a flat report is bounded by the key/value caps as before | `lib.rs`, `413` over the cap |
| How far back a batched reading may be timestamped | 24 h (`capsules::MAX_TELEMETRY_BACKDATE_SECS`) — clamped to the boundary, not an error; a future timestamp clamps to the receive time | `helpers/telemetry_batch.rs`, see [Telemetry](#telemetry) |
| `GET .../telemetry/history` points per query | bucketed by default (`capsules::TELEMETRY_HISTORY_BUCKET_TARGET` = 360 buckets, unlimited range, no cap); `raw=true` caps at 5000 (`capsules::TELEMETRY_HISTORY_MAX_POINTS`) — the range's **newest** 5000, flagged by `X-Telemetry-Truncated`, not an error | `helpers/telemetry.rs` |
| `PUT /pigeons/:id/log-dictionary` — bytes per upload | 4 MiB (`capsules::MAX_LOG_DICTIONARY_BYTES`) | `lib.rs`, `413` over the cap |
| `GET /device/pigeons/:id/ws` — max WebSocket frame size | 16 KiB | `objects/ws.rs::MAX_WS_FRAME_BYTES`, connection closed (`4002`) over the cap |
| `GET /device/pigeons/:id/ws` — frame rate | 50 frames / rolling 10s window, per socket | `objects/ws.rs`, connection closed (`4008`) over the cap |
| Pooled messages per billing period, for an account with no subscription to bill (free, or complimentary) | That account's served tier allowance (see [Billing](#billing)) | `helpers/usage.rs::check_ingest_fuse`; every device ingest surface `429`s past it (WebSocket: upgrade `429`, open socket closed `4029`) |
| Devices per account | Served tier's included count, for an account with no subscription to bill (see [Per-tier limits](#per-tier-limits)) | `helpers/usage.rs::check_device_cap`, `403` at `POST /flock/pigeons` |
| Seats per organization | Tier's seat count — members plus pending invites | `helpers/usage.rs::check_seat_cap`, `403` at `POST /orgs/:org_id/invites` |
| Alert definitions per account | Tier's alert count, across every flock and pigeon | `helpers/usage.rs::check_pigeon_alert_cap`/`check_flock_alert_cap`, `403` at both alert `POST`s |
| Organizations owned per user | Tier's organization count | `helpers/usage.rs::check_org_cap`, `403` at `POST /orgs` |
| `POST /pigeons/:id/shell` — device reply timeout | 10s default, 30s max (caller-configurable `timeout_ms`, clamped) | `objects/pigeons.rs::SHELL_TIMEOUT_DEFAULT_MS`/`SHELL_TIMEOUT_MAX_MS`, `504` over the wait |
| `POST /feedback` — bytes per raw body | 8 KiB (`capsules::MAX_FEEDBACK_BODY_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /feedback` — bytes in `message` | 4 KiB (`capsules::MAX_FEEDBACK_MESSAGE_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /feedback` — `contact_email` / `page_context` length | 254 / 512 bytes (`capsules::MAX_FEEDBACK_CONTACT_EMAIL_BYTES`/`MAX_FEEDBACK_PAGE_CONTEXT_BYTES`) | `lib.rs`, `400` over the cap |
| `POST /feedback` — `diagnostics` length | 2 KiB (`capsules::MAX_FEEDBACK_DIAGNOSTICS_BYTES`) | `lib.rs`, `400` over the cap |
| `POST /contact` — requests per IP | 5 / 60s (Cloudflare rate-limiter binding; counters are roughly per-colo) | `wrangler.toml` `[[ratelimits]]` + `lib.rs`, `429` over the limit — never `401` |
| `POST /contact` — bytes per raw body | 8 KiB (`capsules::MAX_CONTACT_BODY_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /contact` — bytes in `message` | 10 B to 4 KiB (`capsules::MIN_CONTACT_MESSAGE_BYTES`/`MAX_CONTACT_MESSAGE_BYTES`) | `capsules::contact::validate`, `400` under / `413` over |
| `POST /contact` — `name` / `email` / `company` / `about` length | 128 / 254 / 128 / 32 bytes (`capsules::MAX_CONTACT_*_BYTES`) | `capsules::contact::validate`, `400` over the cap |
| `POST /contact` — minimum time on the form | 2s (`capsules::MIN_CONTACT_FILL_MS`) | `capsules::contact::validate`, `400` under the floor |
| `POST /errors` — requests per IP | 20 / 60s (Cloudflare rate-limiter binding; counters are roughly per-colo) | `wrangler.toml` `[[ratelimits]]` + `lib.rs`, `429` over the limit — never `401` (the dashboard treats 401 as "session gone") |
| `GET /device/pigeons/:id/shadow` — requests per pigeon | 120 / 60s | `wrangler.toml` `DEVICE_SHADOW_LIMITER` + `helpers/device_limits.rs`, `429` over the limit. Twice the 60/min a 1s `telemetry_interval` produces, which is the fastest poll cadence the platform can express |
| `GET /device/pigeons/:id/firmware` — Range requests per pigeon | 400 / 10s (2400/min sustained) | `wrangler.toml` `DEVICE_FIRMWARE_LIMITER` + `helpers/device_limits.rs`, `429` over the limit. Set well above any real download rate on purpose: a device aborts the whole transfer on its first chunk error. The short window is load-bearing, not cosmetic — see the note below |
| `/device/pigeons/:id/*` — **failed** device authentications per IP | 10 / 60s | `wrangler.toml` `DEVICE_AUTH_FAIL_LIMITER` + `helpers/device_limits.rs::DeviceAuthGuard`, `429` over the limit. Only `401`s are counted, so a healthy device never touches it; the CoAP terminator's own address is exempt (see below) |
| `POST /errors` — bytes per raw body | 16 KiB (`capsules::MAX_ERROR_REPORT_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /errors` — bytes in `note` (manual JSON body) | 4 KiB (reuses `capsules::MAX_FEEDBACK_MESSAGE_BYTES`) | `lib.rs`, `413` over the cap |
| New-signature ops emails | 5 / hour, global; overflow folded into the next allowed email as a suppressed count | `helpers/errors.rs` |
| Stored error events per signature | newest 200 kept (each group's oldest 5 and all manual reports exempt) | `helpers/errors.rs` retention sweep on the 5-minute cron |
| Stored error events age | 90 days, keyed on server-side `received_at` | `helpers/errors.rs` retention sweep |

Three notes on the device limiters:

- **A high count on a 60s window does not enforce.** Cloudflare's binding is documented as
  permissive and eventually consistent, with counters cached per location and reconciled
  asynchronously, and staging measurement found the practical edge of that: `limit = 1200,
  period = 60` refused nothing across 3200 requests inside one window, and `1800` nothing across
  3000, while `limit = 120, period = 60` and `limit = 400, period = 10` both refused hard on the
  same route with the same code. That is why the firmware limiter uses the short window — it
  buys the same sustained ceiling at a counter magnitude that actually binds. Every one of these
  limits is also *permissive* at the margin: the failed-auth budget of 10/60s was measured
  letting 36 failures through before it cut over. **Retune any of these by measuring, never by
  reasoning about the configured number.**

- **Devices cannot see a `429` as a `429`.** The Zephyr client surfaces neither the status code
  nor response headers to its callers, so a limited request is indistinguishable there from any
  other transport failure and is simply retried on the next cycle. `Retry-After` is emitted for
  correctness and for anything else that speaks HTTP, not because a device reads it. No failure
  on these routes clears a device's configuration, credentials, or provisioning.
- **The CoAP terminator is exempt from the failed-auth IP budget, deliberately.** It proxies the
  whole CoAP fleet onto these same routes from one egress address, so a single device looping on
  a rotated token would spend the shared budget and lock every other CoAP device out behind it.
  The exemption is by source address, reusing the `COAP_SERVICE_ALLOWED_IPS` allowlist that
  already gates `GET /internal/coap-psk/:pigeon_id`. Those devices stay covered: the two
  per-pigeon limiters apply to everything the terminator forwards (the pigeon id is in the path),
  and the terminator applies its own per-source connection admission on the DTLS/TLS side.

Measured on staging against the deployed configuration: 150 shadow polls for one pigeon inside a
window produced 99 `200`s and 51 `429`s, with exactly 99 Durable Object invocations — a limited
request costs no object round trip. A burst of bad tokens from one address produced 36 `401`s
and then, on every subsequent request for the next 60s, a `429` with **zero** Durable Object
invocations. A healthy device's own cadence (30s telemetry plus a shadow poll and report-back
each cycle) saw no `429` at all, and a valid token works again as soon as the window passes.

---

## Dashboard API

All routes below require a valid Kratos session cookie (`credentials: include` from a browser
client whose origin matches `ROOT_URL`) unless noted otherwise.

### Flocks

#### `GET /flocks`

**Auth:** session

Lists every flock the caller can see — personal flocks they own, plus every org-owned flock
of an org they belong to (any role) — each with its member pigeon IDs. `org_id` is `null`
for personal flocks.

```sh
curl -s https://api.pidgeiot.com/flocks \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
[
  {
    "id": "c84932d0-160e-4007-bd72-0235d74a8033",
    "user_id": "8dc58300-70e6-4484-99f3-18ff7487b6fd",
    "name": "Backyard Coop",
    "service_plan": "free",
    "pigeon_ids": ["59d0c929f9124dbb..."],
    "updated_at": "2026-07-17T15:39:23Z",
    "created_at": "2026-07-17T15:39:23Z"
  }
]
```

#### `POST /flocks`

**Auth:** session

Creates a flock owned by the caller. Body: `capsules::FlockCreateRequest`.

```sh
curl -s -X POST https://api.pidgeiot.com/flocks \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"Backyard Coop"}'
```

Response is `capsules::Flock` JSON (empty `pigeon_ids`) with status `201` and a
`Location: /flocks/<flock_id>` header. `400` if `name` is empty. A freshly-created flock is
always **personal** (`org_id: null`) — see the transfer route below for moving it into an org.

There is no `PUT /flocks/:id` route today, even though `capsules::FlockUpdateRequest` exists as
a type — it isn't wired to anything yet.

#### `DELETE /flocks/:flock_id`

**Auth:** flock: manage

Deletes an **empty** flock. A flock that still holds pigeons is refused with `409` and a
message naming how many are in the way — `pigeons.flock_id` cascades, so an unguarded delete
would drop every device's mirror row, history, firmware catalog and alerts while the Durable
Objects lived on. Delete the pigeons first, one at a time, through
[`DELETE /pigeons/:pigeon_id`](#delete-pigeonspigeon_id).

`403` when the caller neither owns the flock nor is an owner/admin of the org that does, and
for an unknown flock id (missing and forbidden are deliberately indistinguishable). Returns
`200` with an empty body on success, and on a flock that was already gone. Firmware images the
flock's catalog referenced stay in R2: object keys are the image's own sha256, shared across
flocks.

The emptiness guard and the delete are one statement, but a pigeon created against this flock
by a concurrent request can still commit just after the guard ran. That pigeon keeps its own
Durable Object — its device credentials and shadow are untouched — so what a lost race costs
is its Postgres mirror row.

```sh
curl -s -X DELETE https://api.pidgeiot.com/flocks/<flock_id> \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `POST /flocks/:flock_id/transfer`

**Auth:** flock owner + target org owner/admin

Moves a **personal** flock into an organization (task #12). Body:
`capsules::FlockTransferRequest` (`{ org_id }`). Requirements, all enforced server-side:

- the caller is the flock's owner (`flocks.user_id`);
- the flock is not already org-owned (`409` otherwise — no org→org re-transfer today);
- the caller is an **owner or admin** of the target org.

On success, every pigeon currently in the flock gets an ACL row `{ entity_id: <org_id>, role:
"owner" }` written into its own Durable Object **first** — this is the authoritative
authorization write and is *not* best-effort: any per-pigeon failure aborts the transfer with
`500` before the flock is marked org-owned. The DO grant is an idempotent upsert, so retrying
a failed transfer is safe. Only after every DO write lands does `flocks.org_id` flip; the
Postgres `pigeon_acl` mirror is then synced best-effort per the usual convention. Returns the
updated `capsules::Flock` (now carrying `org_id`).

Note the flock's pre-existing per-user ACL rows are untouched — the transferring owner keeps
any direct pigeon access they already had. Org-granted access, by contrast, lives and dies
with the membership row: remove a member and their org-derived access goes with it, within the
[propagation window](#organizations) that applies to every membership change.

```sh
curl -s -X POST https://api.pidgeiot.com/flocks/<flock_id>/transfer \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"org_id":"<org_id>"}'
```

### Organizations

Shared-org management (task #12): individual Kratos accounts, one `organizations` row per
team, membership rows carrying per-user roles, app-level invites (self-hosted Kratos has no
B2B invite flow), and org-owned flocks. **No literal shared accounts; no Ory Keto** — the
model is rolled onto the existing per-pigeon ACL + flock tables.

**Roles and the permission matrix.** `capsules::OrgRole` is `owner | admin | member`:

| Capability | owner | admin | member |
|---|---|---|---|
| List/read org, members (`GET /orgs`, `GET /orgs/:id`) | yes | yes | yes |
| Rename org, set its timezone (`PUT /orgs/:id`) | yes | yes | no |
| Delete org (`DELETE /orgs/:id`, only when it owns no flocks) | yes | no | no |
| Invite members (`POST /orgs/:id/invites`), view/revoke invites | yes | yes (but cannot invite an `owner`) | no |
| Change member roles (`PUT /orgs/:id/members/:user_id`) | yes | no | no |
| Remove members (`DELETE /orgs/:id/members/:user_id`) | yes | yes (but never an `owner`) | self-removal (leave) only |
| Org-owned flocks: view (list, pigeons list, telemetry history, firmware list, alerts list/state) | yes | yes | yes |
| Org-owned flocks: manage (create pigeons, upload firmware, create alerts, be a transfer target) | yes | yes | no |
| Org-shared pigeons: member-level routes (read, shadow get/put, telemetry, logs) | yes | yes | yes |
| Org-shared pigeons: owner-level routes (delete, token refresh, ACL changes, shell) | yes | yes | no |
| View billing overview (`GET /orgs/:id/billing`) | yes | yes | yes |
| Manage billing (`POST /orgs/:id/billing/checkout`, `POST /orgs/:id/billing/portal`, `PUT /orgs/:id/billing/plan`) | yes | yes | no |
| View business details (`GET /orgs/:id/business-details`) | yes | yes | yes |
| Change business details (`PUT /orgs/:id/business-details`) | yes | yes | no |

Last-owner protection: an org must always retain at least one `owner` — demoting or removing
the only owner is refused with `409`, regardless of who asks.

The pigeon-side mapping, precisely: an org-shared pigeon carries a `pigeon_acl` row
`{ entity_id: <org id>, role: "owner" }`. A caller's effective rights through that row are
capped by their role in the org — `owner`/`admin` may exercise the row's full (owner-level)
rights; `member` is capped at member-level. Per-user ACL rows are unaffected.

**A membership change takes about 75 seconds to reach authorization, not one request.** Both
role reads on the gateway are plain `SELECT`s issued through Hyperdrive: the per-request
principal set (`load_org_roles`) and the single-org check every `/orgs/*` route makes
(`org_role_of`). Hyperdrive answers an identical query from its own cache for roughly a
minute, so adding a member, removing one, and changing a role all reach the authorization
path within about 75 seconds of the row being written, and until then the caller keeps the
access they had. The entitlement counts behind [Per-tier limits](#per-tier-limits) read
through the same cache and can admit whatever fits in one window. Anchoring these reads on
`now()` would keep them out of that cache at the price of a database round trip on every
authorized organization call, which is why they are not.

**The lists a human reads are exempt, deliberately.** `GET /orgs`, and the member and invite
lists inside `GET /orgs/:org_id`, each anchor their statement on `now()` and are therefore
never served from that cache: they answer within one round trip of the row changing. These
are the reads somebody stares at during onboarding — an invitee's own org list, and the
inviter's view of who has accepted — and the browser watching them is never the browser that
performed the write, so response-driven client state cannot cover the gap. Authorization
stays cached; what the page shows does not.

#### `POST /orgs`

**Auth:** session

Creates an organization; the caller becomes its founding `owner` (an org can never exist
without one). Body: `capsules::OrganizationCreateRequest`
(`{ name, business_name?, tax_id?, tax_id_type? }` — the last three optional and defaulting
to absent/`none`, so a client that predates them is unaffected). Returns
`capsules::Organization` with `201` and a `Location: /orgs/<org_id>` header.

**Business details are settled before the org is inserted.** A `tax_id` that fails the format
check, or that VIES definitively rejects, refuses the whole creation with `400` and leaves no
organization behind. Everything else about the details write is best-effort *after* the
insert: the caller has been granted the org, so a details write that fails leaves the fields
blank and editable rather than failing a creation that already succeeded. See
[Business details](#business-details) for the full semantics — the create path applies exactly
the same rules as `PUT /orgs/:org_id/business-details`.

**Organization-count entitlement.** `403` past the caller's tier's allowance (see
[Per-tier limits](#per-tier-limits)). Counts the organizations this person already *owns*, so
belonging to somebody else's spends none of their own allowance, and the tier is the best one
any of those organizations is entitled to. Deleting an organization frees its slot.

#### `GET /orgs`

**Auth:** session

Lists every org the caller belongs to, with the caller's own role —
`Vec<capsules::OrganizationMembership>` (`{ organization, role }`). Not cached (see
[Organizations](#organizations)): an invite accepted a second ago is in this list.

#### `GET /orgs/:org_id`

**Auth:** any member

Returns `capsules::OrganizationDetail`: the org, the caller's role, the full member list
(each `capsules::OrganizationMember` carries `email` — denormalized at join time — and
`invited_by`, the per-person audit trail), and pending invites (`invites` is only populated
for owner/admin callers; plain members get an empty list). Neither list is cached (see
[Organizations](#organizations)), so an invite disappears from `invites` and its acceptor
appears among the members on the first load after they accept.

#### `PUT /orgs/:org_id`

**Auth:** owner/admin

Renames the org, sets its timezone, or both. Body:
`capsules::OrganizationUpdateRequest` (`{ name?, timezone? }`). An absent field is left
unchanged, so the two controls that write here save independently; `400` when neither is
present, and when `name` is present but blank.

`timezone` is an IANA zone name (`America/New_York`), validated against the timezone
database dovecote carries: a name it knows is stored as written (aliases such as
`US/Eastern` included), and one that differs only in case is repaired
(`america/new_york` is stored as `America/New_York`). Anything the database does not know
is refused with `400`. The zone defaults to `UTC` and is **what the emails about this
organization are stamped in** (see [Email timestamps](#email-timestamps)); the dashboard
itself keeps rendering times in the reader's own browser zone.

Returns the updated `capsules::Organization`.

#### Email timestamps

Every time the invitation and alert-notification emails print is stamped in the
organization's own `timezone`, with UTC beside it:
`26 Aug 2026, 15:10:09 EDT (19:10:09 UTC)`. The parentheses carry the date as well whenever
the two zones disagree about which day it is. That covers the alert's observed-at, the "last
report" a silence observation names, and the invitation's expiry (stated twice, in the fact
row and in the note about the link).

Which zone applies:

- **Invitation**: the organization being invited into. The invitee has no account yet, so
  there is no other clock to use.
- **Alert notification**: the organization owning the flock the pigeon belongs to. A
  **personal** flock belongs to no organization, so its notifications stay in UTC.

An organization on `UTC` (the default) gets exactly what these emails said before zones
existed: one UTC time, not the same time twice. A stored zone the timezone database cannot
resolve (realistically only a zone the database dropped after the write) logs and falls back
to UTC rather than failing the send.

The dashboard is unaffected: it renders times in the reader's own browser zone, as it always
has.

#### `DELETE /orgs/:org_id`

**Auth:** owner

Deletes the org **only when it owns no flocks** (`409` otherwise — transfer or delete them
first). Membership and invite rows cascade. Returns `200` with an empty body.

#### `PUT /orgs/:org_id/members/:user_id`

**Auth:** owner

Changes a member's role. Body: `capsules::OrganizationMemberRoleUpdateRequest`
(`{ role }`). `409` if it would leave the org ownerless. Returns the updated
`capsules::OrganizationMember`.

#### `DELETE /orgs/:org_id/members/:user_id`

**Auth:** owner/admin, or self

Removes a membership row, which is **the revocation mechanism**: the removed user loses every
org-granted flock/pigeon right, with no ACL rows to rewrite, since the principal set is loaded
per request. That load is cached, so revocation lands within about 75 seconds rather than on
the removed user's next request; see [Organizations](#organizations) above for why. Admins can
never remove owners; anyone may remove themselves (leave); `409` if it would leave the org
ownerless.

#### `POST /orgs/:org_id/invites`

**Auth:** owner/admin

Invites an email address at a given role. Body: `capsules::OrganizationInviteCreateRequest`
(`{ email, role }`); inviting at role `owner` is itself owner-only. Mints a random 128-bit+
token, stores **only its sha256 hash** (`organization_invites.token_hash`), and emails the
invite link (`<ROOT_URL>/invite?token=<token>`) through the platform's existing Resend
transport. The message (`capsules::format_invite_email`, HTML plus a plain-text part that says
the same thing; subject `[PidgeIoT] Invitation to join <org>`) names the inviter by the name and
email address on their session (`Ana Ruiz (ana@example.com)`, or whichever the identity
carries), the organization, the role and what it allows, the expiry, and what
to do if the invitation was unexpected. In an environment with no `RESEND_API_KEY` configured (dev), the link is logged to
the Worker console instead — grab it from `wrangler dev` output. Returns `201` with
`capsules::OrganizationInviteCreated` (`{ invite, token, invite_url }`) — **the only place
the cleartext token ever appears** (write-once, same convention as device connector tokens);
`GET` reads return only hash-backed metadata.

Invites expire after **7 days** and are **single-use** (consumed atomically on accept).

**Seat entitlement.** `403` past the org's tier's seat count (see
[Per-tier limits](#per-tier-limits)), checked after the role checks so a non-manager learns
they may not invite rather than how full the org is. **A pending invite counts as a spent
seat** — the count is members plus unaccepted, unexpired invites, the same set `GET
/orgs/:org_id/invites` returns. Counting only filled seats would let an org at its limit invite
its way past it and discover the problem when a colleague accepts, which is the worst place to
find out. `POST /invites/accept` is therefore not gated: the seat was already spent when the
invite was sent.

#### `GET /orgs/:org_id/invites`

**Auth:** owner/admin

Pending (unconsumed, unexpired) invites — `Vec<capsules::OrganizationInvite>`.

#### `DELETE /orgs/:org_id/invites/:invite_id`

**Auth:** owner/admin

Revokes a pending invite. Idempotent (`200` even if already gone).

#### `POST /invites/accept`

**Auth:** session

Consumes an invite token for the **calling session** (requires an authenticated Kratos
session; the frontend's `/invite?token=` page routes unauthenticated visitors through
login/registration first). Body: `capsules::OrganizationInviteAcceptRequest` (`{ token }`).
Returns `201` with the caller's new `capsules::OrganizationMembership` (`{ organization, role }`,
the same item `GET /orgs` lists, so a client can add it to its own list without re-reading one
Hyperdrive may still be serving from cache); `404` for an invalid/expired/used token; `409` if
the caller is already a member (the invite is left unconsumed in that case).

**Token-alone acceptance — a documented tradeoff.** The token is a bearer credential:
whichever authenticated account presents it first joins, *regardless of which email that
account registered under*. This is deliberate — invitees routinely register under a different
address than the one the invite was sent to, and an email-match requirement would strand them
— and is compensated by the short (7-day) expiry, single-use consumption, hash-only storage,
and the inviter's ability to revoke pending invites and remove members. The alternative
(require `session email == invited email`) is stricter against forwarded/leaked invite
emails; if that ever matters more than invitee flexibility, the accept handler is the single
place to add the check.

### Business details

Who an organization's invoices are made out to, and under which tax registration. These live
on the **organization**, not on the Kratos identity: the org is the billing entity (it is what
carries `stripe_customer_id`), it survives a change of individual owner, and one person can
belong to two orgs with two different registrations — an identity trait could not express
that, and it would put a VAT field in front of every hobbyist at signup.

A stored `tax_id` is **not** stripped on read, unlike connector tokens and invite tokens. A VAT
number is printed on every invoice its owner issues and is publicly checkable by anyone; hiding
it from the org's own members would protect nothing and would stop them noticing a typo. Logs
never carry one in full — only its kind, country prefix and length (`capsules::tax_id_log_label`).

**Types** (`capsules::TaxIdType`, serialized snake_case):

| `tax_id_type` | Meaning | Checked remotely? |
|---|---|---|
| `none` | Nothing on file. Sending it with a non-empty `tax_id` is a `400`; it is how the field is cleared. | — |
| `eu_vat` | An EU (or Northern Ireland, `XI`) VAT number. | Yes, against VIES |
| `gb_vat`, `au_abn`, `ca_gst_hst`, `ca_bn`, `in_gst`, `us_ein`, `nz_gst`, `sg_gst`, `jp_trn`, `no_vat`, `za_vat` | A registration Stripe can place by jurisdiction; the values are Stripe's own tax-ID types, so the mapping is the name. Format sanity only here (Stripe validates ABN and UK VAT itself once forwarded). | No |
| `other` | Any other jurisdiction. Format sanity only. Held for display; never forwarded, because Stripe's enum cannot name where it was issued. | No |

**Statuses** (`capsules::TaxIdStatus`):

| `tax_id_status` | Meaning | Forwarded to Stripe at checkout? |
|---|---|---|
| `none` | Nothing on file. | — |
| `pending` | An EU VAT number we hold but could not get a verdict for. Retried by the scheduled sweep. | No |
| `validated` | VIES confirmed a live registration. `tax_id_validated_at` is when. | Yes |
| `invalid` | VIES said it is not a registration. **Only reachable via a re-check** — see below. | No |
| `unverified` | Held but not checked, because nobody validates that kind (`other` and every jurisdiction type). | Yes, for a jurisdiction type; never for `other` |

#### VIES semantics — the rule that matters

EU VAT numbers are validated against the European Commission's VIES REST endpoint,
`POST https://ec.europa.eu/taxation_customs/vies/rest-api/check-vat-number` with
`{"countryCode","vatNumber"}`. No key, no registration, no published quota. Observed latency
is ~0.3–0.5 s.

**A VIES outage never blocks a save.** A lookup has exactly three outcomes, and only one of
them refuses anything:

- **`valid: true`** → stored `validated`, `tax_id_validated_at` stamped.
- **`valid: false`** → the save is **refused** with `400`. This is the only refusal.
- **anything else** → stored `pending`, and the scheduled sweep asks again. "Anything else"
  means: a transport failure, a non-2xx, a body we cannot parse, or an `errorWrappers`
  envelope — `MS_UNAVAILABLE`, `TIMEOUT`, `SERVICE_UNAVAILABLE`, the concurrency limits. None
  of these is evidence about the number.

The distinction is in the wire protocol, not inferred: **`MS_UNAVAILABLE` arrives with HTTP
200**, so a client that checks only the status code and then reads `valid` as false-by-absence
would declare a genuine registration invalid every time its own tax authority went down. VIES
routinely has one or two of its twenty-eight member states listed `Unavailable`
(`GET /rest-api/check-status` reports them live).

**Why `invalid` is unreachable from a save.** At save time a definitive `invalid` refuses the
write, so nothing lands in that state by being entered. It is reachable only through the sweep
resolving a row that is already `pending`. That asymmetry is deliberate: a save can refuse
because there is nothing to leave behind, and a re-check cannot, because there already is.

**An inconclusive re-save never downgrades a confirmed registration.** Saving the form
re-runs the lookup, so somebody editing only their business name during a VIES outage would
otherwise be flipped from `validated` to `pending` for something they did not do. When the
`tax_id` is unchanged and already `validated`, an inconclusive outcome leaves it `validated`.
A *different* number carries none of the old one's history and starts clean.

**There is no revalidation cadence.** A `validated` row is never re-checked by the sweep, so a
registration deregistered after we confirmed it keeps reading as `validated` until its owner
next saves the form. `tax_id_validated_at` is therefore the honest thing to read: it is when
we last confirmed, not a claim about now.

**Format checks run first, locally**, so a typo is refused without spending a VIES call and
with a specific reason (`"that is not the shape of a DE VAT number"`). They are per-country
shape rules only. They stop short of national check digits on purpose — VIES already rejects a
checksum-failing number itself, without contacting the member state, so a second implementation
would only be a second thing to get wrong. `GR` is accepted as an alias for Greece's VAT prefix
`EL` and stored as `EL`.

**The retry sweep** rides the existing 5-minute cron (`dovecote/src/scheduled.rs`, same
5-cron-trigger account limit as the other passengers). It takes `pending` + `eu_vat` rows whose
last attempt is over an hour old, oldest first, at most 20 per sweep. `tax_id_checked_at`
records every attempt and is what paces this; `tax_id_validated_at` records only confirmations
and is deliberately *not* disturbed by an inconclusive re-check, so "confirmed on the 3rd,
unreachable since" stays readable. The update is guarded on the row still holding the same
number and still being `pending`, so an edit made mid-sweep is never overwritten by an answer
about the previous number.

#### What reaches Stripe, and when

The org's row is the source of truth for the Stripe Customer's tax identity, and it is
applied at the one moment it matters: `POST /orgs/:org_id/billing/checkout` brings the Customer
into line **before** the Checkout session is minted
(`dovecote/src/helpers/stripe_tax_identity.rs`).

- **Name.** A non-empty `business_name` becomes the Customer's `name` (and names a brand-new
  Customer outright; the org's display name is only the fallback for an org that never filled
  the form in). That is the legal entity printed on every invoice.
- **Tax ID.** The registration is posted as a Stripe tax ID (`POST /v1/tax_ids`, `owner`
  = the Customer) when it is one Stripe can place **and** there is no reason to doubt it: a
  `validated` `eu_vat`, or an `unverified` jurisdiction type (declared by the customer, checked
  by nobody). It is deliberately **not** forwarded while `pending` — Stripe would re-validate
  it, but an unanswered lookup must not zero-rate an invoice on our say-so — nor when
  `invalid`, nor for `other`. Checkout collects a tax ID itself in those cases (see
  [Billing](#billing)).
- **Idempotent, and a replacement.** The Customer's existing tax IDs are listed first; a
  Customer already holding the same number (compared normalized, since Stripe keeps the
  separators its own form was given) gets nothing created, and a Customer holding a different
  number, or a second copy, has the strays deleted before the org's is created. A Customer
  therefore carries exactly the org's registration, and a repeat checkout adds nothing. When
  the org holds nothing forwardable, whatever Checkout collected is left where it is: that is
  the only place the number was ever entered.
- **Best-effort.** A Stripe refusal (a number Stripe's own format check rejects) or an outage
  is logged with the status, kind and code only — never the number — and the session still
  opens; Checkout then collects.
- **Reverse charge is Stripe Tax's decision**, from the billing address and the tax ID on the
  Customer. Nothing here sets `tax_exempt` by hand; a US customer with an exemption certificate
  is a Dashboard action on the Customer, not a form field.

Stripe validates EU VAT (VIES) and UK VAT (HMRC) again on its side, asynchronously. That
result is not read back; the org row's status remains what **we** established, and the two
checks are not redundant: ours refuses a definitively invalid number before a customer reaches
Checkout, Stripe's decides the invoice.

#### `GET /orgs/:org_id/business-details`

**Auth:** any member

Returns `capsules::OrganizationBusinessDetails`:
`{ org_id, business_name, tax_id, tax_id_type, tax_id_status, tax_id_validated_at, tax_id_checked_at }`.
`404` if no such org; `403` if the caller is not a member.

#### `PUT /orgs/:org_id/business-details`

**Auth:** owner/admin

Body: `capsules::OrganizationBusinessDetailsRequest`
(`{ business_name?, tax_id?, tax_id_type }`). **Replaces every field wholesale** — this is a
small form the customer sees in full, so a partial update would mean guessing which blank meant
"unchanged" and which meant "delete this". Returns the updated
`capsules::OrganizationBusinessDetails`.

Authorization is checked **before** the VIES call, so a non-member cannot use this route as a
free VAT-lookup oracle.

`400` for: a business name over 200 characters; a tax ID over 32 characters after
normalization; a `tax_id` supplied alongside `tax_id_type: none`, or a type without an ID
(both ambiguous, so both are refused rather than half-honoured); a country code VIES does not
serve (`GB` no longer qualifies — only `XI`); a shape that is wrong for its country; and a VAT
ID VIES definitively rejects. `403` for a non-manager; `404` if no such org.

Normalization before storage: uppercased, with whitespace, `.`, `-` and `/` stripped. So
`" ie 6388047v "`, `IE-6388047-V` and `ie/6388047/v` all store as `IE6388047V`.

### Billing

Billing attaches to **organizations** (a personal, org-less account is always the free tier).
Stripe hosts every payment surface — these routes mint redirect URLs and read state; card data
never touches this API. The read side is member-visible; the session mints are manager-only
(owner/admin), matching the rest of the org permission matrix.

**What a dashboard user sees.** Upgrading opens Stripe's hosted Checkout, which asks for a
card, a billing address (always) and, for a business outside the US, a business tax ID
(required wherever Checkout supports one) unless the org's [business details](#business-details)
already carry a registration we forwarded — then that field is simply absent and the invoice
is made out to the registered business name. Tax, if any, shows as its own line computed from
the address; a business with a forwarded or entered tax ID sees the reverse charge or zero
rate its jurisdiction applies instead of tax added. After paying they land back on the org
page, and the plan updates within moments via the webhook.

**Delayed payment methods (ACH Direct Debit).** ACH is a delayed-notification method: the
Checkout session completes with `payment_status=unpaid`, and the debit clears or bounces days
later. Entitlement is decided by the **subscription's** status (`trialing`/`active`/`past_due`),
never by the session's `payment_status`, so the exposure follows two windows, both verified in
Stripe test mode:

- **Before the bank account is verified**, the subscription is `incomplete` and the account is
  **not** entitled — the free tier still applies. Instant (Financial Connections) verification
  clears this in seconds; manual microdeposit verification takes 1–2 business days.
- **Once the account is verified**, the subscription becomes `active` while the first debit is
  still processing (up to ~4 business days to settle), and the account **is** entitled before
  the money arrives. If that first debit is then returned, Stripe emits
  `checkout.session.async_payment_failed` and moves the subscription to `past_due` (still
  entitled) and, through dunning, eventually `unpaid` (not entitled, tier remembered). A
  cleared debit emits `checkout.session.async_payment_succeeded` and the subscription stays
  `active`.

The deliberate exposure is that second window: a customer whose first debit ultimately bounces
holds entitlement from verification until dunning gives up. `async_payment_failed` is logged
and mailed the moment it arrives so the bounce is visible well before the status change. This
matches the policy for cards (entitle on `active`, keep entitlement through `past_due`); ACH
only widens the gap between "entitled" and "paid". Recommended dunning: Smart Retries on, final
action **mark unpaid** (not cancel), which is what the code models.

#### Per-tier limits

Every published per-tier quantity is enforced at the route that would create one more of the
thing. The tier is the **effective** one, resolved in the order subscription, then
complimentary grant, then free: subscription status is checked before the stored plan (so a
cancelled org is gated at the free tier, not at the tier it used to hold), and an org carrying
a complimentary grant is gated at the granted tier. See
[Complimentary tiers](#complimentary-tiers).

| | Perch (free) | Builder | Growth | Scale | Fleet |
|---|---:|---:|---:|---:|---:|
| Devices | 10 (hard cap) | 50 | 250 | 1,500 | 10,000 |
| Pooled messages/period | 300 K | 1.5 M | 7.5 M | 45 M | 300 M |
| Seats per org | 1 | 3 | unlimited | unlimited | unlimited |
| Alerts per account | 1 | 10 | unlimited | unlimited | unlimited |
| Organizations owned | 1 | 1 | unlimited | unlimited | unlimited |

Devices are the one row that is a *price* rather than a ceiling: past the included count an
account **with a live subscription** bills per-device overage instead of being refused. An
account with no subscription to bill — free, or complimentary — hard-caps at its own served
tier's device count. Seats, alerts and organizations refuse on every tier that publishes a
number.

A refusal is always `403` with a plain-text body in one shape:

```
Forbidden: the free tier includes 1 seat and this account already has 1 (members plus pending invites) -- upgrade to add more
```

Four rules hold for all of them:

- **A refusal blocks growth only.** Nothing that already exists is disturbed, so an account
  that lands above a limit by downgrading keeps every device, seat, alert and organization it
  has and is simply refused the next one.
- **They fail open.** A lookup failure allows the request and logs; an infrastructure blip must
  not block a customer from using what they pay for.
- **Counts are per account, not per container.** The alert limit spans every flock and pigeon
  the account owns (a per-pigeon limit would be no limit, since flocks are free); the device
  count spans every flock; the seat count is per organization, which is what a seat belongs to.
- **The free tier keeps one organization on purpose.** Billing attaches to an organization and
  checkout runs against one that already exists, so a free account refused its first could
  never reach a paid tier. What the free tier does not get is a *team* — that is the one seat.

#### Complimentary tiers

An organization can be granted a paid tier's entitlements without a subscription — a partner
fleet, a design-partner pilot. The grant lives in three nullable `organizations` columns
(`comp_plan`, `comp_note`, `comp_granted_at`) and **no route reads or writes them**: granting
and revoking are hand-run SQL, documented in `docs/infra/org-comps.md`. There is deliberately
no self-service or admin surface, so nothing reachable from the internet can grant one.

How it resolves:

- **A live subscription outranks a grant**, even a richer one. A comp is a floor for an account
  that is not paying, not a discount on one that is; a grant left on an org that later
  subscribes is inert, and revoking it changes nobody's invoice. It starts serving again only
  if that subscription lapses.
- **A comped account is never billed.** It has no subscription to put an overage line on, so
  the usage meters skip it entirely and nothing about it reaches Stripe.
- **It is still bounded.** Because it cannot bill overage, it behaves like the free tier at the
  edges of its granted tier: the ingest fuse pauses it at the granted tier's message allowance,
  and the device cap is a hard cap at the granted tier's device count rather than the start of
  per-device billing. A grant with no meter behind it would otherwise be an unbounded one.
- **An unreadable or `perch` grant value is no grant**, and resolves to the free tier — a typo
  under-serves rather than over-serving.

`GET /orgs/:org_id/billing` reports it as `comp_plan`, set only when the grant is what actually
decided `effective_plan`. The grant's note is not exposed over the API.

#### `GET /orgs/:org_id/billing`

**Auth:** org: member

Returns `capsules::OrganizationBillingOverview`: the stored `plan`, `subscription_status`,
whether that status is currently `entitled`, the **`effective_plan`** actually being served
(entitlement-gated — a cancelled org shows its old `plan` but an `effective_plan` of the free
tier), `cancel_at_period_end`, `has_billing_account` (whether a Stripe customer exists — the
precondition for the portal), the usage-period bounds, and usage against allowance:
`billable_messages` / `included_messages`, `device_count` / `included_devices`, and
`comp_plan` (see [Complimentary tiers](#complimentary-tiers) — `null` for almost every org).
Usage-period
bounds are the org's Stripe billing period while a live subscription covers now, the calendar
month otherwise — the same anchoring the usage tally itself uses. `403` for non-members,
`404` for an unknown org.

`included_messages` is the allowance the meter actually charges against, which is why it can
exceed the tier's own pooled figure: **each billed extra device adds 30 K messages to the
pool** (`connected_device_count` beyond `included_devices`, times 30 K — the same per-device
budget every tier's own allowance works out to), and a mid-period downgrade's recorded floor
keeps the outgoing tier's allowance from shrinking retroactively. The extra pool is why a
fleet a little past its device count no longer bills device overage and message overage for a
single act of growth. The free tier has no billed extras at any device count, so its allowance
— and the ingest fuse trained on it — is exactly the tier's own 300 K.

#### `POST /orgs/:org_id/billing/checkout`

**Auth:** org: manage

Mints a Stripe Checkout session for a paid tier and returns
`capsules::BillingSessionUrl` (`{ url }`) for the dashboard to redirect to. Body:
`capsules::BillingCheckoutRequest` (`{ plan: builder|growth|scale|fleet }`) — `perch` is a
`400` (the free tier is not purchasable). The session carries three prices, resolved at
request time by `lookup_key` (never pinned ids): the licensed tier, the pooled
`message-overage` meter price, and that tier's own `device-overage-<tier>` meter price.
Creates (and remembers) the org's Stripe customer on first use — a returning org always
checks out against the same Customer. `502` when Stripe itself is unreachable or the catalog
is missing a price.

**Tax is computed by Stripe Tax, and the session is built to let it.** Every session carries
`automatic_tax[enabled]=true`, `billing_address_collection=required`,
`customer_update[address]=auto` and `customer_update[name]=auto` (the session is always
handed an existing Customer, and Checkout will not write the collected address or business
name back onto one unless told it may — the address is what every later subscription invoice
computes tax from), plus `tax_id_collection[enabled]=true` with `required=if_supported`. The
exact parameter set is pinned by a test in `dovecote/src/helpers/stripe_api.rs`. Consequences:

- **A billing address is always collected** at Checkout, and saved onto the Customer.
- **Tax appears as its own line** when the address falls in a jurisdiction the account is
  registered in (Stripe Dashboard: Tax > Registrations, per mode); elsewhere the calculation
  returns zero with `not_collecting`. What is registered is an owner-side setting, not code.
- **A business buyer abroad must give a tax ID.** In every country Checkout can collect one
  for, `if_supported` makes the field mandatory, which is what keeps sales outside the US B2B.
  Checkout shows that form only to a Customer with no tax ID yet; an org whose registration
  was forwarded from its [business details](#business-details) never sees it. A collected ID
  lands on the Customer and prints on the invoice like a forwarded one.
- **A business tax ID earns the reverse charge or zero rate** Stripe Tax applies for that
  jurisdiction (EU, UK, Australia, Canada, India and the rest of Stripe's list): the invoice
  carries the ID in its header and a zero-rated tax line with the reason, and nothing here
  sets `tax_exempt` by hand.
- `customer_update` is a create-only parameter — the Checkout Session object has no such
  attribute to read back; its effect is the Customer's `address` and `name` after completion.

The Checkout parameters alone are not enough: tax computes correctly only when every price
carries `tax_behavior=exclusive` and every product a tax code (`txcd_10103001`, SaaS for
business use), and a price's `tax_behavior` can be set only while it is still `unspecified`.
`scripts/stripe-catalog.py` builds a fresh environment's catalog with both in place and, as a
dry run, checks an existing one; it never modifies an existing Stripe object.

#### `POST /orgs/:org_id/billing/portal`

**Auth:** org: manage

Mints a Stripe Billing Portal session for the org's existing customer and returns
`capsules::BillingSessionUrl` (`{ url }`) — card updates, invoice history and cancellation
happen on Stripe's hosted page. Plan changes do **not**: Stripe's portal cannot switch a
multi-product subscription (and every checkout-minted subscription here is one), so tier
changes go through `PUT /orgs/:org_id/billing/plan` below. `409` if the org has no billing
account yet (checkout is the flow that creates one); `502` when Stripe is unreachable.

#### `PUT /orgs/:org_id/billing/plan`

**Auth:** org: manage

Moves an org with a live subscription to a different paid tier, in place. Body:
`capsules::BillingPlanChangeRequest` (`{ plan: builder|growth|scale|fleet }`). Returns the
post-change `capsules::OrganizationBilling` (Stripe's own updated subscription state); the org
row itself is written moments later by the `customer.subscription.updated` webhook, same as
every other subscription change.

One Stripe Subscriptions Update call re-prices two items together, resolved by `lookup_key` at
request time: the licensed tier item to the new tier's flat price, and the per-device overage
item to `device-overage-<newtier>` (per-tier rates differ). The pooled `message-overage` item
shares one rate across tiers and is untouched. A subscription that predates the metered
composition gets the device-overage item added by the same call.

**Proration** is immediate in both directions (`proration_behavior=create_prorations`): an
upgrade charges the price difference for the rest of the period onto the next invoice; a
downgrade credits it. Scheduling a downgrade for period end instead is future polish — today a
downgrade applies (and credits) immediately.

The same update passes `automatic_tax[enabled]=true`: enabling Stripe Tax at the account does
not reach into subscriptions that already exist, and this is the one write made to a live
one, so a subscription created before tax was on converges the next time its plan changes.

**Metered usage across a mid-period change**: at the moment of the swap Stripe closes off any
meter usage already reported and bills it at the *old* item's rate; usage reported after the
swap bills at the new rate. Since our reporter posts the extra-devices figure near period
*end*, in practice the whole period's devices bill at the tier held then — the new tier's
per-device rate and included-device count — unless the change lands after the final-day report,
in which case that period's figure keeps the pre-change rate. The message allowance is
customer-favorable the other way: the in-flight period is charged against the **higher** of the
old and new tiers' allowances, so a downgrade never converts already-included messages into
overage retroactively.

Errors: `400` for `perch` (that's a cancellation — use the billing portal) or a bad body;
`403` for non-managers; `404` for an unknown org; `409` when the org is already on the
requested tier, or has no live (`trialing`/`active`/`past_due`) subscription to change;
`502` when Stripe is unreachable or the catalog is missing a price.

#### `POST /billing/webhook`

**Auth:** Stripe signature required

The Stripe event sink (not a dashboard route; authenticated by `Stripe-Signature`
HMAC verification against the endpoint signing secret, 5-minute replay window, `v1` scheme
only). Dispatch is `dovecote/src/helpers/stripe_webhook.rs::webhook_action`, pinned by a test;
the endpoint in each Stripe environment (Dashboard: Developers > Webhooks, per mode) must
subscribe to exactly these eleven events:

| Event | Action |
|---|---|
| `customer.subscription.created`, `.updated`, `.deleted`, `.paused`, `.resumed`, `.pending_update_applied`, `.pending_update_expired` | Writes plan/status/period onto the owning org, idempotently, with out-of-order-event protection. |
| `checkout.session.completed` | Binds the Stripe customer to the originating org and applies the purchased subscription's state. |
| `invoice.finalization_failed` | Reports and acks. With Stripe Tax computing, an invoice cannot finalize when the customer's address is unusable (`automatic_tax.status = requires_location_inputs`), and Stripe keeps the subscription **active** while it cannot be finalized — entitlement continues while nothing is collected. The sink logs the invoice, customer and subscription ids, the tax status and Stripe's `last_finalization_error`, and sends one `[OPS]` email to `OPS_ALERT_EMAIL` (production only, the same knob as the Kratos probe). A retry could change nothing, so it is acknowledged; the fix is the customer's address or tax settings in the Stripe Dashboard, then finalizing the invoice there. |
| `checkout.session.async_payment_succeeded` | Logs and acks. A delayed-notification payment (ACH Direct Debit) cleared after the session completed. Entitlement is decided by the subscription's own status, so this only closes the loop the failure below opens. |
| `checkout.session.async_payment_failed` | Reports and acks. The customer's first ACH debit was returned. This is the earliest signal a new customer has not actually paid — ahead of the subscription's own status change — so it logs an error with the org, customer and subscription ids and sends one `[OPS]` email. The subscription follows to `past_due` and then `unpaid` through `customer.subscription.updated`; no state is written here. |

Anything else is acknowledged without acting. Deliveries are claimed in `stripe_webhook_events`
before anything is applied, so replays and concurrent deliveries are acknowledged without being
re-applied.

### Pigeons

#### `POST /flock/pigeons`

**Auth:** flock: manage

Creates a pigeon inside a flock. Since task #12 this is gated on the **target flock**: a
personal flock's owner, or an org owner/admin for an org-owned flock (pre-org behavior never
checked flock ownership at all — a latent gap this closed). A pigeon created inside an
org-owned flock is seeded with **both** ACL rows: the creator's own `owner` row and the org's
`owner` row (so every org member gets role-mapped access immediately, and the org's access
survives the creator leaving). Body: `capsules::PigeonCreateRequest`
(`{ flock_id, serial?, name?, tags?, connector, board? }`) — `connector` is one of
`{"Https": {"endpoint": "", "token": ""}}`, `{"Coap": {"endpoint": "", "token": ""}}` or
`{"Mqtt": {"endpoint": "", "token": ""}}`; the
`endpoint`/`token` you send are ignored and overwritten server-side (the DO mints its own
device endpoint URL and credential).

**Device-count entitlement.** An account served at the free tier (no org, or an org whose
subscription status isn't entitled) is capped at its included device count — creation past the
cap answers `403` with an upgrade hint. Paid, entitled tiers are never refused here; devices
past the included count bill as per-device overage instead. The check counts *provisioned*
pigeons, deliberately unlike the per-device meter's connected count: a pigeon occupies a
Durable Object whether or not it ever powers on. Shared rules (fail-open, growth-only) and the
refusal shape are in [Per-tier limits](#per-tier-limits).

> **CoAP is terminated by a dedicated service (`loft`), not by the edge Worker.** The `Coap`
> connector variant mints PSK credentials (`tls_psk_identity` = the pigeon's own id,
> `tls_psk_secret` = a 32-char hex PSK minted alongside the bearer token — one mint/refresh
> rotates both), and the minted `coaps://` endpoint points at the CoAP terminator's own host
> (`COAP_DEVICE_HOST`, `coap.pidgeiot.com` in production — the Workers runtime is HTTP-based
> and cannot terminate raw CoAP framing itself). The terminator serves BOTH transports on port
> 5684: CoAP-over-DTLS/UDP (`coaps://`, the primary transport for constrained cellular
> devices) and CoAP-over-TLS/TCP (`coaps+tcp://`, RFC 8323). The endpoint is minted in the
> RFC 7252 `coaps://` (DTLS/UDP) form because the device validates the scheme against its
> compiled transport and fails loudly on a mismatch — a TLS/TCP device build uses the same
> authority and substitutes the `coaps+tcp://` scheme. See
> [CoAP device surface](#coap-device-surface-via-the-loft-terminator) below, and the `loft`
> repo's `docs/infra/coap-terminator.md` for deployment.

> **MQTT is terminated by `pigeonhole`, also not by the edge Worker.** The `Mqtt` connector
> variant mints the same two credentials the `Coap` one does — the bearer token, which
> authorizes every request the broker makes upstream on the device's behalf, and a PSK pair
> (`tls_psk_identity` = the pigeon's own id, `tls_psk_secret` = a 32-char hex PSK) — because the
> broker accepts a certificate handshake and a PSK handshake on one listener and a device may
> arrive by either. The minted endpoint is `mqtts://<MQTT_DEVICE_HOST>:8883` with no path: a
> topic names the resource, and the CONNECT handshake binds the session to one pigeon. See
> [MQTT device surface](#mqtt-device-surface-via-the-pigeonhole-broker) below. `board` is optional — the pigeon's own
Zephyr `CONFIG_BOARD_TARGET` string, if known at provisioning time. Left unset, the pigeon can
never be assigned firmware over the shadow route (see [Shadow](#shadow) above's fail-closed
board-compatibility check) until it's tagged, either here or via a later `PUT`.

```sh
curl -s -X POST https://api.pidgeiot.com/flock/pigeons \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"flock_id":"<flock_id>","name":"Coop Sensor 1","connector":{"Https":{"endpoint":"","token":""}}}'
```

Response is `capsules::PigeonDetail` (`{ pigeon, acl, shadow }`) with status `201` and a
`Location: /pigeons/<pigeon_id>` header. **This is the only place besides `token/refresh` where
`connector.Https.token` (the device's bearer token) is ever returned — save it now.**

```json
{
  "pigeon": {
    "id": "59d0c929f9124dbbc2c0bbb7c429f5e918734c0c949aba02c20d7edf795c72a9",
    "flock_id": "c84932d0-160e-4007-bd72-0235d74a8033",
    "serial": null,
    "name": "Coop Sensor 1",
    "tags": null,
    "connector": {
      "Https": {
        "endpoint": "https://api.pidgeiot.com/device/pigeons/59d0c929f912...",
        "token": "<device_token>"
      }
    },
    "token_expires_at": "2027-07-17T15:39:23Z",
    "updated_at": "2026-07-17T15:39:23Z",
    "created_at": "2026-07-17T15:39:23Z"
  },
  "acl": { "entity_id": "8dc58300-70e6-4484-99f3-18ff7487b6fd", "role": "owner" },
  "shadow": { "target_version": 0, "current_version": 0, "target_config": "{}", "current_config": "{}", "updated_at": 1784302763 }
}
```

Note the pigeon's `id` is not a UUID — it's the hex string form of its Durable Object ID, and
doubles as the path segment for every other pigeon route.

**The connector is a provisioning hint, not a transport boundary.** Nothing about the variant
restricts what the device may do: a pigeon's bearer token authenticates it on the HTTPS device
routes, through `loft`, and as an MQTT CONNECT password alike, and any pigeon that minted a PSK
pair can complete a PSK handshake with either terminator. The variant records how the pigeon was
provisioned, and decides which endpoint and credentials the dashboard shows.

#### `GET /pigeons/:pigeon_id`

**Auth:** member

Returns `capsules::Pigeon` with the connector token/PSK stripped.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id> \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `GET /pigeons/:pigeon_id/detail`

**Auth:** member

Same as above plus `acl` (**only the caller's own ACL row**, not the full list — use
`GET /pigeons/:pigeon_id/acl` for that) and `shadow`. Returns `capsules::PigeonDetail`.

#### `PUT /pigeons/:pigeon_id`

**Auth:** member

Partial update. Body: `capsules::PigeonUpdateRequest` — every field (`serial`, `name`, `tags`,
`connector`, `board`) is optional; omitted fields keep their current value (`COALESCE`
semantics, not a full replace). Returns the updated `capsules::Pigeon`. This is how an existing
(pre-task-#20) pigeon gets its `board` tagged after the fact.

Flock membership is **not** settable here — this route authorizes against the pigeon alone, so
honouring a `flock_id` would write the pigeon into a flock nobody checked the caller against.
Use [`PUT /pigeons/:pigeon_id/flock`](#put-pigeonspigeon_idflock) below; a `flock_id` in this
body is ignored, as any unknown field is.

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id> \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"Coop Sensor 1 (renamed)"}'
```

#### `PUT /pigeons/:pigeon_id/flock`

**Auth:** owner + destination flock: manage

Moves a pigeon into another flock. Body: `capsules::PigeonFlockUpdateRequest`
(`{ flock_id }`). Both ends are checked: the caller must be an owner on the pigeon's own ACL,
and must manage the destination flock (its owner, or an owner/admin of the org that owns it).
Returns the updated `capsules::Pigeon`.

Source and destination must answer to the **same owner** — two personal flocks of the same
user, or two flocks of the same org — `409` otherwise. A pigeon's ACL rows live in its own
Durable Object and name that owner, so a cross-owner move would either hide the pigeon from the
flock it arrived in or leave it readable by the org it left. Moving a whole flock to an
organization is [`POST /flocks/:flock_id/transfer`](#post-flocksflock_idtransfer); there is no
route that moves one pigeon across that line.

The pigeon's own ACL is checked first, so a caller with no claim on it gets that `403` and
learns nothing about which flock it is in. An unknown destination flock is the destination
`403` (missing and forbidden are indistinguishable there too); `404` is reserved for a pigeon
with no Postgres mirror row to compare owners against.

The move is **invisible to the device**: its id, bearer token, connector endpoint and Durable
Object are all untouched, and it needs no reboot or re-provisioning. What changes is which
flock lists it, and therefore which flock-scoped firmware catalog and alerts apply to it. The
Durable Object is written first and the Postgres mirror synced best-effort after, per the usual
convention.

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/flock \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"flock_id":"<destination_flock_id>"}'
```

#### `DELETE /pigeons/:pigeon_id`

**Auth:** owner

Wipes the pigeon's Durable Object storage (its ACL, shadow, telemetry, and log tables) and
deletes its Postgres mirror row. Returns `200` with an empty body. As noted above, subsequent
`GET`s against the same ID return `403`, not `404` — the Durable Object still exists, just
empty.

#### `POST /pigeons/batch`

**Auth:** member (per pigeon)

Bulk-fetches up to 48 pigeons by ID in parallel, silently skipping any the caller isn't
authorized for or that don't exist (never errors on an individual bad ID — the response is
just shorter than the request). Body: a plain JSON array of pigeon ID strings. `400` if more
than 48 are requested.

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/batch \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '["<pigeon_id_1>","<pigeon_id_2>"]'
```

Returns `Vec<capsules::Pigeon>`.

#### `POST /pigeons/:pigeon_id/token/refresh`

**Auth:** owner

Mints a new Ed25519 keypair and device token for this pigeon, immediately revoking the old
one (see [Device authentication](#device-authentication-bearer-token) above). Returns the
updated `capsules::Pigeon` with the new token visible in `connector.Https.token` /
`connector.Coap.token` / `connector.Mqtt.token` — save it now, it won't be shown again. For the
PSK-bearing variants the refresh rotates `tls_psk_secret` in the same response, and the old PSK
stops resolving through the [service-internal route](#service-internal-api) at once.

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/<pigeon_id>/token/refresh \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `POST /pigeons/:pigeon_id/shell`

**Auth:** owner

Runs one diagnostic command on the device and returns its output — a remote shell relayed
over the pigeon's existing device WebSocket connection (see [`GET
/device/pigeons/:pigeon_id/ws`](#get-devicepigeonspigeon_idws) below), **not** a new
persistent connection of its own. The dashboard is a plain HTTP request/response client here;
there is no WebSocket on the operator side in v1. Body: `{"cmd": string, "timeout_ms"?: u32}`.

- Gated by `owner`, stricter than the `member` bar most pigeon routes use — a shell command is
  remote code execution on physical hardware by design, and this project's convention for
  high-blast-radius features is the narrowest gate that's still usable, not the most
  permissive one.
- `timeout_ms` is optional and caller-configurable, clamped server-side to a 30 second maximum
  regardless of what's requested; defaults to 10 seconds if omitted.
- `409 Conflict` if this pigeon currently has no open device WebSocket (nothing to relay
  through — a cellular/HTTPS-only device, or a WS-capable one that just isn't connected right
  now, can never receive a shell command under this design), or if a previous shell command on
  this pigeon is still awaiting a reply (v1 is one command in flight at a time, per pigeon).
- `504 Gateway Timeout` if the device doesn't reply within `timeout_ms`.
- `502 Bad Gateway` if the device's socket disconnects while a command is still in flight.
- `400` for an empty/missing `cmd`.
- On success, `200` with `{"output": string, "exit_code": int, "truncated": bool}` — same
  shape as the WebSocket `shell_output` frame's fields (see the frame table below).

Whether a specific `cmd` string actually does anything is entirely up to the device's own
command allowlist (`CONFIG_PIGEON_SHELL`, off by default) — dovecote relays whatever string is
sent and has no allowlist of its own; the device is the enforcement point.

Audit trail for v1 is log-only, not a durable Postgres table: dovecote logs the requesting
user, pigeon, and command text via `console_log!` before relaying, and the device independently
logs the same locally (see the device library's own docs) before executing.

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/<pigeon_id>/shell \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"cmd":"pigeon shadow","timeout_ms":5000}'
```

```json
{"output":"target_version=2 current_version=1\n","exit_code":0,"truncated":false}
```

### ACL

Roles are free-form strings; `"owner"` is the only one dovecote treats specially. Both ACL
routes require the caller to already hold the `"owner"` role on this pigeon.

#### `GET /pigeons/:pigeon_id/acl`

**Auth:** owner

Lists every ACL entry for the pigeon (`Vec<capsules::PigeonAcl>`), not just the caller's own
row.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/acl \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `POST /pigeons/:pigeon_id/acl`

**Auth:** owner

Upserts an ACL entry (insert, or update the role if `entity_id` already has one). Body:
`capsules::PigeonAclUpdateRequest` (`{ entity_id, role }`). Returns the entry you just set as
`capsules::PigeonAcl`.

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/<pigeon_id>/acl \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"entity_id":"<other_user_uuid>","role":"member"}'
```

### Shadow

The "shadow" is a desired/reported config pair, modeled after AWS IoT Device Shadows: the
dashboard sets `target_config`; the device reports back `current_config` once it's applied it.
`target_version` auto-increments every time `target_config` changes (a SQLite trigger inside the
Durable Object), giving devices a cheap way to detect "there's a newer target than what I last
applied." Read *changes* literally: a `PUT` whose `target_config` is identical to the stored one
leaves `target_version` exactly where it was, which matters to anything device-side that treats a
new version as an instruction. See "re-pushing the same firmware target" under the `PUT` below.

**Asymmetry to know about:** in *request* bodies, `target_config`/`current_config` are native
JSON objects (`serde_json::Value`). In every *response*, they come back as `capsules::JsonString`
— which serializes as a **JSON string containing JSON text**, not a nested object. You'll need a
second `JSON.parse()` (or equivalent) on those two fields specifically. This is a deliberate
wire-format choice (see `capsules::PigeonShadow`'s doc comment), not a bug.

#### `GET /pigeons/:pigeon_id/shadow`

**Auth:** member

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/shadow \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
{
  "target_version": 1,
  "current_version": 0,
  "target_config": "{\"telemetry_interval\":60}",
  "current_config": "{}",
  "updated_at": 1784302765
}
```

(`updated_at` is intentionally a raw unix-seconds integer here, not RFC 3339 — it's parsed by
device-side Zephyr firmware, where a minimal wire size matters.)

#### `PUT /pigeons/:pigeon_id/shadow`

**Auth:** member

Sets a new `target_config`, bumping `target_version`. Body: `capsules::PigeonShadowUpdateRequest`
(`{ target_config: <any JSON object> }`).

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/shadow \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"target_config":{"telemetry_interval":60}}'
```

**Firmware assignment (task #23) reuses this route** — there's no separate "assign firmware"
endpoint. Merge a `firmware` key into `target_config` (see `capsules::FirmwareTarget`), using
`version`/`size`/`sha256` from one of the flock's uploaded images (see
[Firmware](#firmware) below):

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/shadow \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"target_config":{"firmware":{"version":"0.1.0+0","size":393802,"sha256":"<64-char lowercase hex>"}}}'
```

Old firmware that predates FOTA ignores the unknown `firmware` key entirely (Zephyr's
`json_obj_parse` skips unknown keys), so this is backward-compatible with already-deployed
devices. The device picks this up on its next shadow poll and pulls the image via
[`GET /device/pigeons/:pigeon_id/firmware`](#get-devicepigeonspigeon_idfirmware) below.

**Re-pushing the same firmware target.** A device may bound how many times it will chase one
firmware target, and the sane way to key that budget is the shadow's `target_version` rather than
the firmware version string alone, so that an operator can authorize another try without
republishing unchanged bytes under a new label (`pigeon`'s `CONFIG_PIGEON_FOTA_ATTEMPT_BUDGET`
works exactly this way). What reopens such a budget is therefore a **new `target_version`**, and
this route only produces one when `target_config` actually differs from what is stored. Sending
the identical config back is a no-op as far as any device can tell.

To re-assert a firmware target, keep the `firmware` object byte-identical and advance a top-level
`firmware_repush` integer alongside it:

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/shadow \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"target_config":{"firmware":{"version":"0.1.0+0","size":393802,"sha256":"<64-char lowercase hex>"},"firmware_repush":1}}'
```

Nothing device-side reads `firmware_repush`; its whole job is to make the config differ so the
version moves while what the device applies does not. Two properties of that key are deliberate.
It sits **outside** the `firmware` object, because a device decodes `firmware` into a fixed struct
and an unknown field there would be an unexpected key inside the thing it is about to flash,
whereas an unknown key at the top level is what every app's decoder already skips. And it is a
small integer, because the device library caps one decoded config at
`CONFIG_PIGEON_SHADOW_CONFIG_MAX` bytes (320 by default) and a config truncated past that fails to
parse rather than degrading.

The dashboard does this for you: the pigeon detail page's **Re-push firmware** button (next to
Shadow → Edit, shown only when `target_config` already carries a `firmware` key) sends exactly
this `PUT` and reports the new `target_version`.

**Board/geometry compatibility check (task #20, phase 1) — fail-closed.** Whenever this PUT's
`target_config` contains a `firmware` key, dovecote looks up the target image's `board` (matched
by `sha256` against this pigeon's flock's firmware catalog, see [Firmware](#firmware) below) and
compares it against this pigeon's own `board` (`capsules::Pigeon::board`, set at provisioning or
via `PUT /pigeons/:pigeon_id`). The write is rejected with `400` **unless both are set and
equal** — an unset pigeon board, an unset/untagged image, or an explicit mismatch all reject.
This is deliberately fail-closed, not fail-open on unset fields: a firmware image built for one
board's flash/partition geometry, applied on a device with a different geometry, can halt the
device until a manual reset (a real incident this closes, not a hypothetical) — see the task #20
design doc for the full incident writeup and why the MCUboot image header itself can't carry
this information for this fleet's swap-mode builds. **Every pigeon and every firmware image must
be explicitly tagged with a matching `board` before an assignment will succeed** — there's no
grandfather exemption for pre-existing untagged rows.

### Firmware

Firmware images (signed MCUboot application binaries) are catalogued per-**flock**, not
per-pigeon — they're shared across every pigeon in a flock's hardware fleet rather than
duplicated per-pigeon. The binary itself lives in R2, content-addressed by `sha256`
(`firmware/<sha256>.bin`); only metadata lives in Postgres. A pigeon's *assigned* firmware is a
separate, per-pigeon concern set via its own shadow (see above), not here.

#### `POST /flocks/:flock_id/firmware`

**Auth:** flock: manage

**Query:** `version=<string>&board=<string>`, both required

Uploads a firmware image. The request body **is** the image, sent as raw bytes (like
`POST /device/pigeons/:pigeon_id/logs`, not wrapped in JSON). `size` and `sha256` are always
computed server-side from the uploaded bytes — never trust a client-supplied hash.

`board` (task #20, phase 1) is the Zephyr `CONFIG_BOARD_TARGET` string this image was built for
— e.g. `circuitdojo_feather/nrf9160/ns` or `esp32c6_devkitc/esp32c6/hpcore` — the exact string
passed to `west build -b <this>` for the sample that produced it. **Required on every upload**;
there's no way to upload an untagged image going forward (only pre-existing rows from before this
field existed stay untagged). This is what the shadow PUT's board-compatibility check (above)
matches against a pigeon's own `board`.

- `400` if `version` or `board` is missing/empty, or the body is empty.
- `403` if the caller isn't the flock's owner.
- `413 Payload Too Large` if the body exceeds ~2 MiB (`capsules::MAX_FIRMWARE_BYTES`).

Re-uploading identical bytes to the same flock (even under a different `version` label) updates
the existing catalog row in place rather than creating a duplicate — both the Postgres row and
the R2 object are content-addressed by `(flock_id, sha256)`/`sha256` respectively; a re-upload's
`board` overwrites whatever was previously stored for that row.

```sh
curl -s -X POST 'https://api.pidgeiot.com/flocks/<flock_id>/firmware?version=0.1.0+0&board=circuitdojo_feather/nrf9160/ns' \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  --data-binary @https_init.signed.bin
```

```json
{
  "id": "b3f1...",
  "flock_id": "a1c2...",
  "version": "0.1.0+0",
  "size": 393802,
  "sha256": "9f2a...",
  "board": "circuitdojo_feather/nrf9160/ns",
  "uploaded_at": "2026-07-17T15:21:08Z"
}
```

(`capsules::FirmwareImage`.)

#### `GET /flocks/:flock_id/firmware`

**Auth:** flock: view

Lists every firmware image uploaded for this flock, newest first. Same per-item shape as the
`POST` response above.

```sh
curl -s https://api.pidgeiot.com/flocks/<flock_id>/firmware \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

### Telemetry

Every telemetry value, on both the DO's latest-value store and the Postgres history table, is
stored and returned as a **string** — dovecote doesn't know or enforce a schema for what a
device reports. Where a value happens to parse as a number, the history endpoints also populate
a `value_num` float alongside the raw string, so numeric series can be queried/plotted without a
client-side cast.

**A report merges, it doesn't replace.** The pigeon's latest-value store keeps a per-key value
*and* a per-key `reported_at`; a report stamps only the keys it carries, and every other key
keeps the value and timestamp it already had. So a key a device reports once at boot
(`reset_cause`) keeps its boot timestamp while `uptime_s` moves with every report, and the
dashboard's connection-state badge and the `RateOfChange` alert condition both read the per-key
timestamp rather than a single store-wide one. The whole store lives in one JSON object in one
Durable Object SQLite row (DO SQLite bills per row read and written, and this is the platform's
hottest write path), which is what the key cap below bounds.

**Batched reports.** A device may deliver several timestamped readings in one request instead
of one reading per request — see [`POST /device/pigeons/:pigeon_id/telemetry`](#post-devicepigeonspigeon_idtelemetry)
for the body, and the WebSocket [`telemetry` frame](#get-devicepigeonspigeon_idws) for the same
shape over a socket. A batch merges reading by reading in chronological order, so the store ends
up holding the newest value per key exactly as if the readings had arrived one at a time, each
key stamped with its own reading's timestamp rather than the delivery's. A reading older than
what the store already holds for a key updates history but does not move the stored latest value
backwards.

**Device timestamps are advisory, and are clamped.** The `pigeon` device library has no wall
clock — no RTC, and NTP was removed in 0.13.6 — so the form its firmware can actually fill in is
`age_secs`, "this reading was taken this many seconds before I sent the batch". `at` (absolute
unix seconds) is there for clients that do have a clock, such as a gateway or bridge relaying on
a device's behalf. Neither is trusted:

- Both resolve against the **server's** receive time, and the result is clamped into
  `[now - capsules::MAX_TELEMETRY_BACKDATE_SECS, now]` (24 h).
- A timestamp in the future clamps to the receive time. A reading cannot be placed past the end
  of a range a dashboard would query, and a fast clock cannot post readings into a billing
  period that has not started.
- A timestamp older than the backdate window clamps to that boundary rather than being refused,
  so a device that was offline for a week still delivers its buffer — visibly stacked at the
  boundary — instead of losing it.
- `age_secs` wins if a client sends both. Filling it in is a statement that the client does not
  trust its own clock, and the relative form survives a wrong one.
- Readings are sorted chronologically after clamping, so a device may send them in any order.
  Readings that resolve to the same second keep the order they were sent in.

**A batch is billed by its readings, not by its envelope.** A batch of M readings counts as M
billable device messages against the account's allowance, exactly as M separate reports would
have. The meter measures data delivered; batching is a change to what delivery costs *us*, not a
discount on what a customer sent. The count is charged when the batch **arrives**, never against
the timestamps the readings carry, so a backdated reading always bills to the period it landed
in — the one rule a device cannot move by lying about its clock. The [free-tier
fuse](#post-devicepigeonspigeon_idtelemetry), by contrast, is a per-request gate: a paused
account's batch is refused whole with one `429`.

**What batching actually saves.** Per-device-month COGS at the chatty profile (one reading every
10 s, 259,200 readings, 5 keys each), computed on the same Cloudflare rate card as the pricing
model:

| Readings per delivery | COGS/device-month | vs. unbatched |
|---:|---:|---:|
| 1 (unbatched) | $0.9057 | — |
| 6 (one delivery/minute) | $0.2691 | 3.4x cheaper |
| 12 | $0.2054 | 4.4x |
| 30 | $0.1672 | 5.4x |
| 64 (the cap) | $0.1537 | 5.9x |

The saving comes from what a batch collapses: one worker request, one verify hop, one queue
message (3 queue operations), one Durable Object round trip, one blob row written, one history
INSERT, one line-protocol POST — for the whole batch rather than for each reading. Those four
line items are 94% of the unbatched figure. What does **not** collapse is what measures readings
rather than envelopes: history rows, line-protocol lines, and the billable count. Two footnotes
worth knowing. The figures above hold the device's *shadow* poll at its original 10 s cadence,
which is the single largest surviving cost; a device that also polls on its delivery cadence (or
takes `shadow_update` pushes over the WebSocket instead of polling) reaches $0.156 at M=6 and
$0.020 at M=64. And Postgres history growth — ~168 MB/device-month, ~$0.017/month and cumulative
— is untouched by any of this, so past the first few months it dominates a batched device's cost
and a retention policy matters more than it used to.

**Key cap and eviction.** A pigeon's store holds at most `capsules::MAX_TELEMETRY_KEYS` = 128
distinct keys. Past that, the least-recently-reported keys are evicted to make room — silently,
not an error — so a device that renames its keys across firmware versions sheds the abandoned
ones instead of accumulating them forever. A report's own keys are never the ones evicted. A
single report carrying more than 128 keys is refused whole with `400` (nothing partially
applied), as is one carrying a key over 128 bytes or a value over 1024 bytes. For reference,
the `pigeon` device library's own per-report ceiling (`CONFIG_PIGEON_TELEMETRY_MAX_KEYS`)
defaults to 8 and maxes out at 64, and it truncates keys at 31 bytes and values at 127.

#### `GET /pigeons/:pigeon_id/telemetry`

**Auth:** member

Latest value per key, straight from the pigeon's own Durable Object (not Postgres) — always
fresh, but no history.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/telemetry \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
[
  { "key": "temp", "value": "21.5", "reported_at": "2026-07-17T15:34:41Z" },
  { "key": "status", "value": "ok", "reported_at": "2026-07-17T15:34:41Z" }
]
```

(`Vec<capsules::TelemetryLatest>`.)

#### `GET /pigeons/:pigeon_id/telemetry/history`

**Auth:** member

Time-series read from Postgres. All query params are optional:

| Param | Type | Meaning |
|---|---|---|
| `key` | string | filter to one metric key; omit for all keys |
| `keys` | comma-separated strings | filter to several keys at once, e.g. `keys=gps_lat,gps_lon`. Merged with `key` if both are sent; blank entries are ignored, so `keys=` behaves as no filter |
| `since` | RFC 3339 timestamp | inclusive lower bound on `reported_at` (bucketed mode: defaults to `until` minus 24h if omitted) |
| `until` | RFC 3339 timestamp | inclusive upper bound on `reported_at` (bucketed mode: defaults to now if omitted) |
| `raw` | `true`/omit | see [Raw mode](#raw-mode) below |

**Default (bucketed) response.** A point is one key at one timestamp, so a device reporting more
than a couple of keys blows past any fixed response cap within a day or so regardless of how long
a range was asked for — truncating a response at a row count always hits that wall eventually,
just later. This route avoids it by bucketing instead: the requested range is divided into
`capsules::TELEMETRY_HISTORY_BUCKET_TARGET` (360) buckets and aggregated in SQL, so the response
size is roughly constant no matter the range or key count, and every range is fully drawable.

```sh
curl -s "https://api.pidgeiot.com/pigeons/<pigeon_id>/telemetry/history?keys=gps_lat,gps_lon" \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
[
  {
    "pigeon_id": "59d0c929f912...",
    "key": "temp",
    "bucket_start": "2026-07-17T15:30:00Z",
    "min": 21.1,
    "max": 21.8,
    "mean": 21.5,
    "last": "21.5",
    "count": 12
  }
]
```

(`Vec<capsules::TelemetryHistoryBucket>`, ascending by `bucket_start`.) `min`/`max`/`mean` are
`null` for a bucket whose values never parsed as numeric (e.g. a firmware version string) — `last`
(the most recent raw value in the bucket) is always present, since a bucket always has at least
one report backing it. `count` is how many reports landed in the bucket, not how many of those
were numeric. There is no truncation here and no `X-Telemetry-Truncated` header: bucketing bounds
the response by construction, so there's nothing to cut.

##### Raw mode

`?raw=true` gets the pre-bucketing shape instead: a flat `TelemetryHistoryPoint` per key per
report, capped at `capsules::TELEMETRY_HISTORY_MAX_POINTS` = 5000 points, byte-identical to this
route's behavior before bucketing existed. Meant for a caller that needs real per-report values
rather than a bucket's aggregate — pairing `gps_lat`/`gps_lon` from the same report is the
motivating case, since a bucket's mean can't reconstruct a track.

```sh
curl -s "https://api.pidgeiot.com/pigeons/<pigeon_id>/telemetry/history?raw=true&keys=gps_lat,gps_lon" \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
[
  {
    "pigeon_id": "59d0c929f912...",
    "key": "temp",
    "value": "21.5",
    "value_num": 21.5,
    "reported_at": "2026-07-17T15:34:41.389358Z"
  }
]
```

(`Vec<capsules::TelemetryHistoryPoint>`, oldest first.)

**When a range holds more than the cap, you get its newest 5000 points, not its oldest** — the
window always ends at the end of the range you asked for, so the live edge of a chart is never
the part that gets dropped. Because a single report can carry several keys, each one becomes its
own point in the response, so a handful of keys at a short interval passes 5000 within a day —
this is exactly the case bucketed (non-`raw`) mode above exists to avoid. Every raw-mode response
says which case it was:

| Header | Meaning |
|---|---|
| `X-Telemetry-Truncated: false` | the full range fits; you have all of it |
| `X-Telemetry-Truncated: true` | the range held more; you have its newest 5000 points |

The header is listed in `Access-Control-Expose-Headers`, so a browser client can read it. Narrow
the range or the key set to see the rest.

**Backing store (task #26, revised by the Postgres consolidation).** Both modes read from
whichever store actually holds this data: the platform's Postgres `pigeon_telemetry_history`
table by default. A GreptimeDB store remains supported per environment for **raw mode only**
(when `GREPTIMEDB_ENDPOINT` is configured — see `helpers/greptime.rs`; no deployed environment
currently sets it, see `docs/infra/postgres-consolidation.md`), in which case reads go there
first and fall back to Postgres on a query error; bucketed mode always reads Postgres directly.
This is transparent to the caller within a mode — the response shape is identical either way raw
mode's own data came from. **Only populated for reports made while the pigeon had no
`telemetry_endpoint` configured** — see the next section for the per-pigeon override, which still
takes precedence over the platform default in both directions (write and, indirectly, read: an
overridden pigeon's data never lands in the platform's own history store at all, only at the URL
you configured).

#### `GET /flocks/:flock_id/telemetry/history`

**Auth:** flock: view

Same shape and query params as above (bucketed by default, `raw=true` for the flat/capped shape),
across every pigeon in the flock. Unlike the pigeon-scoped route, this checks *flock*-level access
(`authorize_flock` — personal owner, or any org role on an org-owned flock), not any pigeon's ACL
— so a pigeon shared with you via its own ACL, but living in a flock you have no flock-level
access to, won't show up here even though `GET /pigeons/:pigeon_id/telemetry/history` would work
for it directly.

```sh
curl -s "https://api.pidgeiot.com/flocks/<flock_id>/telemetry/history?since=2026-07-17T00:00:00Z" \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `PUT /pigeons/:pigeon_id/telemetry-endpoint`

**Auth:** member

Sets or clears a per-pigeon forwarding target: when configured, every telemetry report for
this pigeon is forwarded as an **InfluxDB line protocol v2 HTTP write** (GreptimeDB-compatible)
to that endpoint *instead of* the platform's own history store (task #26: Postgres by default,
or GreptimeDB where an environment configures it — see [above](#get-pigeonspigeon_idtelemetryhistory)).
The Durable Object's own latest-value table (`GET /pigeons/:pigeon_id/telemetry`) is unaffected
either way — it always gets written.

Body: `capsules::PigeonTelemetryEndpointUpdateRequest` — `{"telemetry_endpoint": {...}}` to
set/replace, or `{"telemetry_endpoint": null}` to clear (revert to the platform default).
`capsules::TelemetryEndpoint` is `{ url, db?, auth_token? }` — `url` is the full write endpoint
(dovecote only appends `precision`/`db` query params, it doesn't assume a fixed path), `db` is
an optional target database name, `auth_token` is sent as `Authorization: Token <auth_token>` on
the outbound write if set.

**`auth_token` handling is asymmetric by design:** the response to *this* route echoes back
whatever `auth_token` you just sent (same exemption as the connector token on
create/`token/refresh`) — but every subsequent `GET` that returns this pigeon (`GET
/pigeons/:pigeon_id`, `/detail`, etc.) has it stripped to `null`. Don't expect to read it back
later.

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/telemetry-endpoint \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"telemetry_endpoint":{"url":"https://greptime.example.com/v1/influxdb/write","db":"pidgeiot","auth_token":"<endpoint_token>"}}'
```

```json
{"url":"https://greptime.example.com/v1/influxdb/write","db":"pidgeiot","auth_token":"<endpoint_token>"}
```

To clear:

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/telemetry-endpoint \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"telemetry_endpoint":null}'
```

### Logs

#### `GET /pigeons/:pigeon_id/logs`

**Auth:** member

Returns every currently-stored device log chunk for this pigeon, oldest first, as
base64-encoded binary (see [device logs](#post-devicepigeonspigeon_idlogs) below for what's
actually in them — dovecote treats the bytes as opaque). At most the 200 most recently received
chunks are kept per pigeon; older ones are silently pruned on ingest, not deleted via this
route.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/logs \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
[
  { "id": 1, "data": "AQLerb4AA...", "received_at": "2026-07-17T15:21:08Z" },
  { "id": 2, "data": "/wCqu...", "received_at": "2026-07-17T15:21:09Z" }
]
```

(`Vec<capsules::PigeonLogChunk>`. `id` is a per-pigeon autoincrement, not globally unique.)

### Log dictionary

The chunks above are Zephyr `CONFIG_LOG_DICTIONARY_SUPPORT` binary records — decodable only
against the producing firmware build's own `log_dictionary.json` (generated at build time by
Zephyr's `database_gen.py`; a dictionary from any other build yields garbage strings). These
routes let a dashboard user store that file **per pigeon**, so the dashboard's log viewer can
decode the chunks in-browser instead of only offering a raw download. Per-pigeon, not
per-flock, because pigeons in one flock may run different builds.

The JSON document is stored verbatim in R2 (`log-dictionaries/<pigeon_id>.json`, under the same
bucket as firmware images); dovecote validates it parses as JSON but otherwise treats the
schema as opaque — Zephyr's tooling and the dashboard's decoder are the consumers, not the
backend. All three routes are **member**-gated (any ACL row on the pigeon), same bar as
`GET /pigeons/:pigeon_id/logs`.

**Upload only a sanitized dictionary.** `database_gen.py` collects the image's whole static
rodata string pool so that `%s` pointers resolve, so a build that bakes a credential (a device
token, a PSK) ships that value verbatim as a `string_mappings` entry, and every member of the
org can read it back through the `GET` route below. Replace such a value with a placeholder
before uploading: lookup is by address, so no real log line decodes differently.

#### `PUT /pigeons/:pigeon_id/log-dictionary`

**Auth:** member

Uploads (or replaces) this pigeon's dictionary. The request body **is** the
`log_dictionary.json` document, sent as raw bytes (like the firmware upload, not wrapped in an
outer JSON envelope).

- `400` if the body is empty or not valid JSON.
- `413 Payload Too Large` if the body exceeds 4 MiB (`capsules::MAX_LOG_DICTIONARY_BYTES`).

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id>/log-dictionary \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  --data-binary @build/zephyr/log_dictionary.json
```

Response is `capsules::LogDictionaryInfo` — size plus the `build_id`/`version` fields dovecote
found inside the uploaded document (`null` where absent):

```json
{ "size": 11913, "build_id": "v4.4.1", "version": 3 }
```

#### `GET /pigeons/:pigeon_id/log-dictionary`

**Auth:** member

Returns the stored dictionary verbatim (`Content-Type: application/json` — the raw Zephyr
database document, **not** a capsules type). `404` if none has been uploaded for this pigeon.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/log-dictionary \
  -H 'Cookie: ory_kratos_session=<session_token>' -o log_dictionary.json
```

#### `DELETE /pigeons/:pigeon_id/log-dictionary`

**Auth:** member

Removes the stored dictionary. Returns `200` with an empty body; idempotent (deleting when
none exists is still `200`). Deleting the pigeon itself also best-effort removes its stored
dictionary.

### Alerts

User-defined threshold/state alerts, evaluated both at telemetry-ingest time and by a five-minute
Cron Trigger sweep (for the absence-of-signal conditions below), with an at-most-one email per
fired/cleared transition. That email (`capsules::format_alert_email`, HTML plus a plain-text
part that says the same thing; subject `[PidgeIoT] Alert firing: <metric> on <pigeon>`,
`Critical alert firing: ...` for a critical-severity definition, or `Alert resolved: ...`
regardless of severity, where `<metric>` is the telemetry key or `device offline`/`device
stale`/`missing reports`) names the pigeon and flock, the condition and its threshold, the
value the evaluator observed, the transition time in UTC, the current state, a link to the
pigeon's dashboard page and a link to the alerts section the definition is edited from. An
alert is scoped to exactly one **pigeon** or one **flock** — never
both — chosen by which of the two create/list route pairs below you call; scope is never read
from the request body. A flock-scoped alert evaluates independently per pigeon currently in that
flock, not once for the flock as a whole.

`capsules::AlertCondition`/`AlertChannel`/`AlertScope` are plain Rust enums with no `#[serde(tag =
...)]` attribute, so they serialize the default serde way — **externally tagged, one key named
for the exact Rust variant** — not the `{"type": "...", ...}` shape other fields on this page use.
`capsules::AlertCondition` (the `condition` field below) is one of:

- `{"Threshold":{"key":"temp","comparator":"Gt","value":30.0}}` — a telemetry key crosses a bound
  (`comparator` is one of `Gt`/`Gte`/`Lt`/`Lte`/`Eq`).
- `{"DeviceState":{"state":"Offline","min_duration_secs":300}}` — the pigeon's own connection-state
  classification (`capsules::ConnectionStateKind`, `"Offline"` or `"Stale"`) has held for at least
  this long.
- `{"MissingReport":{"max_silence_secs":600}}` — no telemetry of any kind reported in at least
  this long.
- `{"RateOfChange":{"key":"temp","max_delta":5.0,"window_secs":300}}` — a key's numeric value has
  moved by more than `max_delta` since its previous report, within an optional time window.

`capsules::AlertChannel` is `{"Email":{"to":[]}}` (deliver to the owning flock's stored
`owner_email`) or `{"Email":{"to":["you@example.com","oncall@example.com"]}}` — up to
`capsules::MAX_ALERT_RECIPIENTS` (8) explicit addresses. **Each recipient gets its own copy** of
the message, so a bounce for one costs only that delivery and nobody learns who else is on the
alert; the fired/cleared transition is still decided once per (definition, pigeon), so the
debounce fires for the whole list or for none of it.

Every address has to be one the platform already ties to this account — the caller's own
**verified** Kratos addresses, the owning flock's `owner_email`, or the stored address of a
member of the organization that owns that flock. Anything else refuses the whole request with
`400`, so open signup can't turn alert mail into an arbitrary spam relay; the same check runs
again at send time against the flock and its organization, and drops a recipient that has since
lost its claim. Addresses are lowercased and de-duplicated on write. For compatibility with
definitions written before the list, `to` also accepts `null` or a single bare string.

`notes` is free text the operator writes for whoever reads the notification — which breaker to
check first, a runbook link. Up to `capsules::MAX_ALERT_NOTES_BYTES` (1024) bytes, `null` when
unset, and rendered above the alert's facts in the email, one paragraph per line, escaped.

Times in the notification follow the owning organization's zone, described under
[Email timestamps](#email-timestamps).

#### `POST /pigeons/:pigeon_id/alerts`

**Auth:** member

Body: `capsules::AlertDefinitionCreateRequest` (`{ name, condition, severity?, channel, notes? }`;
`severity` is `"Warning"` or `"Critical"`, defaulting to `"Warning"`).

**Alert-count entitlement.** `403` past the owning account's tier's alert count (see
[Per-tier limits](#per-tier-limits)). The limit spans every alert the account owns — pigeon-
and flock-scoped alike, across all of its flocks — not just this pigeon's.

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/<pigeon_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"High temp","condition":{"Threshold":{"key":"temp","comparator":"Gt","value":30.0}},"channel":{"Email":{"to":["you@example.com"]}},"notes":"Check the vent breaker first."}'
```

```json
{
  "id": "b3f1...",
  "user_id": "a7e2...",
  "scope": { "Pigeon": "59d0c929f912..." },
  "name": "High temp",
  "condition": { "Threshold": { "key": "temp", "comparator": "Gt", "value": 30.0 } },
  "severity": "Warning",
  "channel": { "Email": { "to": ["you@example.com"] } },
  "notes": "Check the vent breaker first.",
  "enabled": true,
  "created_at": "2026-07-17T15:21:08Z",
  "updated_at": "2026-07-17T15:21:08Z"
}
```

(`capsules::AlertDefinition`, `201`. A flock-scoped alert's `scope` is `{"Flock":"<flock_uuid>"}`
instead.)

#### `GET /pigeons/:pigeon_id/alerts`

**Auth:** member

Every alert scoped directly to this pigeon, newest first — **not** flock-scoped alerts that
happen to cover it (see [`GET /flocks/:flock_id/alerts`](#get-flocksflock_idalerts) for those).
Same per-item shape as the `POST` response above, as `Vec<capsules::AlertDefinition>`.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `GET /pigeons/:pigeon_id/alerts/state`

**Auth:** member

Current fired/cleared status for every alert scoped directly to this pigeon — a separate route
from the definitions list above because state (`capsules::AlertState`) and definitions
(`capsules::AlertDefinition`) are different rows with different lifecycles: a freshly created
alert has no state row until the evaluator has run against it at least once, and a flock-scoped
alert's state is per-*pigeon* (see below), not something that would fit as a single field
embedded onto one `AlertDefinition`. Absence of a row for a given `alert_definition_id` means the
same thing as `"Ok"` for counting purposes — it has never fired.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/alerts/state \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

```json
[
  {
    "alert_definition_id": "b3f1...",
    "pigeon_id": "59d0c929f912...",
    "status": "Firing",
    "first_true_at": "2026-08-11T15:20:08Z",
    "last_notified_at": "2026-08-11T15:21:08Z"
  }
]
```

(`Vec<capsules::AlertState>`. `status` is `"Ok"` or `"Firing"` — note this is the derived-Serialize
casing, distinct from the lowercase `"ok"`/`"firing"` `alert_state.status` is stored as in
Postgres; `first_true_at`/`last_notified_at` are `null` while `"Ok"` and never having fired.)

#### `POST /flocks/:flock_id/alerts`

**Auth:** flock: manage

Same body/response shape as the pigeon-scoped `POST` above, with `scope: {"Flock":"<flock_id>"}`
in the response. Stricter than pigeon-scoped creation: only a flock **manager** (personal owner,
or an `owner`/`admin` org role on an org-owned flock) may create a flock-scoped alert, whereas any
ACL'd pigeon member may create a pigeon-scoped one. Subject to the same account-wide
alert-count entitlement as the pigeon-scoped route (see
[Per-tier limits](#per-tier-limits)).

```sh
curl -s -X POST https://api.pidgeiot.com/flocks/<flock_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"Fleet offline","condition":{"DeviceState":{"state":"Offline","min_duration_secs":300}},"severity":"Critical","channel":{"Email":{"to":[]}}}'
```

#### `GET /flocks/:flock_id/alerts`

**Auth:** flock: view

Every alert scoped to this flock, newest first, as `Vec<capsules::AlertDefinition>`.

```sh
curl -s https://api.pidgeiot.com/flocks/<flock_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `GET /flocks/:flock_id/alerts/state`

**Auth:** flock: view

Flock counterpart of the pigeon-scoped state route above. A flock-scoped alert can appear more
than once here — one `AlertState` row per pigeon currently in the flock that the evaluator has
run it against, since it fires and clears independently per pigeon. Counting rows where
`status == "Firing"` (across this route and the pigeon-scoped one) is the "open alerts" count —
there is currently no single fleet-wide alert route, so a fleet-wide total still costs one call
per flock, same limitation `GET /flocks/:flock_id/alerts` already has for definitions.

```sh
curl -s https://api.pidgeiot.com/flocks/<flock_id>/alerts/state \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `PUT /alerts/:alert_id`

**Auth:** alert owner

Partial update — an omitted field keeps its current value. Body:
`capsules::AlertDefinitionUpdateRequest` (`{ name?, condition?, severity?, channel?, notes?,
enabled? }`). An omitted `notes` keeps the stored notes; an empty string clears them.
Gated by a direct `alert_definitions.user_id` check (whoever created the alert), regardless of
whether it's pigeon- or flock-scoped — **not** the pigeon's ACL or the flock's ownership. The
`enabled`-only body is also how the dashboard's list-view toggle flips an alert on/off without a
full edit.

```sh
curl -s -X PUT https://api.pidgeiot.com/alerts/<alert_id> \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"enabled":false}'
```

#### `DELETE /alerts/:alert_id`

**Auth:** alert owner

Same ownership gate as `PUT` above. Returns `200` with an empty body. `alert_state` rows for this
definition cascade-delete via the table's own foreign key.

### Feedback

#### `POST /feedback`

**Auth:** no auth required (optionally authenticated)

The dashboard's feedback form. Unlike every other Dashboard route, this one does **not**
require a Kratos session — public marketing pages link the same form. If a valid session cookie
*is* present, dovecote resolves it server-side and includes the submitter's identity id/email in
the notification email; the submitter is never trusted from the request body.

Body: `capsules::FeedbackRequest`. Only `message` is required; `category` is one of `"bug"`,
`"feature_request"`, `"general"`, `"problem"` (treated as general when omitted). `"problem"`
is the dashboard's persistent "Report a problem" flow, which also auto-attaches a
`diagnostics` string (the recent-request breadcrumb trail — method + route template + status,
never bodies — plus the app build hash) so the report is debuggable without a reproduction.

```sh
curl -s -X POST https://api.pidgeiot.com/feedback \
  -H 'Content-Type: application/json' \
  -d '{"message":"The shadow editor loses my edits.","category":"bug","contact_email":"me@example.com","page_context":"/flocks/abc/pigeons/def"}'
```

Returns `202` with an empty JSON object. `202`, not `200`/`201`, because nothing is persisted —
the submission is formatted (`capsules::format_feedback_email`) and delivered best-effort as one
email to the `OPS_ALERT_EMAIL` var via the existing Resend transport
(`helpers/feedback.rs::send_feedback_email`). `OPS_ALERT_EMAIL` is set in production's `[vars]`
block only (same single-knob convention as the ops health probe), so staging/dev accept the
request and log the formatted email instead of sending — the `202` never depends on delivery.

Rejections:

- `400` if `Content-Type` is not `application/json`, the JSON is invalid, `message` is empty,
  `category` is an unknown value, or `contact_email`/`page_context` exceed their length caps.
- `413` if the raw body exceeds `capsules::MAX_FEEDBACK_BODY_BYTES` or `message` exceeds
  `capsules::MAX_FEEDBACK_MESSAGE_BYTES` (see the size-limits table above).

There is no per-IP rate limiting in-route on this one (see "Rate & size limits" above —
`POST /errors` and `POST /contact` are the routes that carry one); platform-level protection
(a Cloudflare WAF rate rule on `POST /feedback`, or Turnstile) is the intended follow-up if
abuse appears.

### Contact

#### `POST /contact`

**Auth:** no auth required (optionally authenticated)

The public contact form at `https://pidgeiot.com/contact/`, which every "Contact" and "Talk to
us" link on the site now opens. Unauthenticated by definition: the people it exists for do not
have accounts yet. If a valid session cookie happens to be present it is resolved server-side
and stored alongside the enquiry, so a note from an existing user is recognisable; the
submitter is never trusted from the request body.

Body: `capsules::ContactRequest`. `name`, `email` and `message` are required. `company` is
optional free text; `fleet_size` is optional and, when present, one of `"under_50"`,
`"50_to_250"`, `"250_to_1500"`, `"1500_to_10000"`, `"over_10000"`, `"not_sure"` (an unknown
value fails deserialization, surfacing as a `400` rather than being silently coerced); `about`
is an optional lowercase slug naming the link that opened the form (`"fleet"` from the pricing
page's Fleet tier).

Two further fields exist only as abuse controls and are not part of the enquiry:

- `website` — a honeypot. The form renders it off-screen with `aria-hidden` and
  `tabindex="-1"`, so a real browser always sends it empty. A non-empty value answers `202`
  exactly like a success and stores and mails **nothing**: telling a script which control
  caught it tells it what to change.
- `elapsed_ms` — milliseconds between the form mounting and the submit click. Under
  `capsules::MIN_CONTACT_FILL_MS`, or absent entirely, is a `400` with a message inviting a
  retry (recoverable on purpose: the floor is one no human reaches, but silently discarding a
  real enquiry is the worse failure).
- `turnstile_token` — the one-time token Cloudflare Turnstile's widget issued to the browser.
  Verified against `https://challenges.cloudflare.com/turnstile/v0/siteverify` (with
  `CF-Connecting-IP` as `remoteip`) whenever the `TURNSTILE_SECRET` Worker secret is set; spent
  at verification, never stored. See "Turnstile" below for what each failure answers.

```sh
curl -s -X POST https://api.pidgeiot.com/contact \
  -H 'Content-Type: application/json' \
  -d '{"name":"Dana Okafor","email":"dana@example.com","company":"Meterworks",
       "fleet_size":"250_to_1500","about":"fleet","elapsed_ms":9000,
       "message":"We have about 900 water meters and need OTA updates."}'
```

(The example carries no `turnstile_token`, so against production — where the secret is set —
it answers `403`; it works verbatim against a dovecote with no secret configured.)

Returns `202` with an empty JSON object once the enquiry is **stored**. Unlike
`POST /feedback`, this route persists before it notifies: the row in `contact_submissions`
(`infra/migrations/2026-08-24-contact-submissions.sql`) is what keeps an enquiry from being
lost to a mail-transport outage, so a storage failure is a real `500`. The notification email
is then best-effort through the same `OPS_ALERT_EMAIL` + Resend transport every other ops mail
uses (`helpers/contact.rs`), stamping `notified_at` only once a send succeeds — `OPS_ALERT_EMAIL`
is set in production's `[vars]` block only, so staging and dev store the row and log the
formatted email instead of sending it.

Validation is `capsules::contact::validate`, called by both this route and the form that
produced the request, so the two cannot disagree about what a valid enquiry is.

Rejections:

- `400` if `Content-Type` is not `application/json`, the JSON is invalid, `name`/`email` are
  empty, `email` fails the shape check, `message` is shorter than
  `capsules::MIN_CONTACT_MESSAGE_BYTES`, `about` is not a plain slug, any field exceeds its
  length cap, or `elapsed_ms` is missing or under the fill-time floor. The response body is the
  user-facing sentence the form renders verbatim (`ContactRejection::message`).
- `403` if `TURNSTILE_SECRET` is set and the body carries no `turnstile_token`, or Cloudflare
  does not vouch for the one it carries (already spent, expired, minted for another site key).
  Never `401`. The form resets its widget and asks for one more click.
- `413` if the raw body exceeds `capsules::MAX_CONTACT_BODY_BYTES` or `message` exceeds
  `capsules::MAX_CONTACT_MESSAGE_BYTES`.
- `429` if the per-IP limiter (5 / 60s) is over its window. Deliberately never `401`, which the
  dashboard treats as a sign-out signal.
- `500` if the enquiry could not be stored.
- `503` if Cloudflare's siteverify could not be asked — unreachable, non-2xx, unparseable, or
  slower than the route's 5s deadline — or if it rejected our own secret
  (`missing-input-secret` / `invalid-input-secret`). Neither says anything about the sender,
  so neither is a `403`.

**Turnstile.** The widget (`fancier/src/views/contact.rs`) is rendered explicitly, after
hydration, into an empty container, so the prerendered page carries no third-party script and
a visitor without a token cannot submit. Its site key is the compile-time `TURNSTILE_SITE_KEY`
constant (`fancier/src/config.rs`, sourced from `.env.dev` / `.env.staging` / `.env.release`,
one widget per hostname). The server half is `helpers/turnstile.rs`, keyed by the
`TURNSTILE_SECRET` Worker secret on `dovecote` and on `dovecote --env staging`; local dev reads
it from `dovecote/.dev.vars`, where Cloudflare's published always-pass test pair
(`1x00000000000000000000AA` / `1x0000000000000000000000000000000AA`) is the intended value.
**A missing secret fails open**: the route logs once per isolate and accepts submissions
unverified, because a form that refuses every visitor over an unset secret loses real
enquiries to a configuration nobody is watching, and the limiter, honeypot and fill-time floor
still stand. Everything else fails closed: with a secret present, the only way past is a token
Cloudflare vouches for. The check runs after `capsules::contact::validate`, not before: a
token is single-use, so a field-error `400` must not spend it, and the honeypot's silent `202`
stays silent rather than becoming a `403` that names the control that fired.

### Error reporting

#### `POST /errors`

**Auth:** no auth required (identity only on the manual JSON path)

Ingest for the dashboard's automatic crash reports (Rust panic hook, pre-boot JS shim) and
for the crash screen's manual "tell us what happened" note. Rate-limited per IP (20/60s,
answering `429` — deliberately never `401`, which the dashboard treats as a sign-out signal).
Returns `202` with an empty JSON object; nothing about the response is actionable to the
sender (`sendBeacon` cannot read it).

The `Content-Type` carries the identity policy, not just the parse mode:

- **`text/plain`** (the automatic path — what `navigator.sendBeacon` sends, since it cannot
  negotiate a CORS preflight): body is a JSON-encoded `capsules::ErrorReport`. **Always
  anonymous**: this branch never resolves a session, and the envelope rejects unknown fields,
  so a body claiming an identity (a note, a user id, contact details) fails with `400` rather
  than being partially honored. `text/plain` is CORS-safelisted — any origin can POST it
  credentialed without a preflight — which is exactly why this branch is structurally
  cookie-blind.
- **`application/json`** (the manual path — the crash screen's note): body is
  `capsules::ErrorNoteRequest` (`{"note": "...", "report": {...ErrorReport...}}`). Not
  CORS-safelisted, so the preflight gates it to `ROOT_URL`, which is what makes an identified
  report unforgeable cross-origin. A present Kratos session is resolved server-side and stored
  as `user_id` on the event row (never trusted from the body); no session just stores the note
  anonymously.

```sh
curl -s -X POST https://api.pidgeiot.com/errors \
  -H 'Content-Type: text/plain;charset=UTF-8' \
  --data '{"kind":"rust_panic","message":"called Option::unwrap() on a None value","location":"src/views/pigeon.rs:412:18","route":"/flocks/…/pigeons/…","build":"dxh7a1e5a63c0523eb1","user_agent":"…","breadcrumbs":[{"age_ms":1200,"kind":"api","detail":"GET /flocks -> 500"}],"session_kind":"anonymous","occurred_at_ms":1755600000000,"client_event_id":"018f3b9e-…"}'
```

Server-side handling (all client fields are treated as hostile):

- The message and route are **re-normalized server-side** — UUIDs, long hex/base64 runs,
  emails, and bare integers replaced with placeholders; query strings dropped — and the
  normalized message is what the indefinitely-retained `error_groups` exemplar stores. The
  raw (byte-capped) message lives only on the 90-day `error_events` row.
- The grouping signature is computed server-side only (truncated SHA-256 over
  kind + normalized message + location); there is no client signature field.
- **Whose code threw decides the kind.** A report whose `location` (or, without one, top
  stack frame) is on an origin other than `ROOT_URL` — the analytics beacon, a browser
  extension — is folded into kind `third_party` with that origin as its location, so it makes
  one group per origin rather than one per minified column. `wasm:` frames, blobs minted by
  our origin, and reports carrying no URL at all count as ours, so a failure in our own JS
  glue is never dropped for want of a filename (`capsules::error_source`).
- **One message pattern also decides the kind**, because it arrives with nothing else to
  decide on. Microsoft's link-scanning crawler (Outlook / Defender for Office 365 Safe Links)
  opens linked pages in an instrumented browser whose injected bridge object is gone by the
  time the page calls back into it, and the rejection that follows —
  `Object Not Found Matching Id:<n>, MethodName:<name>, ParamCount:<n>`, all three fields in
  that order — carries no location and no stack. It folds to `third_party`
  (`capsules::is_link_scanner_noise`). The dashboard's pre-boot shim drops it before sending,
  so this fold is what covers clients still running an older build.
- `third_party` and `unsupported_browser` — the shim's kind for a browser that fails the
  pre-boot wasm capability probe, with the missing features named in the message — are stored
  and counted like any other kind but never mail (`ErrorKind::notifies`).
- `build` must match the release artifact's `dxh` + unpadded-u64-hex shape or it is blanked.
- `occurred_at` is clamped to ±24h of server time; retention keys on `received_at`.
- `client_event_id` is a client-minted correlation id (shown on the crash screen) that joins
  a manual note to the automatic crash it describes — a hint, not a key.
- A **new** signature of a notifying kind sends one ops email (`[ERROR] New: …`) to
  `OPS_ALERT_EMAIL`, under a global budget of 5/hour with the overflow folded into the next
  allowed email.

Rejections: `400` (unsupported `Content-Type`, invalid JSON, unknown fields on the text/plain
envelope, empty `note`), `413` (body or `note` over cap), `429` (rate limit).

#### `DELETE /errors`

**Auth:** session required

Erases every identified error-report row (`user_id` + `report_note`) belonging to the caller;
automatic reports never stored an identity, so there is nothing of theirs to erase there.
Returns `{"deleted": <count>}`. The manual account-deletion runbook runs the same statement
directly (documented in `infra/migrations/2026-08-19-error-reporting.sql`).

### Dashboard state

How a person has set their dashboard up — which telemetry graphs they saved against a pigeon,
and at what time range. Stored against the **Kratos identity**, never the organization: a saved
graph is how one person chose to look at a fleet, not a fact about the fleet, so two members of
one org keep their own.

A document is **opaque**. The platform stores the JSON body verbatim and never reads inside it,
which is what lets a new widget claim a key without a schema change. The only rules are the
ones below, and they are about size and naming, not shape.

`:scope_key` names what the document is about, and the dashboard mints it — today
`graphs.v1.pigeon.<pigeon_id>` and `graphs.v1.flock.<flock_id>` (`fancier`'s
`helpers::graph_store`). It must be 1–128 bytes of `[A-Za-z0-9._-]`
(`capsules::valid_scope_key`); anything else is a `400`, so a key can never escape its path
segment. There is no listing route: a client reads the scope it is rendering.

**The `GET` is deliberately exempt from Hyperdrive's ~60s query cache** — its statement carries
`now()`, which Hyperdrive refuses to cache. A browser with no local copy of a document has to
see a save made seconds ago, and without the exemption a page load's `404` seeds the cache and
the next profile is served that `404` after the document exists. The `PUT` still echoes the
stored entry, so a client never needs to re-read to confirm its own write. `updated_at` is the
server's clock and is what a client compares its own copy against.

#### `GET /dashboard-state/:scope_key`

**Auth:** session

Returns `capsules::DashboardStateEntry` — `{ scope_key, value, updated_at }`, where `value` is
the stored JSON as a string (same convention as the shadow's `target_config`). **404** when the
account has never saved this scope, which is a normal state and not an error.

#### `PUT /dashboard-state/:scope_key`

**Auth:** session

The request body **is** the document — no wrapper object. Replaces whatever was stored
wholesale and returns the stored `DashboardStateEntry`.

Rejections: `400` (invalid scope key, body that is not JSON), `413` (body over
`capsules::MAX_DASHBOARD_STATE_BYTES`, 16 KiB). An account may hold
`capsules::MAX_DASHBOARD_STATE_KEYS` (256) distinct keys; a **new** key past that is refused
`400`, while replacing a key that already exists never is, so an account at the cap can still
work with what it has.

#### `DELETE /dashboard-state/:scope_key`

**Auth:** session

Drops the document and frees its key against the cap. **204**, and deleting a scope that was
never stored is not an error. Account-deletion erasure removes every row for an identity
directly (`infra/migrations/2026-08-31-dashboard-state.sql`).

---

## Public Demo API

Three routes, all **read-only** and **unauthenticated** — no Kratos session, no device bearer
token, no `X-User-Id`, nothing. They back `fancier`'s public `/demo` page (a live, no-signup
preview of the platform) and exist for that page alone: no shadow, no logs, no listing route, no
write path of any kind is reachable here.

Authorization is a single allowlist check instead of a session/ACL/token check: the Worker var
`DEMO_PIGEON_IDS` (`wrangler.toml` — a comma-separated list of pigeon ids, one demo pigeon per
deployed environment) is matched exactly against the `:pigeon_id` path segment
(`helpers/demo.rs::is_demo_pigeon`). A `pigeon_id` not on that list — including a real,
currently-provisioned pigeon that just isn't the demo one — gets a plain **404**, not 403, so this
surface never confirms or denies whether an arbitrary id exists. `DEMO_PIGEON_IDS` is empty in
`dev`, so these routes 404 for every id there.

### Demo telemetry

#### `GET /demo/pigeons/:pigeon_id/telemetry`

**Auth:** none

Latest-value read — identical response shape to the dashboard's
[`GET /pigeons/:pigeon_id/telemetry`](#get-pigeonspigeon_idtelemetry) above (`Vec<capsules::
TelemetryLatest>`), reading the same Durable Object table, just without the `X-User-Id`/ACL
check (`objects/pigeons.rs::get_telemetry_latest_demo`).

```sh
curl -s https://api.pidgeiot.com/demo/pigeons/<demo_pigeon_id>/telemetry
```

#### `GET /demo/pigeons/:pigeon_id/telemetry/history`

**Auth:** none

History read — same query params and response shape (bucketed by default,
`Vec<capsules::TelemetryHistoryBucket>`; `raw=true` for the flat/capped
`Vec<capsules::TelemetryHistoryPoint>`, Greptime-first/Postgres-fallback) as the dashboard's
[`GET /pigeons/:pigeon_id/telemetry/history`](#get-pigeonspigeon_idtelemetryhistory) above, just
without the ACL probe.

```sh
curl -s "https://api.pidgeiot.com/demo/pigeons/<demo_pigeon_id>/telemetry/history?key=temp_c"
```

### Demo alerts

#### `GET /demo/pigeons/:pigeon_id/alerts`

**Auth:** none

The alerts the platform is really enforcing on the demo pigeon, so the demo page can draw a
threshold line from the rule itself rather than from a number written into the page.

Unlike the two routes above, this one does **not** share the dashboard's response shape. The
dashboard returns `capsules::AlertDefinition`, which carries `user_id` (the owner's account UUID)
and `channel` (an `AlertChannel::Email` holding a real recipient address) — neither of which may
appear on a route that answers anyone. This route returns `Vec<capsules::DemoAlert>`, a separate
type holding only the six fields below, and `helpers/alerts.rs::list_demo_pigeon_alerts` never
selects the excluded columns from Postgres in the first place.

| Field | Type | Notes |
|---|---|---|
| `name` | string | The alert's own name |
| `severity` | `"Warning"` \| `"Critical"` | |
| `status` | `"Ok"` \| `"Firing"` | Current state for this pigeon; `Ok` if never evaluated |
| `key` | string \| null | Telemetry key the threshold watches |
| `comparator` | `"Gt"` \| `"Gte"` \| `"Lt"` \| `"Lte"` \| `"Eq"` \| null | |
| `value` | number \| null | The threshold itself |

`key`, `comparator` and `value` are non-null only for an `AlertCondition::Threshold` — the one
condition with a number a chart can draw a line at. Other conditions are still listed, with all
three null, so the page can show that an alert exists without inventing a line for it.

Only **pigeon-scoped, enabled** definitions appear. A flock-scoped alert is shared configuration
that also governs pigeons which are not on the demo allowlist, and a disabled definition would
draw a threshold nothing is actually checking.

```sh
curl -s https://api.pidgeiot.com/demo/pigeons/<demo_pigeon_id>/alerts
```

```json
[
  {
    "name": "Greenhouse too warm",
    "severity": "Warning",
    "status": "Ok",
    "key": "temp_c",
    "comparator": "Gt",
    "value": 30.0
  }
]
```

---

## Discovery

### API catalog

#### `GET|HEAD /.well-known/api-catalog`

**Auth:** no auth required

An [RFC 9727](https://www.rfc-editor.org/info/rfc9727) API catalog: a machine-readable
linkset describing this API host, for agents and crawlers doing capability discovery
(Cloudflare's Agent Readiness checklist probes this exact path). Pure metadata — it links to
the public documentation and carries no data about any account, flock, or pigeon.

Response `Content-Type` is `application/linkset+json; profile="https://www.rfc-editor.org/info/rfc9727"`.
The API origins in the body are derived from the request URL (so staging/dev describe
themselves); the documentation links point at the frontend origin (`ROOT_URL`).

```sh
curl -s https://api.pidgeiot.com/.well-known/api-catalog
```

```json
{
  "linkset": [
    {
      "anchor": "https://api.pidgeiot.com/.well-known/api-catalog",
      "item": [{ "href": "https://api.pidgeiot.com/" }]
    },
    {
      "anchor": "https://api.pidgeiot.com/",
      "service-doc": [
        { "href": "https://pidgeiot.com/api-reference/", "type": "text/html" },
        { "href": "https://pidgeiot.com/api-reference/index.md", "type": "text/markdown" }
      ],
      "service-meta": [
        { "href": "https://pidgeiot.com/auth.md", "type": "text/markdown" },
        { "href": "https://pidgeiot.com/llms.txt", "type": "text/plain" }
      ]
    }
  ]
}
```

A copy of this catalog (anchored at the frontend origin) is also served statically at
`https://pidgeiot.com/.well-known/api-catalog` by `fancier` (`fancier/public/.well-known/`).

---

## Device API

Every route below is under `/device/pigeons/:pigeon_id/*` and authenticates via
`Authorization: Bearer <device_token>` — see [Device authentication](#device-authentication-bearer-token). None of these accept or check a Kratos session.

### Device shadow

#### `GET /device/pigeons/:pigeon_id/shadow`

**Auth:** device token

Reads the current shadow — same shape as the dashboard's `GET /pigeons/:pigeon_id/shadow`
above (same `JsonString`-wrapped-fields caveat applies).

```sh
curl -s https://api.pidgeiot.com/device/pigeons/<pigeon_id>/shadow \
  -H 'Authorization: Bearer <device_token>'
```

#### `POST /device/pigeons/:pigeon_id/shadow`

**Auth:** device token

Device report-back: confirms `target_config` was applied. Body:
`capsules::PigeonShadowReportRequest` — `{ current_config: <JSON object>, current_version: <int> }`.
`current_version` should be the `target_version` the device read in its last shadow `GET`, echoed
back — it's stored as-is, not re-derived, since a newer target may already be waiting by the
time this lands. Returns the updated shadow (same shape as the `GET` above).

```sh
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/shadow \
  -H 'Authorization: Bearer <device_token>' \
  -H 'Content-Type: application/json' \
  -d '{"current_config":{"telemetry_interval":60},"current_version":1}'
```

This also best-effort syncs the reported shadow into dovecote's Postgres mirror on the gateway
side, so `fancier` doesn't need to poll the Durable Object directly to see a device's latest
reported state.

An accepted report-back counts as one billable device message against the owning account's
message allowance, the same as a telemetry report, and is refused with the same `429` by the
free-tier allowance fuse (see the telemetry route below) once that allowance is spent. The
check runs inside the Durable Object, after the bearer token is verified and before the report
is stored, so a refused report is neither stored nor counted.

### Device telemetry

#### `POST /device/pigeons/:pigeon_id/telemetry`

**Auth:** device token

Reports telemetry, in either of two body shapes.

**Flat** — a JSON object of string key/value pairs, one reading taken now. No nesting, no typed
values; this matches the wire shape the `pigeon` Zephyr device library's
`pigeon_set_shadow_param()`/`pigeon_shadow_flush()` calls produce. `400` if the body is empty
or not a flat string map, or if it breaks one of the [telemetry caps](#telemetry) (more than 128
keys, a key over 128 bytes, a value over 1024 bytes) — an over-cap report is refused whole, with
none of its keys applied. The report merges into the pigeon's stored keys rather than replacing
them; see [Telemetry](#telemetry) above for what that means for per-key timestamps.

**Batched** — `{"reports": [ ... ]}`, where each entry is `{"metrics": {...}}` plus at most one
timestamp field: `age_secs` (seconds before the batch was sent — the form a device with no wall
clock can fill in) or `at` (absolute unix seconds). A reading with neither is treated as taken
now. This lets a device accumulate readings locally and deliver a window of them in one request;
see [Telemetry](#telemetry) above for the clamping rules, the ordering guarantees, and what it
saves.

The two are told apart by shape, not by a version field or a query param: a telemetry value is
always a string, so a body whose `reports` field holds an *array* cannot be a flat report, and a
device that happens to use `reports` as a telemetry key sends a string there and is read as flat.
Nothing about the flat form changed.

Caps specific to the batch form, each refusing the batch whole: more than 64 readings
(`capsules::MAX_TELEMETRY_BATCH_READINGS`, `400`), more than 128 distinct keys across the whole
batch (`400` — the union, so each reading being individually within the key cap is not enough),
a reading with no metrics (`400`), or a raw body over 16 KiB
(`capsules::MAX_TELEMETRY_BATCH_BYTES`, `413`). Every per-reading cap from the flat form applies
to every reading.

A batch counts as **one** request against the route's rate limiting and the free-tier fuse, and
as **M** billable messages against the account's message allowance — see
[Telemetry](#telemetry) for why those two differ.

**Free-tier allowance fuse.** On a free-tier account that has exhausted its monthly pooled
message allowance, this route answers `429 Too Many Requests` (after the bearer token has been
verified) for the rest of the billing period — the `pigeon` device library backs off and keeps
unsent readings queued, so data is delayed rather than lost. Paid, entitled tiers are never
paused; their over-allowance usage bills as metered overage instead. The check fails open: a
usage-lookup failure never blocks ingestion. Every ingest surface answers to the same fuse:
shadow report-backs and log uploads `429` alongside this route, and the WebSocket endpoint
refuses the upgrade with the same `429` and closes an already-open socket on its next billable
frame (code `4029`).

```sh
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/telemetry \
  -H 'Authorization: Bearer <device_token>' \
  -H 'Content-Type: application/json' \
  -d '{"temp":"21.5","status":"ok"}'
```

```sh
# Batched: six readings taken 10s apart, delivered together. The device
# never needs to know the time -- only how long ago it took each reading.
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/telemetry \
  -H 'Authorization: Bearer <device_token>' \
  -H 'Content-Type: application/json' \
  -d '{"reports":[
        {"age_secs":50,"metrics":{"temp":"21.1"}},
        {"age_secs":40,"metrics":{"temp":"21.2"}},
        {"age_secs":30,"metrics":{"temp":"21.3"}},
        {"age_secs":20,"metrics":{"temp":"21.4"}},
        {"age_secs":10,"metrics":{"temp":"21.5"}},
        {"age_secs":0,"metrics":{"temp":"21.6"}}
      ]}'
```

**Response behavior differs by environment.** In an environment with a telemetry queue bound
(staging and production today — `TELEMETRY_QUEUE` in `wrangler.toml`), the gateway synchronously
verifies the bearer token against the Durable Object, then enqueues the report and returns
immediately:

```
202 Accepted
{}
```

The actual write (the Durable Object's latest-value upsert, plus history — the platform's
Postgres store (or GreptimeDB where configured), or an external line-protocol forward if this
pigeon has its own `telemetry_endpoint` configured; see [task
#26](#get-pigeonspigeon_idtelemetryhistory) above) happens asynchronously afterward — a `202`
confirms the report was authenticated and queued, not that it's been persisted yet. In an
environment with no queue bound (dev only), the same auth + write happens
synchronously in one round trip and returns:

```
200 OK
{"temp":"21.5","status":"ok"}
```

(the metrics you just sent, echoed back — for a batch, the merged newest-value-per-key union of
every reading in it).

### Device logs

#### `POST /device/pigeons/:pigeon_id/logs`

**Auth:** device token

Ingests one binary log chunk — the request body **is** the chunk, sent as raw bytes (not
wrapped in JSON, no base64 encoding needed on the way in — that only happens on the read side,
`GET /pigeons/:pigeon_id/logs`). Intended for Zephyr's `CONFIG_LOG_DICTIONARY_SUPPORT`
token-compressed log records, but dovecote never inspects the contents — it's opaque storage,
decoded host-side against the firmware's own dictionary/ELF.

- `400` if the body is empty.
- `413 Payload Too Large` if the body exceeds 16 KiB (`capsules::MAX_LOG_CHUNK_BYTES`).
- `200` with an empty body on success.

An accepted chunk counts as one billable device message against the owning account's message
allowance, the same as a telemetry report, and is refused with the same `429` by the free-tier
allowance fuse once that allowance is spent, checked after the bearer token is verified and
before the chunk is stored.

```sh
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/logs \
  -H 'Authorization: Bearer <device_token>' \
  --data-binary @log-chunk.bin
```

### Device firmware

#### `GET /device/pigeons/:pigeon_id/firmware`

**Auth:** device token

Downloads the firmware image currently assigned to **this pigeon's own shadow**
(`target_config.firmware` — see [Shadow](#shadow) above). There's no version/sha256 path
parameter; the route always serves whatever this pigeon is currently targeted at. Supports
standard HTTP `Range` requests (`bytes=<start>-<end>`, `bytes=<start>-` for open-ended, or
`bytes=-<suffix>`) — R2-backed, so ranged reads are efficient server-side; a single-range request
only (no multi-range). This is required, not optional, for constrained devices: the nRF9160
writes chunks straight to the MCUboot secondary flash slot rather than buffering the whole image
in its ~256 KB of RAM.

- `401` for a missing/invalid/expired bearer token.
- `404` if this pigeon's shadow currently has no `firmware` key set, or the assigned image is
  somehow missing from R2.
- `200` (whole image) or `206 Partial Content` (a `Range` was honored).

Response headers: `Content-Length`, `Accept-Ranges: bytes`, `Content-Range` (on a `206`), `ETag`,
and `X-Firmware-Sha256`/`X-Firmware-Version`/`X-Firmware-Size` mirroring the assigned
`FirmwareTarget`, so the device can verify total size + hash without re-parsing the shadow
document.

```sh
# Whole image
curl -s https://api.pidgeiot.com/device/pigeons/<pigeon_id>/firmware \
  -H 'Authorization: Bearer <device_token>' \
  -o firmware.bin

# One chunk, as the nRF9160 would request it
curl -s https://api.pidgeiot.com/device/pigeons/<pigeon_id>/firmware \
  -H 'Authorization: Bearer <device_token>' \
  -H 'Range: bytes=0-4095' \
  -o chunk0.bin
```

### Device WebSocket

#### `GET /device/pigeons/:pigeon_id/ws`

**Auth:** device token

Upgrades to a persistent WebSocket — the real-time channel for non-cellular (WiFi/mains-powered)
devices (task #32), replacing the poll (`GET .../shadow`) + report (`POST .../shadow`,
`POST .../telemetry`) pattern above with one long-lived connection. Cellular/constrained devices
can keep using the HTTP routes above unchanged; the two are independent, not a migration.

**Handshake.** Standard WebSocket upgrade — a `GET` request carrying `Upgrade: websocket` (and
the usual `Connection`/`Sec-WebSocket-*` handshake headers) — with the device's bearer token on
`Authorization: Bearer <device_token>`, same as every other device route. The gateway rejects
anything without `Upgrade: websocket` with a plain `400` before it ever reaches a Durable Object.
The owning Durable Object then verifies the bearer token **before** accepting the socket
(`is_authorized_device`, same check every other device route uses) — an invalid/expired/wrong-
pigeon token gets a normal `401 Unauthorized` HTTP response instead of a `101 Switching
Protocols` upgrade. A client library that only understands the standard `WebSocket` constructor
(no custom headers, e.g. a browser) cannot open this connection; use a library/runtime that lets
you set `Authorization` on the handshake request (Node's `ws` package with a `headers` option,
Zephyr's own WebSocket client, etc).

```sh
# using websocat, or any WS client that can set a header on the handshake
websocat -H 'Authorization: Bearer <device_token>' \
  wss://api.pidgeiot.com/device/pigeons/<pigeon_id>/ws
```

**One socket per pigeon.** A new connection replaces any existing one for the same pigeon rather
than coexisting with it — the old socket is closed (code `4009`, reason "replaced by new
connection") as part of accepting the new one. Useful for a device that reconnects after a
network blip before its old socket has timed out.

**Token refresh and pigeon deletion close the open socket.** Bearer auth on this endpoint is
checked once, at accept, not per frame — so a socket opened before `POST
/pigeons/:pigeon_id/token/refresh` or `DELETE /pigeons/:pigeon_id` would otherwise keep running on
a credential (or a pigeon) that no longer exists, until it happened to drop on its own. Both
routes close any open device socket for the pigeon as part of the same request: a refresh closes
it with code `4004`, reason "token revoked"; a deletion closes it with code `4005`, reason
"pigeon deleted". Either way the device's WS client reconnects on any close and re-authenticates
with whatever token it currently holds — a refresh is what makes that reconnect attempt actually
fail once it retries with the now-superseded token, and a deletion's reconnect fails the same way
since `is_authorized_device` finds no `pigeons` row left to check against.

**Shadow snapshot on connect.** Immediately after the socket is accepted, the server pushes one
`shadow_update` frame (same shape as every other `shadow_update` — see the frame table below)
carrying this pigeon's current shadow, so a freshly (re)connected device doesn't need a separate
`GET .../shadow` to catch up on a `target_config` it missed while disconnected. This is
best-effort — a device should still be able to fall back to `GET .../shadow` on connect if it
wants to be defensive, but in practice it's redundant once this frame arrives.

**Server implementation note (not a wire-protocol detail, but relevant if you're touching this
code):** accepted via the Durable Object *hibernation* WebSocket API
(`State::accept_websocket_with_tags`, `worker` crate v0.8+), not the in-memory
`WebSocket::accept()` — an idle connection can be evicted from the Durable Object's memory
between messages without being torn down, keeping a fleet of mostly-idle long-lived connections
cheap. This is transparent to the device; reconnection is never required just because nothing
was sent for a while.

**Frame protocol.** JSON text frames only — binary frames get the connection closed (code
`4001`). Every frame is a JSON object with a `type` field:

| Direction | `type` | Fields | Effect |
|---|---|---|---|
| device → server | `telemetry` | `metrics: {string: string}` **or** `reports: [{metrics, age_secs \| at}]` | Same handling as `POST /device/pigeons/:id/telemetry`, in both of that route's body shapes: an immediate merge into the pigeon's own Durable Object latest-value store, plus (environment-dependent — see below) a queued write for history/forwarding. The two fields are alternatives, not a pair — `metrics` is one reading taken now, `reports` a batch of timestamped ones, and the same clamping rules apply as on the HTTP route. Sending only `metrics` is what shipped firmware does and is unchanged. The same [telemetry caps](#telemetry) apply, plus the batch caps when `reports` is used, but a frame has no reply of its own to carry a `400`: an over-cap frame is logged server-side and dropped whole, and the socket stays open. A batched frame is **one** frame against the socket's frame-rate limit and **M** billable messages. |
| device → server | `shadow_report` | `current_version: int`, `current_config: <JSON object>` | Same handling as `POST /device/pigeons/:id/shadow`: updates `pigeon_shadow.current_config`/`current_version` and best-effort syncs the result to Postgres. |
| device → server | `ping` | — | Server replies with `{"type":"pong"}`. |
| device → server | `pong` | — | Liveness acknowledgement only; no reply, no other effect. |
| device → server | `shell_output` | `request_id: string`, `output: string`, `exit_code: int`, `truncated: bool` | Reply to a server-sent `shell_cmd` (task #34, v1). Resolves the matching `POST /pigeons/:pigeon_id/shell` request by `request_id`; a reply with no matching in-flight request (already timed out, or a stray/duplicate) is logged server-side and dropped, not treated as a protocol error. `truncated` is set if the command's output exceeded the device's local output buffer — the honest signal that `output` might be incomplete, since the device's shell backend silently drops overflow bytes with no other indication. |
| server → device | `shadow_update` | `shadow: <capsules::PigeonShadow, same shape as the GET .../shadow responses above>` | Pushed **immediately** whenever this pigeon's `target_config` changes via a dashboard `PUT /pigeons/:id/shadow` (including a firmware assignment, which reuses that same route) — this is the headline reason this endpoint exists: no more waiting for the device's next poll to learn about a new target. Also pushed once, unprompted, right after the socket is accepted (see "Shadow snapshot on connect" above), so a reconnecting device is caught up before it ever sends a frame of its own. |
| server → device | `pong` | — | Reply to a device-sent `ping`. |
| server → device | `shell_cmd` | `request_id: string`, `cmd: string` | Sent by `POST /pigeons/:pigeon_id/shell` (task #34, v1, owner-gated — see [Pigeons](#pigeons) above) to run one command on the device and relay its output back over plain HTTP. `request_id` is a correlation token, not a security boundary — the auth gate is the owner-only check before this frame is ever sent. Devices without shell support compiled in (`CONFIG_PIGEON_SHELL`, off by default) silently ignore this frame type via the existing forward-compat unknown-`type` fallthrough, so older/non-participating firmware in the field is unaffected. |

```json
// device -> server
{"type":"telemetry","metrics":{"temp":"21.5","status":"ok"}}
{"type":"telemetry","reports":[{"age_secs":10,"metrics":{"temp":"21.5"}},{"age_secs":0,"metrics":{"temp":"21.6"}}]}
{"type":"shadow_report","current_version":1,"current_config":{"telemetry_interval":60}}
{"type":"ping"}
{"type":"shell_output","request_id":"01991a2b-...","output":"target_version=2 current_version=1\n","exit_code":0,"truncated":false}
```

```json
// server -> device, pushed the moment a dashboard PUT lands
{"type":"shadow_update","shadow":{"target_version":2,"current_version":1,"target_config":"{\"telemetry_interval\":30}","current_config":"{\"telemetry_interval\":60}","updated_at":1784390937}}
```

```json
// server -> device, sent by POST /pigeons/:pigeon_id/shell
{"type":"shell_cmd","request_id":"01991a2b-...","cmd":"pigeon shadow"}
```

Note `shadow.target_config`/`current_config` in a `shadow_update` push are `capsules::JsonString`
— a JSON string containing JSON text, same asymmetry as the HTTP shadow routes' response bodies
(see [Shadow](#shadow) above) — not nested objects.

**Limits, enforced by the owning Durable Object:**

| Limit | Value | Behavior over the limit |
|---|---|---|
| Max frame size | 16 KiB | Connection closed, code `4002`, reason "frame too large" |
| Frame rate | 50 frames / rolling 10s window, per socket | Connection closed, code `4008`, reason "rate limit exceeded" |
| Malformed frame (not valid JSON, or missing/unknown `type`) | — | Connection closed, code `4003`, reason "malformed frame"; logged server-side |
| Free-tier message allowance spent, on a `telemetry` or `shadow_report` frame | Monthly pooled allowance | Connection closed, code `4029`, reason "free tier message allowance exhausted" |

None of the first three are "recoverable" mid-connection: reconnect (a fresh `GET .../ws`) to
resume after any of them.

`4029` is the WebSocket spelling of the `429` the HTTP ingest routes answer with, and it is the
one close in this table that reconnecting does **not** clear: the upgrade itself is refused with
`429` until the allowance resets or the account moves to a paid tier. A device should back off
as it does on an HTTP `429` rather than reconnect immediately. `ping`/`pong` and `shell_output`
frames are not billable and are still served on a paused account; only the billable frames
close the socket.

**`shell_cmd`/`shell_output` count toward the same 50-frame/10s budget above — no carve-out in
v1.** One command invocation is one frame each way, negligible against that budget; see the
task #34 design doc if a future interactive/streaming shell ever needs a dedicated rate-limit
class of its own.

**`TELEMETRY_QUEUE` and the `telemetry` frame — same environment-dependent behavior as the HTTP
route** (see [`POST /device/pigeons/:pigeon_id/telemetry`](#post-devicepigeonspigeon_idtelemetry)
above). Where a telemetry queue is bound (staging and production today), a `telemetry` frame's
metrics are upserted into the Durable Object's latest-value table synchronously, then enqueued
for the same consumer path the HTTP route uses (the platform's Postgres history store — or
GreptimeDB where configured — or an external line-protocol forward if `telemetry_endpoint` is
configured — task #26) — but since the frame already arrived on an authenticated connection,
there's no
separate verify-before-enqueue round trip the way the HTTP route needs (auth happened once, at
socket accept). Where no queue is bound (dev), the Durable Object writes the same
platform history store directly instead, so telemetry sent over the socket doesn't
silently skip history in that environment.

**No response is sent for `telemetry`/`shadow_report` frames themselves** — there's no
frame-level ack. Read back the result via the ordinary HTTP routes (`GET
/pigeons/:pigeon_id/telemetry`, `GET /pigeons/:pigeon_id/shadow`) if you need confirmation, or
rely on the `shadow_update` push for the shadow side.

---

## CoAP device surface (via the `loft` terminator)

The five HTTP device routes above are also reachable over CoAP, terminated by `loft` (a
first-party Rust service in its own repo, `github.com/justins-engineering/loft`) at
`coap.pidgeiot.com:5684` — **not** by the edge Worker. Two transports, same port, same resources:

| Transport | Scheme | Notes |
|---|---|---|
| DTLS 1.2 / UDP (RFC 7252) | `coaps://` | The **primary** device transport — cheapest secure wake-and-send for PSM'd cellular devices. CON and NON both supported; piggybacked ACK responses; duplicate CONs get the original response replayed, not re-executed. |
| TLS 1.2 / TCP (RFC 8323) | `coaps+tcp://` | For device builds compiled with the TCP transport — same authority as the minted `coaps://` endpoint, scheme substituted. The terminator sends its CSM (7.01) after the handshake and answers Ping (7.02) with Pong (7.03), but tolerates minimal clients that never send a CSM of their own. |

### PSK authentication

**Authentication is the PSK handshake itself.** Both listeners accept only PSK ciphersuites
(`TLS_PSK_WITH_AES_128_CCM_8` preferred, GCM/CBC-SHA256 fallbacks; TLS 1.2 — no certificates
anywhere). The PSK identity is the pigeon's id, and the PSK key is the raw UTF-8 bytes of
`connector.Coap.tls_psk_secret` — a 32-char hex string minted alongside the bearer token,
deliberately NOT the token itself: RFC 4279 only obliges TLS stacks to support PSKs up to 64
bytes (mbedTLS defaults to 32, libcoap's client caps at 64), so the 92-char token can't serve
as a PSK on the constrained stacks CoAP targets. `loft` resolves identity → (PSK, token) at
handshake time through [`GET /internal/coap-psk/:pigeon_id`](#service-internal-api) (below),
then proxies each CoAP request to the matching HTTP device route with
`Authorization: Bearer <token>` — so the pigeon's own Durable Object still cryptographically
verifies every request, exactly as for direct HTTPS devices, and a `token/refresh` rotates the
PSK and the token together, revoking CoAP access and HTTPS access at once.

**The Uri-Path pigeon id must equal the handshake identity.** A request whose path names any
other pigeon gets 4.03 Forbidden without ever reaching dovecote, regardless of the path's
validity. The device bearer token may additionally appear in a `Uri-Query` option
(`auth=<token>`, the `~/pigeon` client's current shape) — it's ignored; the
handshake-authenticated secret is what's forwarded upstream.

### Resource map

**Resource map** (Uri-Path mirrors the HTTP paths 1:1):

| CoAP request | HTTP route behind it | Response |
|---|---|---|
| `GET device/pigeons/:id/shadow` | `GET /device/pigeons/:id/shadow` | 2.05 Content, JSON payload |
| `POST device/pigeons/:id/shadow` | `POST /device/pigeons/:id/shadow` | 2.04 Changed, JSON payload (updated shadow) |
| `POST device/pigeons/:id/telemetry` | `POST /device/pigeons/:id/telemetry` | 2.04 Changed |
| `POST device/pigeons/:id/logs` | `POST /device/pigeons/:id/logs` | 2.04 Changed, empty payload |
| `GET device/pigeons/:id/firmware` | `GET /device/pigeons/:id/firmware` | 2.05 Content, always Block2 (below) |

Status mapping for errors: HTTP 400/401/403/404/405/413 → CoAP 4.00/4.01/4.03/4.04/4.05/4.13;
upstream 5xx → 5.02 Bad Gateway; dovecote unreachable → 5.04 Gateway Timeout. Error payloads
carry the upstream diagnostic text (capped).

### Block-wise transfer

**Block-wise transfer (RFC 7959).** Firmware downloads are always served Block2-wise (1024-byte
blocks max, szx ≤ 6; BERT szx 7 is down-negotiated to 6): each Block2 request maps directly to
an HTTP `Range` request against dovecote — block N = `bytes=N*size-(N*size+size-1)` — so the
image never transits the terminator as a whole. Block 0's response carries `Size2` (total image
bytes) and an `ETag` (first 8 bytes of the image's sha256) for mid-transfer change detection. A
firmware GET without a Block2 option gets block 0 with the more-bit set (spontaneous Block2).
Large JSON responses are spontaneously Block2'd over UDP only (>1024 bytes; TCP frames are sent
whole, matching the minimal `~/pigeon` client). POST bodies may be sent Block1-wise (2.31
Continue per intermediate block; 64 KiB reassembly cap; 4.08 on a broken sequence).

### Client examples

```sh
# libcoap client, DTLS PSK over UDP — note -k takes the RAW secret string
coap-client -m get -u <pigeon_id> -k '<tls_psk_secret>' \
  "coaps://coap.pidgeiot.com/device/pigeons/<pigeon_id>/shadow"

# Same resource over TLS/TCP (RFC 8323)
coap-client -m get -u <pigeon_id> -k '<tls_psk_secret>' \
  "coaps+tcp://coap.pidgeiot.com/device/pigeons/<pigeon_id>/shadow"

# Telemetry report
coap-client -m post -u <pigeon_id> -k '<tls_psk_secret>' \
  -t application/json -e '{"temp":"21.5"}' \
  "coaps://coap.pidgeiot.com/device/pigeons/<pigeon_id>/telemetry"
```

### Connection ID

**Connection ID (RFC 9146) is supported.** The DTLS listener runs mbedTLS with CID enabled, so
a PSM/NAT'd cellular device whose NAT mapping dies during sleep can keep its DTLS association
across an address/port rebind instead of paying a fresh handshake (~2 RTT with these PSK
suites) on every wake. Devices that offer no CID negotiate a plain session and work unchanged.
The `loft` repo's `docs/infra/coap-terminator.md` documents the deployment posture.

---

## MQTT device surface (via the `pigeonhole` broker)

The same device routes are reachable over MQTT, terminated by `pigeonhole` (a first-party Rust
service in its own repo) at `mqtt.pidgeiot.com:8883` — **not** by the edge Worker, which cannot
hold a TCP session. The broker is a thin bridge: it terminates TLS, framing, sessions and
keepalive, and every publish becomes one of the HTTP device routes above, carrying the pigeon's
own bearer token. It holds no per-pigeon state and stores nothing, so authorization is still the
owning Durable Object's, exactly as for a direct HTTPS device.

### Listener and handshakes

**One TLS listener, two handshakes, no cleartext.** There is no port 1883 and no unencrypted
listener in any deployment shape: the CONNECT password is a device token, and that rule is what
keeps it off the wire. The ClientHello decides which credential is used:

| Mode | Handshake | CONNECT |
|---|---|---|
| Certificate | The broker's own Let's Encrypt chain for `mqtt.pidgeiot.com`; the device verifies it | `username` = pigeon id, `password` = the device bearer token, `client_id` = the pigeon id or empty |
| PSK | `TLS_PSK_WITH_AES_128_CCM_8` / GCM / CBC-SHA256, the same suites `loft` offers | PSK identity = pigeon id, key = the raw UTF-8 bytes of `connector.Mqtt.tls_psk_secret`; `username`, if sent, must equal the identity and `password` is ignored |

In certificate mode the broker cannot verify an Ed25519 token itself, so it does not try: it
opens the pigeon's [device WebSocket](#get-devicepigeonspigeon_idws) with the presented token,
and that upgrade **is** the authentication — 101 accepts the session, 401 refuses it. In PSK
mode it resolves identity → (PSK, token) through
[`GET /internal/device-psk/:pigeon_id`](#service-internal-api) at handshake time, then opens the
same socket. Wherever an identity appears more than once (PSK identity, username, client id) all
of them must agree.

### Topics

**Topics are session-scoped and carry no pigeon id** — the handshake already bound the
connection to exactly one pigeon, so the id would be redundant weight on every publish. Payloads
are byte-identical to the HTTP bodies:

| Topic | Direction | Payload | Route behind it |
|---|---|---|---|
| `pigeon/telemetry` | device → | flat JSON object of string values | `POST /device/pigeons/:id/telemetry` (QoS 1), or a `telemetry` frame on the held device WebSocket (QoS 0) |
| `pigeon/shadow/report` | device → | `{"current_config": {...}, "current_version": N}` | `POST /device/pigeons/:id/shadow` |
| `pigeon/logs` | device → | one raw dictionary-log chunk, ≤ 16 KiB | `POST /device/pigeons/:id/logs` |
| `pigeon/shadow/target` | → device, retained | `capsules::PigeonShadow` as the device shadow GET returns it | the device WebSocket's snapshot-on-accept and every `shadow_update` frame |

The retained value on `pigeon/shadow/target` is the Durable Object's own live shadow, not a copy
the broker keeps: a subscriber gets the current one on SUBACK and a fresh PUBLISH whenever
`target_version` changes. Accepted filters are `pigeon/shadow/target`, `pigeon/shadow/#` and
`pigeon/#`, all meaning the shadow target; any other filter gets a SUBACK failure for that entry,
and publishing to an unknown topic closes the connection.

### Quality of service

**QoS 0 and 1 only.** A QoS 1 PUBACK is sent when the upstream route answers, so an ack means
dovecote accepted the report and nothing is buffered on the broker; QoS 0 is fire-and-forget over
the already-open WebSocket. QoS 2 is not offered rather than shimmed: true exactly-once needs a
per-client dedup store that survives reconnects, which a stateless bridge deliberately has not
got, and delivering it as at-least-once would silently break the contract the client asked for.
Sessions are stateless (`session_present` is always 0) — the retained shadow gives a reconnecting
device the catch-up a queued session would have. A Last Will is accepted when its topic is one
this session may publish to, and delivered as an ordinary bridged publish.

### Rotation and deletion

Rotation and deletion reach a live session, in both directions: a bridged publish that answers
401 ends the session, and `token/refresh` and `delete` close the pigeon's device WebSocket with
`4004` / `4005` themselves, which ends it even if the device never publishes again. Firmware has
no MQTT surface — the `firmware` key arrives inside the retained shadow and the device downloads
it over
[`GET /device/pigeons/:id/firmware`](#get-devicepigeonspigeon_idfirmware), which already does
ranged, resumable chunking.

### Broker endpoint

`MQTT_DEVICE_HOST` (`dovecote/wrangler.toml`) is what points minted endpoints at a broker;
where an environment leaves it empty the endpoint falls back to that environment's own API host,
so an `Mqtt` pigeon can be provisioned with real credentials before a broker exists to dial.

---

## Service-internal API

### PSK lookup

#### `GET /internal/device-psk/:pigeon_id`

**Auth:** service secret required

Also served at its original name, `GET /internal/coap-psk/:pigeon_id`. One handler, two paths,
identical in every respect: the neutral name is what a terminator that is not CoAP asks for, and
the older one stays because `loft` is deployed against it.

PSK resolution for the protocol terminators — the only route in this API authenticated by a
shared service secret rather than a Kratos session or device token:
`Authorization: Bearer <COAP_SERVICE_SECRET>` (a Worker secret, set per environment via
`wrangler secret put COAP_SERVICE_SECRET`, never a `[vars]` entry; `loft` holds the same value
in its own `COAP_SERVICE_SECRET` env var, and `pigeonhole` in `PIGEONHOLE_SERVICE_SECRET`; one
value, one gate, whatever each side calls it). The secret is compared in constant time, and is
only half the gate: the request's source address (`CF-Connecting-IP`, edge-set and not
client-forgeable) must also appear in the environment's `COAP_SERVICE_ALLOWED_IPS` allowlist
(`dovecote/wrangler.toml`) — the terminator's own egress addresses, empty meaning deny-all.
An environment where either layer is unconfigured refuses every call (fail closed). Not
CORS-usable from a browser in any meaningful way; never called by devices or the dashboard.

`:pigeon_id` is the PSK identity (identical to the pigeon's id). Response is
`capsules::CoapPskLookup`:

```json
{"identity": "<pigeon_id>", "secret": "<tls_psk_secret>", "token": "<device_bearer_token>"}
```

- `401` missing bearer, `403` source address outside the allowlist or wrong/unconfigured
  secret.
- `404` for an unknown identity or a pigeon whose connector mints no PSK (`Https`). Both
  PSK-bearing variants (`Coap`, `Mqtt`) resolve, under either route name: which transport minted
  the pair says nothing about which terminator may resolve it, and both hold the same secret.
- `400` for a string that cannot be a pigeon id at all (Durable Object ids embed a namespace
  check, so a malformed/foreign id fails before any lookup). `loft` treats `400` and `404`
  identically: authoritatively unknown, negative-cached.

**This is the one deliberate exception to the strip-on-read rule for connector secrets** — a
PSK terminator cannot complete a handshake without the key. Scope of what the secret's holder
gains: per-identity device credentials (each still verified per-request by the owning Durable
Object), no dashboard/org/flock access of any kind. `loft` caches positives for 60s — after a
`token/refresh`, the OLD PSK can therefore still complete a *handshake* for up to 60s, but
every request on such a session presents the revoked bearer token and 401s at the DO, so no
data access outlives the refresh.

### Consent hooks

#### `POST /internal/consent`

**Auth:** service secret required

Records a change of marketing consent. The only legitimate caller is our own Kratos instance,
whose after-registration and after-settings web hooks post here; `docs/consent.md` holds the
config block for each environment and the reasoning behind the split between the trait and this
record.

Authenticated by the `KRATOS_HOOK_SECRET` Worker secret (`wrangler secret put
KRATOS_HOOK_SECRET`, per environment; dev reads it from the gitignored `dovecote/.dev.vars`),
presented in an `X-Kratos-Hook-Secret` header rather than `Authorization` because Kratos's
`api_key` hook auth sends a bare value, not a `Bearer` credential. Compared in constant time,
and checked before the body is read. An environment with no secret set refuses every call
(fail closed) — this route writes evidence for a legal claim, so a deploy that forgot the
secret should record nothing rather than record whatever it is told.

Body is `capsules::ConsentHookPayload`:

```json
{"identity_id": "<kratos_identity_uuid>", "granted": true, "source": "registration",
 "flow_id": "<kratos_flow_uuid>"}
```

`source` is one of `registration`, `settings`, `import`. `flow_id`, `ip` and `user_agent` are
all optional; the shipped hooks send only `flow_id`, and the reason the other two are accepted
but not sent is in `docs/consent.md`. `ip` and `user_agent` are truncated to
`capsules::MAX_CONSENT_CONTEXT_BYTES`. Neither the notice version nor a timestamp is accepted
from the caller: dovecote stamps `capsules::PRIVACY_NOTICE_VERSION` and the server clock,
because a value the caller supplies is an assertion rather than a record.

- `200 recorded` when a row was appended, `200 unchanged` when the flow left consent where it
  already was. Only transitions are stored, so a hook that fires on an unrelated settings save,
  or the same hook delivered twice, is a no-op — see
  `capsules::consent::consent_transition`.
- `400` for a body that is not a valid payload.
- `403` for a missing, wrong or unconfigured secret. **Never `401`** — the dashboard reads a
  401 from this API as "the session is gone" and signs the tab out, so no misconfiguration here
  may reach that path.
- `500` if the write itself fails. Kratos is configured with `response.ignore: true`, so a
  failure is logged on both sides and does not block the registration or settings save that
  triggered it; the trait still carries the person's choice, and `docs/consent.md` describes
  the reconciliation.


---

## Type reference

Every request/response shape above is defined in `capsules/src/lib.rs`:

- `Flock`, `FlockCreateRequest`, `FlockTransferRequest`
- `OrgRole`, `Organization`, `OrganizationMembership`, `OrganizationMember`,
  `OrganizationDetail`, `OrganizationInvite`, `OrganizationInviteCreated`,
  `OrganizationCreateRequest`, `OrganizationRenameRequest`,
  `OrganizationMemberRoleUpdateRequest`, `OrganizationInviteCreateRequest`,
  `OrganizationInviteAcceptRequest`, `OrgRoleEntry` (internal `X-Org-Roles` header entry)
- `OrganizationBusinessDetails`, `OrganizationBusinessDetailsRequest`, `TaxIdType`,
  `TaxIdStatus`, `MAX_TAX_ID_CHARS`, `MAX_BUSINESS_NAME_CHARS` — `capsules/src/tax_id.rs`,
  which also holds the shared format rules (`prepare_tax_id`, `parse_eu_vat`) and the
  save-versus-recheck state machine (`decide_status`, `recheck_status`)
- `Pigeon` / `PigeonRow`, `PigeonCreateRequest`, `PigeonUpdateRequest`,
  `PigeonFlockUpdateRequest`, `PigeonDetail`
- `PigeonAcl`, `PigeonAclUpdateRequest`
- `PigeonShadow` / `PigeonShadowRow`, `PigeonShadowUpdateRequest`, `PigeonShadowReportRequest`,
  `JsonString`
- `Connector` (`Https(HttpsConfig)` | `Coap(CoapConfig)` | `Mqtt(MqttConfig)`), `CoapPskLookup`
  (service-internal, the `/internal/device-psk/:pigeon_id` response)
- `ConsentHookPayload`, `ConsentKind`, `ConsentSource`, `MARKETING_CONSENT_LABEL`,
  `MAX_CONSENT_CONTEXT_BYTES` — `capsules/src/consent.rs`, which also holds the transition
  rule (`consent_transition`) the `/internal/consent` route applies; the notice version it
  stamps is the crate-root `PRIVACY_NOTICE_VERSION`
- `MQTT_TLS_PORT`, `MQTT_TOPIC_TELEMETRY`, `MQTT_TOPIC_SHADOW_REPORT`, `MQTT_TOPIC_LOGS`,
  `MQTT_TOPIC_SHADOW_TARGET` — the wire constants the broker mirrors
- `TelemetryLatest` / `TelemetryLatestRow`, `TelemetryHistoryPoint`, `TelemetryHistoryBucket`,
  `TelemetryHistoryQuery`, `TELEMETRY_HISTORY_BUCKET_TARGET`, `TelemetryEndpoint`,
  `PigeonTelemetryEndpointUpdateRequest`
- `PigeonLogChunk` / `PigeonLogChunkRow`, `MAX_LOG_CHUNK_BYTES`
- `LogDictionaryInfo`, `MAX_LOG_DICTIONARY_BYTES`
- `FirmwareImage`, `FirmwareTarget`, `FirmwareUploadQuery`, `MAX_FIRMWARE_BYTES`
- `FeedbackRequest`, `FeedbackCategory`, `format_feedback_email` (+ the `MAX_FEEDBACK_*` caps) —
  `capsules/src/feedback.rs`

`*Row` variants (e.g. `PigeonRow`, `PigeonShadowRow`) are internal DB-deserialization shapes and
never appear over the wire — only their non-`Row` counterparts do.

**One exception:** the [WebSocket frame types](#get-devicepigeonspigeon_idws)
(`WsInboundFrame`, `WsOutboundFrame`, and the `MAX_WS_FRAME_BYTES`/rate-limit constants) live in
`dovecote/src/objects/ws.rs`, not `capsules`. Every other type in this document is shared with
`fancier` (a Rust/Dioxus consumer), which is the whole reason `capsules` exists — but the only
other consumer of the WS wire format is the `pigeon` device library, which is Zephyr/C, not Rust,
so there's no second Rust crate to share these with. The wire shapes themselves (documented
above) are still normative; only the Rust type definitions are dovecote-local.
