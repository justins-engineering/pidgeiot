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
- **Base URL (staging):** `https://dovecote-staging.justinsengineeringservices.workers.dev`
- **Base URL (local dev):** `http://127.0.0.1:8787`

All examples below use placeholder IDs and credentials — `<pigeon_id>`, `<flock_id>`,
`<device_token>`, etc. Never substitute real secrets into a shared document or commit history.

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
`tls_psk_secret`) field, and **only** in the response to the route that just minted it — pigeon
create (`POST /flock/pigeons`) or token refresh (`POST /pigeons/:pigeon_id/token/refresh`).
Every other route that returns a `Pigeon` (`GET /pigeons/:id`, `GET /pigeons/:id/detail`,
`PUT /pigeons/:id`, `POST /pigeons/batch`) strips it to an empty string first
(`strip_secrets`, `objects/pigeons.rs`) — treat that field as write-once, read-never after the
initial mint.

A missing/malformed/expired/wrong-pigeon token gets `401 Unauthorized`.

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
- Status codes used throughout: `400` (malformed JSON, missing/invalid path param, empty
  telemetry report, empty log chunk), `401` (missing/invalid session cookie or device token),
  `403` (authenticated but not authorized — wrong ACL role, or CF Access rejection on staging),
  `404` (no matching route), `413` (log chunk over the size cap), `500` (internal error — DB
  connection failure, Durable Object dispatch failure, etc).
- A deleted pigeon's Durable Object is never actually destroyed (Cloudflare DOs have no
  "delete yourself" API — see `objects/pigeons.rs`'s `delete` handler) — its tables are just
  emptied. A `GET` against a deleted pigeon therefore returns `403 Forbidden` (no ACL rows left
  to authorize against), not `404`.
- `GET /device/pigeons/:pigeon_id/ws` is the one exception to "error responses are plain text
  HTTP status codes": a rejected upgrade (bad `Upgrade` header, bad token) is still a normal HTTP
  error response (`400`/`401`), but a problem discovered *after* the socket is open (oversize
  frame, malformed frame, frame flood) has no HTTP status to report — it's a WebSocket close
  code instead (`4001`-`4009`; see that route's own section for the full list).

## Rate & size limits

There is no general-purpose rate limiting in `dovecote` today (beyond whatever Cloudflare
applies at the platform level); the one route with a real per-IP limiter is `POST /errors`,
via Cloudflare's rate-limiter binding. The limits that do exist are:

| Limit | Value | Where |
|---|---|---|
| `POST /pigeons/batch` — pigeon IDs per request | 48 | `lib.rs` (Workers subrequest budget) |
| `POST /device/pigeons/:id/logs` — bytes per chunk | 16 KiB (`capsules::MAX_LOG_CHUNK_BYTES`) | `objects/pigeons.rs::report_logs_device`, `413` over the cap |
| Stored log chunks per pigeon | 200 (oldest silently pruned, not an error) | `objects/pigeons.rs::MAX_STORED_LOG_CHUNKS` |
| `GET .../telemetry/history` points per query | bucketed by default (`capsules::TELEMETRY_HISTORY_BUCKET_TARGET` = 360 buckets, unlimited range, no cap); `raw=true` caps at 5000 (`capsules::TELEMETRY_HISTORY_MAX_POINTS`) — the range's **newest** 5000, flagged by `X-Telemetry-Truncated`, not an error | `helpers/telemetry.rs` |
| `PUT /pigeons/:id/log-dictionary` — bytes per upload | 4 MiB (`capsules::MAX_LOG_DICTIONARY_BYTES`) | `lib.rs`, `413` over the cap |
| `GET /device/pigeons/:id/ws` — max WebSocket frame size | 16 KiB | `objects/ws.rs::MAX_WS_FRAME_BYTES`, connection closed (`4002`) over the cap |
| `GET /device/pigeons/:id/ws` — frame rate | 50 frames / rolling 10s window, per socket | `objects/ws.rs`, connection closed (`4008`) over the cap |
| `POST /pigeons/:id/shell` — device reply timeout | 10s default, 30s max (caller-configurable `timeout_ms`, clamped) | `objects/pigeons.rs::SHELL_TIMEOUT_DEFAULT_MS`/`SHELL_TIMEOUT_MAX_MS`, `504` over the wait |
| `POST /feedback` — bytes per raw body | 8 KiB (`capsules::MAX_FEEDBACK_BODY_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /feedback` — bytes in `message` | 4 KiB (`capsules::MAX_FEEDBACK_MESSAGE_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /feedback` — `contact_email` / `page_context` length | 254 / 512 bytes (`capsules::MAX_FEEDBACK_CONTACT_EMAIL_BYTES`/`MAX_FEEDBACK_PAGE_CONTEXT_BYTES`) | `lib.rs`, `400` over the cap |
| `POST /errors` — requests per IP | 20 / 60s (Cloudflare rate-limiter binding; counters are roughly per-colo) | `wrangler.toml` `[[ratelimits]]` + `lib.rs`, `429` over the limit — never `401` (the dashboard treats 401 as "session gone") |
| `POST /errors` — bytes per raw body | 16 KiB (`capsules::MAX_ERROR_REPORT_BYTES`) | `lib.rs`, `413` over the cap |
| `POST /errors` — bytes in `note` (manual JSON body) | 4 KiB (reuses `capsules::MAX_FEEDBACK_MESSAGE_BYTES`) | `lib.rs`, `413` over the cap |
| New-signature ops emails | 5 / hour, global; overflow folded into the next allowed email as a suppressed count | `helpers/errors.rs` |
| Stored error events per signature | newest 200 kept (each group's oldest 5 and all manual reports exempt) | `helpers/errors.rs` retention sweep on the 5-minute cron |
| Stored error events age | 90 days, keyed on server-side `received_at` | `helpers/errors.rs` retention sweep |

---

## Dashboard API

All routes below require a valid Kratos session cookie (`credentials: include` from a browser
client whose origin matches `ROOT_URL`) unless noted otherwise.

### Flocks

#### `GET /flocks`

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

There is no `PUT`/`DELETE /flocks/:id` route today, even though `capsules::FlockUpdateRequest`
exists as a type — it isn't wired to anything yet.

#### `POST /flocks/:flock_id/transfer`

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
with the membership row (remove a member and their org-derived access is gone on their next
request).

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
| Rename org (`PUT /orgs/:id`) | yes | yes | no |
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

Last-owner protection: an org must always retain at least one `owner` — demoting or removing
the only owner is refused with `409`, regardless of who asks.

The pigeon-side mapping, precisely: an org-shared pigeon carries a `pigeon_acl` row
`{ entity_id: <org id>, role: "owner" }`. A caller's effective rights through that row are
capped by their role in the org — `owner`/`admin` may exercise the row's full (owner-level)
rights; `member` is capped at member-level. Per-user ACL rows are unaffected.

#### `POST /orgs`

Creates an organization; the caller becomes its founding `owner` (an org can never exist
without one). Body: `capsules::OrganizationCreateRequest` (`{ name }`). Returns
`capsules::Organization` with `201` and a `Location: /orgs/<org_id>` header.

#### `GET /orgs`

Lists every org the caller belongs to, with the caller's own role —
`Vec<capsules::OrganizationMembership>` (`{ organization, role }`).

#### `GET /orgs/:org_id` — any member

Returns `capsules::OrganizationDetail`: the org, the caller's role, the full member list
(each `capsules::OrganizationMember` carries `email` — denormalized at join time — and
`invited_by`, the per-person audit trail), and pending invites (`invites` is only populated
for owner/admin callers; plain members get an empty list).

#### `PUT /orgs/:org_id` — owner/admin

Rename. Body: `capsules::OrganizationRenameRequest` (`{ name }`). Returns the updated
`capsules::Organization`.

#### `DELETE /orgs/:org_id` — owner

Deletes the org **only when it owns no flocks** (`409` otherwise — transfer or delete them
first). Membership and invite rows cascade. Returns `200` with an empty body.

#### `PUT /orgs/:org_id/members/:user_id` — owner

Changes a member's role. Body: `capsules::OrganizationMemberRoleUpdateRequest`
(`{ role }`). `409` if it would leave the org ownerless. Returns the updated
`capsules::OrganizationMember`.

#### `DELETE /orgs/:org_id/members/:user_id` — owner/admin, or self

Removes a membership row — **the revocation mechanism**: the removed user loses every
org-granted flock/pigeon right on their next request (the principal set is loaded
per-request; no ACL rows need rewriting). Admins can never remove owners; anyone may remove
themselves (leave); `409` if it would leave the org ownerless.

#### `POST /orgs/:org_id/invites` — owner/admin

Invites an email address at a given role. Body: `capsules::OrganizationInviteCreateRequest`
(`{ email, role }`); inviting at role `owner` is itself owner-only. Mints a random 128-bit+
token, stores **only its sha256 hash** (`organization_invites.token_hash`), and emails the
invite link (`<ROOT_URL>/invite?token=<token>`) through the platform's existing Resend
transport. In an environment with no `RESEND_API_KEY` configured (dev), the link is logged to
the Worker console instead — grab it from `wrangler dev` output. Returns `201` with
`capsules::OrganizationInviteCreated` (`{ invite, token, invite_url }`) — **the only place
the cleartext token ever appears** (write-once, same convention as device connector tokens);
`GET` reads return only hash-backed metadata.

Invites expire after **7 days** and are **single-use** (consumed atomically on accept).

#### `GET /orgs/:org_id/invites` — owner/admin

Pending (unconsumed, unexpired) invites — `Vec<capsules::OrganizationInvite>`.

#### `DELETE /orgs/:org_id/invites/:invite_id` — owner/admin

Revokes a pending invite. Idempotent (`200` even if already gone).

#### `POST /invites/accept`

Consumes an invite token for the **calling session** (requires an authenticated Kratos
session; the frontend's `/invite?token=` page routes unauthenticated visitors through
login/registration first). Body: `capsules::OrganizationInviteAcceptRequest` (`{ token }`).
Returns `201` with the new `capsules::OrganizationMember`; `404` for an invalid/expired/used
token; `409` if the caller is already a member (the invite is left unconsumed in that case).

**Token-alone acceptance — a documented tradeoff.** The token is a bearer credential:
whichever authenticated account presents it first joins, *regardless of which email that
account registered under*. This is deliberate — invitees routinely register under a different
address than the one the invite was sent to, and an email-match requirement would strand them
— and is compensated by the short (7-day) expiry, single-use consumption, hash-only storage,
and the inviter's ability to revoke pending invites and remove members. The alternative
(require `session email == invited email`) is stricter against forwarded/leaked invite
emails; if that ever matters more than invitee flexibility, the accept handler is the single
place to add the check.

### Billing

Billing attaches to **organizations** (a personal, org-less account is always the free tier).
Stripe hosts every payment surface — these routes mint redirect URLs and read state; card data
never touches this API. The read side is member-visible; the session mints are manager-only
(owner/admin), matching the rest of the org permission matrix.

#### `GET /orgs/:org_id/billing` — org: member

Returns `capsules::OrganizationBillingOverview`: the stored `plan`, `subscription_status`,
whether that status is currently `entitled`, the **`effective_plan`** actually being served
(entitlement-gated — a cancelled org shows its old `plan` but an `effective_plan` of the free
tier), `cancel_at_period_end`, `has_billing_account` (whether a Stripe customer exists — the
precondition for the portal), the usage-period bounds, and usage against allowance:
`billable_messages` / `included_messages`, `device_count` / `included_devices`. Usage-period
bounds are the org's Stripe billing period while a live subscription covers now, the calendar
month otherwise — the same anchoring the usage tally itself uses. `403` for non-members,
`404` for an unknown org.

#### `POST /orgs/:org_id/billing/checkout` — org: manage

Mints a Stripe Checkout session for a paid tier and returns
`capsules::BillingSessionUrl` (`{ url }`) for the dashboard to redirect to. Body:
`capsules::BillingCheckoutRequest` (`{ plan: builder|growth|scale|fleet }`) — `perch` is a
`400` (the free tier is not purchasable). The session carries three prices, resolved at
request time by `lookup_key` (never pinned ids): the licensed tier, the pooled
`message-overage` meter price, and that tier's own `device-overage-<tier>` meter price.
Creates (and remembers) the org's Stripe customer on first use. `502` when Stripe itself is
unreachable or the catalog is missing a price.

#### `POST /orgs/:org_id/billing/portal` — org: manage

Mints a Stripe Billing Portal session for the org's existing customer and returns
`capsules::BillingSessionUrl` (`{ url }`) — card updates, invoice history and cancellation
happen on Stripe's hosted page. Plan changes do **not**: Stripe's portal cannot switch a
multi-product subscription (and every checkout-minted subscription here is one), so tier
changes go through `PUT /orgs/:org_id/billing/plan` below. `409` if the org has no billing
account yet (checkout is the flow that creates one); `502` when Stripe is unreachable.

#### `PUT /orgs/:org_id/billing/plan` — org: manage

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

#### `POST /billing/webhook` — Stripe signature required

The Stripe event sink (not a dashboard route; authenticated by `Stripe-Signature`
HMAC verification against the endpoint signing secret, 5-minute replay window, `v1` scheme
only). Handles `customer.subscription.*` (writes plan/status/period onto the owning org,
idempotently, with out-of-order-event protection) and `checkout.session.completed` (binds the
Stripe customer to the originating org and applies the purchased subscription's state).
Deliveries are claimed in `stripe_webhook_events` before anything is applied, so replays and
concurrent deliveries are acknowledged without being re-applied.

### Pigeons

#### `POST /flock/pigeons` — flock: manage

Creates a pigeon inside a flock. Since task #12 this is gated on the **target flock**: a
personal flock's owner, or an org owner/admin for an org-owned flock (pre-org behavior never
checked flock ownership at all — a latent gap this closed). A pigeon created inside an
org-owned flock is seeded with **both** ACL rows: the creator's own `owner` row and the org's
`owner` row (so every org member gets role-mapped access immediately, and the org's access
survives the creator leaving). Body: `capsules::PigeonCreateRequest`
(`{ flock_id, serial?, name?, tags?, connector, board? }`) — `connector` is either
`{"Https": {"endpoint": "", "token": ""}}` or `{"Coap": {"endpoint": "", "token": ""}}`; the
`endpoint`/`token` you send are ignored and overwritten server-side (the DO mints its own
device endpoint URL and credential).

**Device-count entitlement.** An account served at the free tier (no org, or an org whose
subscription status isn't entitled) is capped at its included device count — creation past the
cap answers `403` with an upgrade hint. Paid, entitled tiers are never refused here; devices
past the included count bill as per-device overage instead. The check fails open on lookup
errors, and a refusal only ever blocks *growth*: existing devices keep ingesting regardless.

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
> repo's `docs/infra/coap-terminator.md` for deployment. `board` (task #20, phase 1) is optional — the pigeon's own
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

#### `GET /pigeons/:pigeon_id` — member

Returns `capsules::Pigeon` with the connector token/PSK stripped.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id> \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `GET /pigeons/:pigeon_id/detail` — member

Same as above plus `acl` (**only the caller's own ACL row**, not the full list — use
`GET /pigeons/:pigeon_id/acl` for that) and `shadow`. Returns `capsules::PigeonDetail`.

#### `PUT /pigeons/:pigeon_id` — member

Partial update. Body: `capsules::PigeonUpdateRequest` — every field (`flock_id`, `serial`,
`name`, `tags`, `connector`, `board`) is optional; omitted fields keep their current value
(`COALESCE` semantics, not a full replace). Returns the updated `capsules::Pigeon`. This is how
an existing (pre-task-#20) pigeon gets its `board` tagged after the fact.

```sh
curl -s -X PUT https://api.pidgeiot.com/pigeons/<pigeon_id> \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"Coop Sensor 1 (renamed)"}'
```

#### `DELETE /pigeons/:pigeon_id` — owner

Wipes the pigeon's Durable Object storage (its ACL, shadow, telemetry, and log tables) and
deletes its Postgres mirror row. Returns `200` with an empty body. As noted above, subsequent
`GET`s against the same ID return `403`, not `404` — the Durable Object still exists, just
empty.

#### `POST /pigeons/batch` — member (per pigeon)

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

#### `POST /pigeons/:pigeon_id/token/refresh` — owner

Mints a new Ed25519 keypair and device token for this pigeon, immediately revoking the old
one (see [Device authentication](#device-authentication-bearer-token) above). Returns the
updated `capsules::Pigeon` with the new token visible in `connector.Https.token`/`connector.Coap.token` — save it now, it won't be shown again.

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/<pigeon_id>/token/refresh \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `POST /pigeons/:pigeon_id/shell` — owner (task #34, v1)

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

#### `GET /pigeons/:pigeon_id/acl` — owner

Lists every ACL entry for the pigeon (`Vec<capsules::PigeonAcl>`), not just the caller's own
row.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/acl \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `POST /pigeons/:pigeon_id/acl` — owner

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
applied."

**Asymmetry to know about:** in *request* bodies, `target_config`/`current_config` are native
JSON objects (`serde_json::Value`). In every *response*, they come back as `capsules::JsonString`
— which serializes as a **JSON string containing JSON text**, not a nested object. You'll need a
second `JSON.parse()` (or equivalent) on those two fields specifically. This is a deliberate
wire-format choice (see `capsules::PigeonShadow`'s doc comment), not a bug.

#### `GET /pigeons/:pigeon_id/shadow` — member

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

#### `PUT /pigeons/:pigeon_id/shadow` — member

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

#### `POST /flocks/:flock_id/firmware?version=<string>&board=<string>` — flock: manage

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

#### `GET /flocks/:flock_id/firmware` — flock: view

Lists every firmware image uploaded for this flock, newest first. Same per-item shape as the
`POST` response above.

```sh
curl -s https://api.pidgeiot.com/flocks/<flock_id>/firmware \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

### Telemetry

Every telemetry value, on both the DO's latest-value table and the Postgres history table, is
stored and returned as a **string** — dovecote doesn't know or enforce a schema for what a
device reports. Where a value happens to parse as a number, the history endpoints also populate
a `value_num` float alongside the raw string, so numeric series can be queried/plotted without a
client-side cast.

#### `GET /pigeons/:pigeon_id/telemetry` — member

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

#### `GET /pigeons/:pigeon_id/telemetry/history` — member

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

#### Raw mode

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

#### `GET /flocks/:flock_id/telemetry/history` — flock: view

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

#### `PUT /pigeons/:pigeon_id/telemetry-endpoint` — member

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

#### `GET /pigeons/:pigeon_id/logs` — member

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

#### `PUT /pigeons/:pigeon_id/log-dictionary` — member

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

#### `GET /pigeons/:pigeon_id/log-dictionary` — member

Returns the stored dictionary verbatim (`Content-Type: application/json` — the raw Zephyr
database document, **not** a capsules type). `404` if none has been uploaded for this pigeon.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/log-dictionary \
  -H 'Cookie: ory_kratos_session=<session_token>' -o log_dictionary.json
```

#### `DELETE /pigeons/:pigeon_id/log-dictionary` — member

Removes the stored dictionary. Returns `200` with an empty body; idempotent (deleting when
none exists is still `200`). Deleting the pigeon itself also best-effort removes its stored
dictionary.

### Alerts

User-defined threshold/state alerts, evaluated both at telemetry-ingest time and by a five-minute
Cron Trigger sweep (for the absence-of-signal conditions below), with an at-most-one email per
fired/cleared transition. An alert is scoped to exactly one **pigeon** or one **flock** — never
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

`capsules::AlertChannel` is `{"Email":{"to":null}}` (deliver to the owning flock's stored
`owner_email`) or `{"Email":{"to":"you@example.com"}}` — an explicit override, which must match
one of the caller's own **verified** Kratos email addresses (`400` otherwise, so open signup
can't turn this into an arbitrary spam relay).

#### `POST /pigeons/:pigeon_id/alerts` — member

Body: `capsules::AlertDefinitionCreateRequest` (`{ name, condition, severity?, channel }`;
`severity` is `"Warning"` or `"Critical"`, defaulting to `"Warning"`).

```sh
curl -s -X POST https://api.pidgeiot.com/pigeons/<pigeon_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"High temp","condition":{"Threshold":{"key":"temp","comparator":"Gt","value":30.0}},"channel":{"Email":{"to":null}}}'
```

```json
{
  "id": "b3f1...",
  "user_id": "a7e2...",
  "scope": { "Pigeon": "59d0c929f912..." },
  "name": "High temp",
  "condition": { "Threshold": { "key": "temp", "comparator": "Gt", "value": 30.0 } },
  "severity": "Warning",
  "channel": { "Email": { "to": null } },
  "enabled": true,
  "created_at": "2026-07-17T15:21:08Z",
  "updated_at": "2026-07-17T15:21:08Z"
}
```

(`capsules::AlertDefinition`, `201`. A flock-scoped alert's `scope` is `{"Flock":"<flock_uuid>"}`
instead.)

#### `GET /pigeons/:pigeon_id/alerts` — member

Every alert scoped directly to this pigeon, newest first — **not** flock-scoped alerts that
happen to cover it (see [`GET /flocks/:flock_id/alerts`](#get-flocksflock_idalerts) for those).
Same per-item shape as the `POST` response above, as `Vec<capsules::AlertDefinition>`.

```sh
curl -s https://api.pidgeiot.com/pigeons/<pigeon_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `GET /pigeons/:pigeon_id/alerts/state` — member

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

#### `POST /flocks/:flock_id/alerts` — flock: manage

Same body/response shape as the pigeon-scoped `POST` above, with `scope: {"Flock":"<flock_id>"}`
in the response. Stricter than pigeon-scoped creation: only a flock **manager** (personal owner,
or an `owner`/`admin` org role on an org-owned flock) may create a flock-scoped alert, whereas any
ACL'd pigeon member may create a pigeon-scoped one.

```sh
curl -s -X POST https://api.pidgeiot.com/flocks/<flock_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"name":"Fleet offline","condition":{"DeviceState":{"state":"Offline","min_duration_secs":300}},"severity":"Critical","channel":{"Email":{"to":null}}}'
```

#### `GET /flocks/:flock_id/alerts` — flock: view

Every alert scoped to this flock, newest first, as `Vec<capsules::AlertDefinition>`.

```sh
curl -s https://api.pidgeiot.com/flocks/<flock_id>/alerts \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `GET /flocks/:flock_id/alerts/state` — flock: view

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

#### `PUT /alerts/:alert_id` — alert owner

Partial update — an omitted field keeps its current value. Body:
`capsules::AlertDefinitionUpdateRequest` (`{ name?, condition?, severity?, channel?, enabled? }`).
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

#### `DELETE /alerts/:alert_id` — alert owner

Same ownership gate as `PUT` above. Returns `200` with an empty body. `alert_state` rows for this
definition cascade-delete via the table's own foreign key.

### Feedback

#### `POST /feedback` — **no auth required** (optionally authenticated)

The dashboard's feedback form. Unlike every other Dashboard route, this one does **not**
require a Kratos session — public marketing pages link the same form. If a valid session cookie
*is* present, dovecote resolves it server-side and includes the submitter's identity id/email in
the notification email; the submitter is never trusted from the request body.

Body: `capsules::FeedbackRequest`. Only `message` is required; `category` is one of `"bug"`,
`"feature_request"`, `"general"` (treated as general when omitted).

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

There is no per-IP rate limiting in-route (see "Rate & size limits" above — `POST /errors`
is the one route that carries one); platform-level protection (a Cloudflare WAF rate rule on
`POST /feedback`, or Turnstile) is the intended follow-up if abuse appears.

### Error reporting

#### `POST /errors` — **no auth required** (identity only on the manual JSON path)

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
- `build` must match the release artifact's `dxh` + 16-hex shape or it is blanked.
- `occurred_at` is clamped to ±24h of server time; retention keys on `received_at`.
- `client_event_id` is a client-minted correlation id (shown on the crash screen) that joins
  a manual note to the automatic crash it describes — a hint, not a key.
- A **new** signature sends one ops email (`[ERROR] New: …`) to `OPS_ALERT_EMAIL`, under a
  global budget of 5/hour with the overflow folded into the next allowed email.

Rejections: `400` (unsupported `Content-Type`, invalid JSON, unknown fields on the text/plain
envelope, empty `note`), `413` (body or `note` over cap), `429` (rate limit).

#### `DELETE /errors` — session required

Erases every identified error-report row (`user_id` + `report_note`) belonging to the caller;
automatic reports never stored an identity, so there is nothing of theirs to erase there.
Returns `{"deleted": <count>}`. The manual account-deletion runbook runs the same statement
directly (documented in `infra/migrations/2026-08-19-error-reporting.sql`).

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

### `GET /demo/pigeons/:pigeon_id/telemetry`

Latest-value read — identical response shape to the dashboard's
[`GET /pigeons/:pigeon_id/telemetry`](#get-pigeonspigeon_idtelemetry) above (`Vec<capsules::
TelemetryLatest>`), reading the same Durable Object table, just without the `X-User-Id`/ACL
check (`objects/pigeons.rs::get_telemetry_latest_demo`).

```sh
curl -s https://api.pidgeiot.com/demo/pigeons/<demo_pigeon_id>/telemetry
```

### `GET /demo/pigeons/:pigeon_id/telemetry/history`

History read — same query params and response shape (bucketed by default,
`Vec<capsules::TelemetryHistoryBucket>`; `raw=true` for the flat/capped
`Vec<capsules::TelemetryHistoryPoint>`, Greptime-first/Postgres-fallback) as the dashboard's
[`GET /pigeons/:pigeon_id/telemetry/history`](#get-pigeonspigeon_idtelemetryhistory) above, just
without the ACL probe.

```sh
curl -s "https://api.pidgeiot.com/demo/pigeons/<demo_pigeon_id>/telemetry/history?key=temp_c"
```

### `GET /demo/pigeons/:pigeon_id/alerts`

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

### `GET|HEAD /.well-known/api-catalog` — **no auth required**

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

### `GET /device/pigeons/:pigeon_id/shadow`

Reads the current shadow — same shape as the dashboard's `GET /pigeons/:pigeon_id/shadow`
above (same `JsonString`-wrapped-fields caveat applies).

```sh
curl -s https://api.pidgeiot.com/device/pigeons/<pigeon_id>/shadow \
  -H 'Authorization: Bearer <device_token>'
```

### `POST /device/pigeons/:pigeon_id/shadow`

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
message allowance, the same as a telemetry report. The free-tier allowance fuse (see the
telemetry route below) never `429`s this route, though — a device can always confirm the
config it applied.

### `POST /device/pigeons/:pigeon_id/telemetry`

Reports telemetry. Body: a **flat JSON object of string key/value pairs** — no nesting, no
typed values; this matches the wire shape the `pigeon` Zephyr device library's
`pigeon_set_shadow_param()`/`pigeon_shadow_flush()` calls produce. `400` if the body is empty
or not a flat string map.

**Free-tier allowance fuse.** On a free-tier account that has exhausted its monthly pooled
message allowance, this route answers `429 Too Many Requests` (after the bearer token has been
verified) for the rest of the billing period — the `pigeon` device library backs off and keeps
unsent readings queued, so data is delayed rather than lost. Paid, entitled tiers are never
paused; their over-allowance usage bills as metered overage instead. The check fails open: a
usage-lookup failure never blocks ingestion. Only this route pauses: shadow report-backs and
log uploads keep counting toward the same allowance but are never refused by the fuse.

```sh
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/telemetry \
  -H 'Authorization: Bearer <device_token>' \
  -H 'Content-Type: application/json' \
  -d '{"temp":"21.5","status":"ok"}'
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

(the metrics you just sent, echoed back).

### `POST /device/pigeons/:pigeon_id/logs`

Ingests one binary log chunk — the request body **is** the chunk, sent as raw bytes (not
wrapped in JSON, no base64 encoding needed on the way in — that only happens on the read side,
`GET /pigeons/:pigeon_id/logs`). Intended for Zephyr's `CONFIG_LOG_DICTIONARY_SUPPORT`
token-compressed log records, but dovecote never inspects the contents — it's opaque storage,
decoded host-side against the firmware's own dictionary/ELF.

- `400` if the body is empty.
- `413 Payload Too Large` if the body exceeds 16 KiB (`capsules::MAX_LOG_CHUNK_BYTES`).
- `200` with an empty body on success.

An accepted chunk counts as one billable device message against the owning account's message
allowance, the same as a telemetry report; the free-tier allowance fuse never `429`s this route.

```sh
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/logs \
  -H 'Authorization: Bearer <device_token>' \
  --data-binary @log-chunk.bin
```

### `GET /device/pigeons/:pigeon_id/firmware`

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

### `GET /device/pigeons/:pigeon_id/ws`

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
| device → server | `telemetry` | `metrics: {string: string}` | Same handling as `POST /device/pigeons/:id/telemetry`: an immediate latest-value upsert in the pigeon's own Durable Object, plus (environment-dependent — see below) a queued write for history/forwarding. |
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

None of these three are "recoverable" mid-connection — reconnect (a fresh `GET .../ws`) to
resume after any of them.

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

**Block-wise transfer (RFC 7959).** Firmware downloads are always served Block2-wise (1024-byte
blocks max, szx ≤ 6; BERT szx 7 is down-negotiated to 6): each Block2 request maps directly to
an HTTP `Range` request against dovecote — block N = `bytes=N*size-(N*size+size-1)` — so the
image never transits the terminator as a whole. Block 0's response carries `Size2` (total image
bytes) and an `ETag` (first 8 bytes of the image's sha256) for mid-transfer change detection. A
firmware GET without a Block2 option gets block 0 with the more-bit set (spontaneous Block2).
Large JSON responses are spontaneously Block2'd over UDP only (>1024 bytes; TCP frames are sent
whole, matching the minimal `~/pigeon` client). POST bodies may be sent Block1-wise (2.31
Continue per intermediate block; 64 KiB reassembly cap; 4.08 on a broken sequence).

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

**Connection ID (RFC 9146) is supported.** The DTLS listener runs mbedTLS with CID enabled, so
a PSM/NAT'd cellular device whose NAT mapping dies during sleep can keep its DTLS association
across an address/port rebind instead of paying a fresh handshake (~2 RTT with these PSK
suites) on every wake. Devices that offer no CID negotiate a plain session and work unchanged.
The `loft` repo's `docs/infra/coap-terminator.md` documents the deployment posture.

---

## Service-internal API

### `GET /internal/coap-psk/:pigeon_id` — **service secret required**

PSK resolution for the CoAP terminator — the only route in this API authenticated by a shared
service secret rather than a Kratos session or device token:
`Authorization: Bearer <COAP_SERVICE_SECRET>` (a Worker secret, set per environment via
`wrangler secret put COAP_SERVICE_SECRET`, never a `[vars]` entry; `loft` holds the same value
in its own `COAP_SERVICE_SECRET` env var). The secret is compared in constant time, and is
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
- `404` for an unknown identity or an `Https`-connector pigeon (no PSK exists).
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

---

## Type reference

Every request/response shape above is defined in `capsules/src/lib.rs`:

- `Flock`, `FlockCreateRequest`, `FlockTransferRequest`
- `OrgRole`, `Organization`, `OrganizationMembership`, `OrganizationMember`,
  `OrganizationDetail`, `OrganizationInvite`, `OrganizationInviteCreated`,
  `OrganizationCreateRequest`, `OrganizationRenameRequest`,
  `OrganizationMemberRoleUpdateRequest`, `OrganizationInviteCreateRequest`,
  `OrganizationInviteAcceptRequest`, `OrgRoleEntry` (internal `X-Org-Roles` header entry)
- `Pigeon` / `PigeonRow`, `PigeonCreateRequest`, `PigeonUpdateRequest`, `PigeonDetail`
- `PigeonAcl`, `PigeonAclUpdateRequest`
- `PigeonShadow` / `PigeonShadowRow`, `PigeonShadowUpdateRequest`, `PigeonShadowReportRequest`,
  `JsonString`
- `Connector` (`Https(HttpsConfig)` | `Coap(CoapConfig)`), `CoapPskLookup` (service-internal,
  the `/internal/coap-psk/:pigeon_id` response)
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
