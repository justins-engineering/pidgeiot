# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

PidgeIoT is an edge-native IoT platform built in Rust, structured as a Cargo workspace with three crates:

- **`dovecote`** (backend): serverless edge router on Cloudflare Workers + Durable Objects. Compiles to a `cdylib` via `worker-build`. Handles device ingestion, provisioning, and session validation.
- **`fancier`** (frontend): WebAssembly SPA built with Dioxus 0.7 + TailwindCSS/DaisyUI. The human-facing dashboard.
- **`capsules`** (shared models): serde structs/RPC schemas shared by both `dovecote` and `fancier` so frontend/backend stay in sync. Keep this crate free of Worker- or Dioxus-specific dependencies.

Auth/identity is handled by a self-hosted Ory Kratos instance (via `docker-compose.yml`), with device-facing durable objects, plus Ed25519-signed JWTs for device auth.

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

- `src/lib.rs` — the single Cloudflare Worker entrypoint (`#[event(fetch, ...)]`). Defines all HTTP routes on a `worker::Router`. Every route handler manually attaches CORS via `.with_cors(&CORS)` and returns explicit error `Response`s (no `?`-propagation through route closures — see git history: "Unroll router let chains to avoid silent failures", "Response::Error() can't fail, unwrap instead of ?"). When adding routes, follow the existing pattern of `let Ok(x) = ... else { return Response::error(...) }` rather than `?`.
- Two local macros in `lib.rs` remove boilerplate: `get_pigeon_do!` (resolves `pigeon_id` → Durable Object stub) and `get_db!` (opens a Postgres client via Hyperdrive, mapping failure to a 500).
- **Dual persistence model**: each `Pigeon` is authoritative in its own Durable Object (SQLite-backed, one row per DO), and is also mirrored into Postgres (via Cloudflare Hyperdrive) for cross-pigeon querying (e.g. listing a flock's pigeons). The DO is the source of truth; every mutating route proxies to the DO first, then best-effort syncs the result to Postgres (`update_pigeon_pg_db`, `insert_pigeon_pg_db`, etc. in `src/helpers/pigeons.rs`) — a Postgres sync failure is logged but does not fail the request.
- `src/objects/pigeons.rs` — the `Pigeons` Durable Object. Owns its own embedded SQLite schema (created in `DurableObject::new`, mirrors `init-db.sql`'s Postgres schema but with SQLite triggers for `updated_at`/immutability/shadow version bumping). Internal routes are dispatched by string path match (`/pigeon/get`, `/pigeon/create`, `/pigeon/shadow/update`, etc.) and are reached only via `proxy_to_pigeon_do` from the top-level router — never exposed directly to the internet.
- Authorization inside the DO is header-based, not cookie-based: the router resolves the Kratos session into a user id and forwards it as `X-User-Id`; `is_authorized`/`is_owner` in `objects/pigeons.rs` check that header against the DO's local `pigeon_acl` table.
- Two distinct auth mechanisms in `src/helpers/auth.rs`:
  - `authenticate_browser` — validates the Kratos session cookie for dashboard users (`fancier` frontend calls).
  - `require_device_auth` — validates an Ed25519-signed JWT (`jwt-simple`) presented by IoT devices themselves, scoped to a specific pigeon ID as the audience claim. Signing key material comes from Worker secrets (`DEVICE_PUBLIC_KEY`), not `wrangler.toml` vars.
- Connectors: a `Pigeon` has one `Connector` (`Https` or `Coap`), each carrying its own endpoint + token/PSK. Tokens are always stripped before returning a `Pigeon` from `GET` routes (`objects/pigeons.rs::get`/`get_detail`) — they're only ever returned on `create` or `token/refresh`.
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

## Conventions worth knowing

- 2-space indentation (`rustfmt.toml`), consistent across all crates.
- No `?`-based early return inside the Worker route closures in `dovecote/src/lib.rs` — use explicit `let Ok(x) = ... else { return ... }` so failures always produce a CORS-wrapped error `Response` rather than silently 500ing through the framework's generic error path.
- Every response returned from the top-level router must call `.with_cors(&CORS)`.
- DB sync from a Durable Object back to Postgres is always best-effort/fire-and-log (`console_error!`), never blocking or failing the primary request.
