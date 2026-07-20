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

Flocks have no separate ACL table — a flock's owner is just `flocks.user_id`, checked directly
against the caller's Kratos ID. There is no flock-sharing mechanism today.

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
applies at the platform level). The limits that do exist are:

| Limit | Value | Where |
|---|---|---|
| `POST /pigeons/batch` — pigeon IDs per request | 48 | `lib.rs` (Workers subrequest budget) |
| `POST /device/pigeons/:id/logs` — bytes per chunk | 16 KiB (`capsules::MAX_LOG_CHUNK_BYTES`) | `objects/pigeons.rs::report_logs_device`, `413` over the cap |
| Stored log chunks per pigeon | 200 (oldest silently pruned, not an error) | `objects/pigeons.rs::MAX_STORED_LOG_CHUNKS` |
| `GET .../telemetry/history` rows per query | 5000 (silently truncated, not an error) | `helpers/telemetry.rs` |
| `GET /device/pigeons/:id/ws` — max WebSocket frame size | 16 KiB | `objects/ws.rs::MAX_WS_FRAME_BYTES`, connection closed (`4002`) over the cap |
| `GET /device/pigeons/:id/ws` — frame rate | 50 frames / rolling 10s window, per socket | `objects/ws.rs`, connection closed (`4008`) over the cap |

---

## Dashboard API

All routes below require a valid Kratos session cookie (`credentials: include` from a browser
client whose origin matches `ROOT_URL`) unless noted otherwise.

### Flocks

#### `GET /flocks`

Lists every flock owned by the caller, each with its member pigeon IDs.

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

Returns `201`-shaped `capsules::Flock` JSON (empty `pigeon_ids`). `400` if `name` is empty.

There is no `PUT`/`DELETE /flocks/:id` route today, even though `capsules::FlockUpdateRequest`
exists as a type — it isn't wired to anything yet.

### Pigeons

#### `POST /flock/pigeons`

Creates a pigeon inside a flock. Body: `capsules::PigeonCreateRequest`
(`{ flock_id, serial?, name?, tags?, connector }`) — `connector` is either
`{"Https": {"endpoint": "", "token": ""}}` or `{"Coap": {"endpoint": "", "token": ""}}`; the
`endpoint`/`token` you send are ignored and overwritten server-side (the DO mints its own
device endpoint URL and credential).

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
`name`, `tags`, `connector`) is optional; omitted fields keep their current value (`COALESCE`
semantics, not a full replace). Returns the updated `capsules::Pigeon`.

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

### Firmware

Firmware images (signed MCUboot application binaries) are catalogued per-**flock**, not
per-pigeon — they're shared across every pigeon in a flock's hardware fleet rather than
duplicated per-pigeon. The binary itself lives in R2, content-addressed by `sha256`
(`firmware/<sha256>.bin`); only metadata lives in Postgres. A pigeon's *assigned* firmware is a
separate, per-pigeon concern set via its own shadow (see above), not here.

#### `POST /flocks/:flock_id/firmware?version=<string>` — flock owner

Uploads a firmware image. The request body **is** the image, sent as raw bytes (like
`POST /device/pigeons/:pigeon_id/logs`, not wrapped in JSON). `size` and `sha256` are always
computed server-side from the uploaded bytes — never trust a client-supplied hash.

- `400` if `version` is missing/empty, or the body is empty.
- `403` if the caller isn't the flock's owner.
- `413 Payload Too Large` if the body exceeds ~2 MiB (`capsules::MAX_FIRMWARE_BYTES`).

Re-uploading identical bytes to the same flock (even under a different `version` label) updates
the existing catalog row in place rather than creating a duplicate — both the Postgres row and
the R2 object are content-addressed by `(flock_id, sha256)`/`sha256` respectively.

```sh
curl -s -X POST 'https://api.pidgeiot.com/flocks/<flock_id>/firmware?version=0.1.0+0' \
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
  "uploaded_at": "2026-07-17T15:21:08Z"
}
```

(`capsules::FirmwareImage`.)

#### `GET /flocks/:flock_id/firmware` — flock owner

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
| `since` | RFC 3339 timestamp | inclusive lower bound on `reported_at` |
| `until` | RFC 3339 timestamp | inclusive upper bound on `reported_at` |

```sh
curl -s "https://api.pidgeiot.com/pigeons/<pigeon_id>/telemetry/history?key=temp" \
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

(`Vec<capsules::TelemetryHistoryPoint>`, capped at 5000 rows, oldest first.) **Only populated
for reports made while the pigeon had no `telemetry_endpoint` configured** — see the next
section.

#### `GET /flocks/:flock_id/telemetry/history` — flock owner

Same shape and query params as above, across every pigeon in the flock. Unlike the pigeon-scoped
route, this checks *flock* ownership (`flocks.user_id`), not any pigeon's ACL — so a pigeon
shared with you via its own ACL, but living in a flock you don't own, won't show up here even
though `GET /pigeons/:pigeon_id/telemetry/history` would work for it directly.

```sh
curl -s "https://api.pidgeiot.com/flocks/<flock_id>/telemetry/history?since=2026-07-17T00:00:00Z" \
  -H 'Cookie: ory_kratos_session=<session_token>'
```

#### `PUT /pigeons/:pigeon_id/telemetry-endpoint` — member

Sets or clears a per-pigeon forwarding target: when configured, every telemetry report for
this pigeon is forwarded as an **InfluxDB line protocol v2 HTTP write** (GreptimeDB-compatible)
to that endpoint *instead of* being written into dovecote's own Postgres history table above.
The Durable Object's own latest-value table (`GET /pigeons/:pigeon_id/telemetry`) is unaffected
either way — it always gets written.

Body: `capsules::PigeonTelemetryEndpointUpdateRequest` — `{"telemetry_endpoint": {...}}` to
set/replace, or `{"telemetry_endpoint": null}` to clear (revert to Postgres history).
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

### `POST /device/pigeons/:pigeon_id/telemetry`

Reports telemetry. Body: a **flat JSON object of string key/value pairs** — no nesting, no
typed values; this matches the wire shape the `pigeon` Zephyr device library's
`pigeon_set_shadow_param()`/`pigeon_shadow_flush()` calls produce. `400` if the body is empty
or not a flat string map.

```sh
curl -s -X POST https://api.pidgeiot.com/device/pigeons/<pigeon_id>/telemetry \
  -H 'Authorization: Bearer <device_token>' \
  -H 'Content-Type: application/json' \
  -d '{"temp":"21.5","status":"ok"}'
```

**Response behavior differs by environment.** In an environment with a telemetry queue bound
(currently staging only — `TELEMETRY_QUEUE` in `wrangler.toml`), the gateway synchronously
verifies the bearer token against the Durable Object, then enqueues the report and returns
immediately:

```
202 Accepted
{}
```

The actual write (both the Durable Object's latest-value upsert and, depending on
`telemetry_endpoint`, either the Postgres history write or the external line-protocol forward)
happens asynchronously afterward — a `202` confirms the report was authenticated and queued, not
that it's been persisted yet. In an environment with no queue bound (dev, and production today),
the same auth + write happens synchronously in one round trip and returns:

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
| server → device | `shadow_update` | `shadow: <capsules::PigeonShadow, same shape as the GET .../shadow responses above>` | Pushed **immediately** whenever this pigeon's `target_config` changes via a dashboard `PUT /pigeons/:id/shadow` (including a firmware assignment, which reuses that same route) — this is the headline reason this endpoint exists: no more waiting for the device's next poll to learn about a new target. |
| server → device | `pong` | — | Reply to a device-sent `ping`. |

```json
// device -> server
{"type":"telemetry","metrics":{"temp":"21.5","status":"ok"}}
{"type":"shadow_report","current_version":1,"current_config":{"telemetry_interval":60}}
{"type":"ping"}
```

```json
// server -> device, pushed the moment a dashboard PUT lands
{"type":"shadow_update","shadow":{"target_version":2,"current_version":1,"target_config":"{\"telemetry_interval\":30}","current_config":"{\"telemetry_interval\":60}","updated_at":1784390937}}
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

**`TELEMETRY_QUEUE` and the `telemetry` frame — same environment-dependent behavior as the HTTP
route** (see [`POST /device/pigeons/:pigeon_id/telemetry`](#post-devicepigeonspigeon_idtelemetry)
above). Where a telemetry queue is bound (staging and production today), a `telemetry` frame's
metrics are upserted into the Durable Object's latest-value table synchronously, then enqueued
for the same consumer path the HTTP route uses (Postgres history write, or an external
line-protocol forward if `telemetry_endpoint` is configured) — but since the frame already
arrived on an authenticated connection, there's no separate verify-before-enqueue round trip the
way the HTTP route needs (auth happened once, at socket accept). Where no queue is bound (dev),
the Durable Object writes Postgres history directly instead, so telemetry sent over the socket
doesn't silently skip history in that environment.

**No response is sent for `telemetry`/`shadow_report` frames themselves** — there's no
frame-level ack. Read back the result via the ordinary HTTP routes (`GET
/pigeons/:pigeon_id/telemetry`, `GET /pigeons/:pigeon_id/shadow`) if you need confirmation, or
rely on the `shadow_update` push for the shadow side.

---

## Type reference

Every request/response shape above is defined in `capsules/src/lib.rs`:

- `Flock`, `FlockCreateRequest`
- `Pigeon` / `PigeonRow`, `PigeonCreateRequest`, `PigeonUpdateRequest`, `PigeonDetail`
- `PigeonAcl`, `PigeonAclUpdateRequest`
- `PigeonShadow` / `PigeonShadowRow`, `PigeonShadowUpdateRequest`, `PigeonShadowReportRequest`,
  `JsonString`
- `Connector` (`Https(HttpsConfig)` | `Coap(CoapConfig)`)
- `TelemetryLatest` / `TelemetryLatestRow`, `TelemetryHistoryPoint`, `TelemetryHistoryQuery`,
  `TelemetryEndpoint`, `PigeonTelemetryEndpointUpdateRequest`
- `PigeonLogChunk` / `PigeonLogChunkRow`, `MAX_LOG_CHUNK_BYTES`
- `FirmwareImage`, `FirmwareTarget`, `FirmwareUploadQuery`, `MAX_FIRMWARE_BYTES`

`*Row` variants (e.g. `PigeonRow`, `PigeonShadowRow`) are internal DB-deserialization shapes and
never appear over the wire — only their non-`Row` counterparts do.

**One exception:** the [WebSocket frame types](#get-devicepigeonspigeon_idws)
(`WsInboundFrame`, `WsOutboundFrame`, and the `MAX_WS_FRAME_BYTES`/rate-limit constants) live in
`dovecote/src/objects/ws.rs`, not `capsules`. Every other type in this document is shared with
`fancier` (a Rust/Dioxus consumer), which is the whole reason `capsules` exists — but the only
other consumer of the WS wire format is the `pigeon` device library, which is Zephyr/C, not Rust,
so there's no second Rust crate to share these with. The wire shapes themselves (documented
above) are still normative; only the Rust type definitions are dovecote-local.
