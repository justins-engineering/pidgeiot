# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

PidgeIoT is an edge-native IoT platform built in Rust, structured as a Cargo workspace with three crates:

- **`dovecote`** (backend): serverless edge router on Cloudflare Workers + Durable Objects. Compiles to a `cdylib` via `worker-build`. Handles device ingestion, provisioning, and session validation.
- **`fancier`** (frontend): WebAssembly SPA built with Dioxus 0.7 + TailwindCSS/DaisyUI. The human-facing dashboard.
- **`capsules`** (shared models): serde structs/RPC schemas shared by both `dovecote` and `fancier` so frontend/backend stay in sync. Keep this crate free of Worker- or Dioxus-specific dependencies.

Auth/identity is handled by a self-hosted Ory Kratos instance (via `docker-compose.yml`) for dashboard users, plus per-pigeon Ed25519 keypairs and compact binary bearer tokens (not JWTs) for device auth — see `dovecote` below.

## Development commands

Three services run in parallel, each in its own terminal, from repo root unless noted:

```sh
# 1. Auth + DB (Kratos, Postgres, MailSlurper)
docker-compose -f docker-compose.yml up --force-recreate

# 2. Edge backend (dovecote) — served at http://127.0.0.1:8787
cd dovecote && bunx wrangler dev --ip 127.0.0.1 --port 8787 --env dev

# 3. Frontend (fancier) — served at http://127.0.0.1:4455
cd fancier && dx serve --addr 127.0.0.1 --port 4455

# Live CSS rebuild while developing fancier
cd fancier && bunx @tailwindcss/cli -i ./assets/tailwind.css -o ./assets/styling/main.css --watch
```

- Kratos Admin UI: http://127.0.0.1:3000
- MailSlurper (local email capture): http://127.0.0.1:4436

Rebuilding the architecture diagram (fancier only):
```sh
cd fancier && bunx mmdc -i assets/architecture.mmd -o assets/images/architecture.svg -b transparent
```

Standard Cargo workflows apply per-crate (`cargo check -p dovecote`, `cargo check -p fancier`, `cargo check -p capsules`) — `dovecote` and `fancier` both target wasm/Workers, so a plain `cargo build` at the workspace root will not fully validate them; use `wrangler dev`/`dx serve` (above) or `worker-build`/`dx build` for real compilation checks.

Formatting: `tab_spaces = 2` (see `rustfmt.toml` in root, `dovecote/`, and `fancier/`) — this repo uses 2-space indentation everywhere, not the Rust default of 4.

## Architecture

### `dovecote` — edge router + Durable Objects

- `src/lib.rs` — the single Cloudflare Worker entrypoint (`#[event(fetch, ...)]`). Defines all HTTP routes on a `worker::Router`. Every route handler manually attaches CORS via `.with_cors(&cors)` and returns explicit error `Response`s (no `?`-propagation through route closures — see git history: "Unroll router let chains to avoid silent failures", "Response::Error() can't fail, unwrap instead of ?"). When adding routes, follow the existing pattern of `let Ok(x) = ... else { return Response::error(...) }` rather than `?`.
- CORS is **not** a global static — each route closure calls `build_cors(&ctx.env, &req)` itself (`lib.rs`), matching the request's `Origin` header against `ROOT_URL` and handing `Cors::with_origins` exactly that one matching value (or `ROOT_URL` as an inert non-matching default). This exists because `worker::Cors` joins every configured origin into `Access-Control-Allow-Origin` with commas rather than matching per-request — a comma-joined value is invalid per the CORS spec and real browsers silently reject it once more than one origin is configured, so a single shared multi-origin `Cors` can't be used here. It also can't be computed once in `main` and shared by reference across routes, since each route is a separate `async move` closure and `Cors` isn't `Copy`. `ROOT_URL` (`[vars]`/`[env.dev.vars]`, `wrangler.toml`) is the frontend's own origin in both environments — `https://pidgeiot.com` in production, the local `dx serve` address in dev.
- Two local macros in `lib.rs` remove boilerplate: `get_pigeon_do!` (resolves `pigeon_id` → Durable Object stub) and `get_db!` (opens a Postgres client via Hyperdrive, mapping failure to a 500). Both take the caller's local `cors` as a final `$cors:expr` argument (needed for their early-return error paths) since macro hygiene means they can't see a caller-scope `cors` implicitly.
- **Dual persistence model**: each `Pigeon` is authoritative in its own Durable Object (SQLite-backed, one row per DO), and is also mirrored into Postgres (via Cloudflare Hyperdrive) for cross-pigeon querying (e.g. listing a flock's pigeons). The DO is the source of truth; every mutating route proxies to the DO first, then best-effort syncs the result to Postgres (`update_pigeon_pg_db`, `insert_pigeon_pg_db`, etc. in `src/helpers/pigeons.rs`) — a Postgres sync failure is logged but does not fail the request.
- `src/objects/pigeons.rs` — the `Pigeons` Durable Object. Owns its own embedded SQLite schema (created in `DurableObject::new`, mirrors `init-db.sql`'s Postgres schema but with SQLite triggers for `updated_at`/immutability/shadow version bumping). Internal routes are dispatched by string path match (`/pigeon/get`, `/pigeon/create`, `/pigeon/shadow/update`, etc.) and are reached only via `proxy_to_pigeon_do` from the top-level router — never exposed directly to the internet.
- Authorization inside the DO is header-based, not cookie-based, for user-facing routes: the router resolves the Kratos session into a user id and forwards it as `X-User-Id`; `is_authorized`/`is_owner` in `objects/pigeons.rs` check that header against the DO's local `pigeon_acl` table. The device-facing route (`GET /device/pigeons/:id/shadow`, dispatched inside the DO as `/pigeon/device/shadow`) does **not** go through `X-User-Id`/ACL at all — a device has no Kratos user identity. Instead `get_shadow_device` (`objects/pigeons.rs`) verifies the request's `Authorization` bearer token directly against this pigeon's own `device_public_key` column, via `verify_device_token` (`objects/helpers.rs`). This used to be gated by a `X-User-Id: ""` sentinel that `is_authorized` had no special case for, permanently 403ing the route — replacing ACL-based auth with real per-pigeon cryptographic auth for this route removed the need for that sentinel entirely.
- Device auth mechanism (`src/objects/helpers.rs`, `src/objects/pigeons.rs`) — **not JWT**: each pigeon gets its own Ed25519 keypair, generated fresh in the DO on `create` and on `token/refresh` (`mint_device_credential`). Only the public key is ever persisted (in the DO's own SQLite `pigeons.device_public_key` column — never mirrored to Postgres, never returned over the API); the private key signs one token and is discarded. The token itself is a 69-byte binary blob — `version(1) | expires_at(4, u32 LE unix seconds) | signature(64)` — base64url-encoded for transport, with no `pigeon_id`/subject claim: the binding to a specific pigeon comes from which pigeon's stored public key the DO verifies against, not from a claim inside the token. There are no `DEVICE_SEED`/`DEVICE_PUBLIC_KEY` Worker secrets anymore — verification happens entirely inside the owning DO (`verify_device_token`), reached via `proxy_to_pigeon_do` forwarding the caller's `Authorization` header straight through (`src/helpers/pigeons.rs`). **Refreshing a device's token revokes the previous one**: `refresh_token` mints a brand-new keypair and overwrites `device_public_key`, so the old token's signature can never verify again, regardless of its embedded expiry.
- `src/helpers/auth.rs` now holds only `authenticate_browser` (Kratos session cookie validation for dashboard users).
- Connectors: a `Pigeon` has one `Connector` (`Https` or `Coap`), each carrying its own endpoint + token/PSK. Tokens are always stripped before returning a `Pigeon` from `GET` routes (`objects/pigeons.rs::get`/`get_detail`) — they're only ever returned on `create` or `token/refresh`. The CoAP connector speaks CoAP-over-TLS/TCP (RFC 8323, `coaps+tcp://`), not CoAP-over-DTLS/UDP — `COAP_ENDPOINT`/`build_coap_endpoint` (`objects/pigeons.rs`) emit `coaps+tcp://`, and `capsules::CoapConfig`'s PSK fields are named `tls_psk_identity`/`tls_psk_secret` accordingly. This matches the sibling `~/pigeon` Zephyr device library, which has no on-device UDP support and was already ahead on this — see its `CLAUDE.md`/`pigeon.h` for the device-side rationale.
- Pigeon deletion (`delete` in `objects/pigeons.rs`, dispatched at `/pigeon/delete`, gated by `is_owner`) wipes the DO's own tables — `pigeon_acl` explicitly (it has no FK to `pigeons`, since each DO holds exactly one pigeon's ACL) and `pigeons` itself (`pigeon_shadow` cascades via `ON DELETE CASCADE`). Durable Objects have no "delete yourself" API; emptying its storage is the idiomatic equivalent. The gateway's `DELETE /pigeons/:pigeon_id` route already proxied here and synced the deletion to Postgres before this handler existed — it just 404'd with nothing to dispatch to.
- **Gotcha**: `SqlCursor::one()` (`worker` crate) throws an uncaught JS exception — crashing the DO, not a catchable `Result::Err` — when a query returns zero rows. Every handler in `objects/pigeons.rs` used to assume the `pigeons`/`pigeon_shadow` tables always have exactly one row, which was safe only because nothing could ever leave them empty before `delete()` existed. Use `to_array()` and take the first element instead (see `one_row` in `objects/pigeons.rs`) for any query that could plausibly run against an already-deleted pigeon's DO — plain `.one()` is only safe immediately after an `INSERT ... RETURNING` in the same request.
- `wrangler.toml` defines two environments: default (production, `api.pidgeiot.com`) and `[env.dev]` (local, `127.0.0.1`, using `localConnectionString` for Hyperdrive instead of the production binding id).

### `capsules` — shared models

Plain serde structs with no Worker/Dioxus dependencies, consumed by both other crates via `capsules.workspace = true`. Notable pattern: every entity has a `*Row` variant (deserializes DB-native types, e.g. unix-epoch floats/integers for timestamps) and a public API variant (RFC 3339 `OffsetDateTime`), connected by a `From<XRow> for X` impl. When adding a new persisted field, update both variants and the conversion.

### `fancier` — Dioxus SPA

- `src/lib.rs` defines the full route table (`Route` enum, `Routable` derive) and the top-level `App` component. Routes under `#[layout(Wrapper)] #[layout(AuthGuard)]` require an authenticated Kratos session (checked client-side via `session_cookie_valid()` against `SESSION_COOKIE_NAME`); everything else (login, registration, marketing pages) is public.
- Global reactive state is plain Dioxus `Signal`s provided via context, not a store library: `Session { state: Signal<AuthState> }` for auth, `LocalSession { flocks, pigeons }` (both `Signal<HashMap<...>>`) as a client-side cache of fetched entities.
- `src/api/*` are the HTTP client functions (one file per resource, e.g. `pigeons.rs`, `flocks.rs`), all going through `fetch_json` in `src/api/helpers.rs` — a thin `web_sys`/`wasm_bindgen_futures` wrapper (not `reqwest`), always `credentials: include` so the Kratos session cookie rides along. API functions return `Option<T>` (fetch/parse failure just collapses to `None`, logged via `tracing::error!`) and, on success, write straight into the relevant `LocalSession` signal — callers re-read from context rather than the function's return value for cached fields.
- Config constants (`src/config.rs`: `KRATOS_BROWSER_URL`, `API_HOST`, `SESSION_COOKIE_NAME`) are baked in at **compile time** via `option_env!`, sourced from `.env.dev`/`.env.release` by `build.rs` (picks the file based on Cargo's `PROFILE`). To change these per-environment, edit the `.env.*` file, not `config.rs`.
- Ory Kratos self-service flows (login/registration/recovery/verification/settings) are driven by `ory-kratos-client-wasm`, rendered generically through `ory_form_builder`/`ory_error` components rather than bespoke forms per flow.
- `dx serve`/`dx build` are configured via `Dioxus.toml`; wrangler serves the built static assets in production (`fancier/wrangler.toml` points `[assets].directory` at the Dioxus release output).
- Most modals are native `<dialog>` elements toggled imperatively via `document::eval(...showModal()/close()...)` (e.g. `UpdatePigeonModal`, `EditShadowModal`). Modals that hold their own reset-sensitive state instead render conditionally from a signal in the parent (`if show_modal() { Modal { ... } }`), so opening always remounts the component fresh — `TokenReveal` (`views/pigeons.rs`, gated by `Signal<Option<String>>`) and `DeletePigeonModal` (`views/pigeon.rs`, gated by `Signal<bool>`, the typed-name-to-confirm delete flow) both use this pattern rather than the native `<dialog>` one, since a stale `<dialog>` left open with prior input wouldn't reset on its own.

## Conventions worth knowing

- 2-space indentation (`rustfmt.toml`), consistent across all crates.
- No `?`-based early return inside the Worker route closures in `dovecote/src/lib.rs` — use explicit `let Ok(x) = ... else { return ... }` so failures always produce a CORS-wrapped error `Response` rather than silently 500ing through the framework's generic error path.
- Every response returned from the top-level router must call `.with_cors(&cors)`, where `cors` is that route's own `build_cors(&ctx.env, &req)` result (see `dovecote` architecture notes above) — there is no shared/global CORS instance to reference.
- DB sync from a Durable Object back to Postgres is always best-effort/fire-and-log (`console_error!`), never blocking or failing the primary request.
