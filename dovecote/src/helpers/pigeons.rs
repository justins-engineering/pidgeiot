use capsules::{Connector, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow, TelemetryEndpoint};
use time::OffsetDateTime;
use tokio_postgres::{Client, types::Type};
use worker::{Request, RequestInit, Response, console_error};

use crate::helpers::ensure_pigeons_telemetry_endpoint_column;

/// Proof that this pigeon's ACL check (the DO's `/pigeon/authz/check`
/// route, dispatched to `objects::pigeons::check_authorized`) has already
/// run and passed for the current request. Constructible only via
/// `check_pigeon_authz` below -- a bare `pigeon_id` string can be
/// constructed or forwarded by anyone, but a `PigeonAccess` can only come
/// from code that actually queried `pigeon_acl` and got a passing result.
pub struct PigeonAccess {
  pigeon_id: String,
}

impl PigeonAccess {
  pub fn pigeon_id(&self) -> &str {
    &self.pigeon_id
  }

  /// Alternate proof source for the public, unauthenticated demo routes
  /// (`GET /demo/pigeons/:id/telemetry/history`, `lib.rs`) -- a demo
  /// visitor has no Kratos session, so `check_pigeon_authz`'s `X-User-Id` +
  /// `pigeon_acl` check can never run. The gateway route's own
  /// `helpers::is_demo_pigeon` allowlist check IS the authorization here;
  /// this constructor just forces that check to happen at a call site the
  /// compiler can verify, instead of trusting a bare `&str`.
  pub fn from_demo_allowlist(pigeon_id: &str) -> Self {
    Self {
      pigeon_id: pigeon_id.to_string(),
    }
  }
}

/// Runs the DO's bare ACL probe (`/pigeon/authz/check`) for `pigeon_id` and
/// returns proof of a passing check, or the DO's own error response
/// unchanged for the caller to return as-is. Lets
/// `query_telemetry_history_for_pigeon`, which otherwise trusts
/// `pigeon_id` unconditionally, require this type instead of a bare `&str`
/// -- purely a compile-time guard, not a new check.
pub async fn check_pigeon_authz(
  req: Request,
  user_id_str: &str,
  org_roles_json: Option<&str>,
  stub: &worker::ObjectId<'_>,
  pigeon_id: &str,
) -> worker::Result<std::result::Result<PigeonAccess, Response>> {
  let authz_resp =
    proxy_to_pigeon_do(req, user_id_str, org_roles_json, stub, "/authz/check").await?;
  if authz_resp.status_code() >= 400 {
    return Ok(Err(authz_resp));
  }
  Ok(Ok(PigeonAccess {
    pigeon_id: pigeon_id.to_string(),
  }))
}

/// `org_roles_json` is the caller's org-membership set as compact JSON
/// (`Principal::org_roles_header`, `helpers/orgs.rs`), forwarded as the
/// internal `X-Org-Roles` header so the DO's centralized ACL check
/// (`objects/pigeons.rs::authorize_dashboard`) can match org-granted
/// `pigeon_acl` rows. `None` (device routes, org-less users) adds no
/// header at all.
pub async fn proxy_to_pigeon_do(
  mut req: Request,
  user_id_str: &str,
  org_roles_json: Option<&str>,
  stub: &worker::ObjectId<'_>,
  do_path: &str,
) -> worker::Result<Response> {
  let stub = stub.get_stub().map_err(|e| {
    console_error!("Failed to get DO stub for pigeon {stub}: {e}");
    worker::Error::RustError("Bad Request".into())
  })?;

  let mut init = RequestInit::default();
  init.with_method(req.method().clone());
  init.headers.set("X-User-Id", user_id_str).map_err(|e| {
    console_error!("Failed to set X-User-Id: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  if let Some(org_roles) = org_roles_json {
    init.headers.set("X-Org-Roles", org_roles).map_err(|e| {
      console_error!("Failed to set X-Org-Roles: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;
  }

  // Device-facing routes carry no Kratos session — their Authorization
  // header is the credential the DO itself verifies (see
  // objects::verify_device_token). Forwarding it unconditionally is
  // harmless for user-authenticated DO routes, which never inspect it.
  if let Ok(Some(auth_header)) = req.headers().get("Authorization") {
    init
      .headers
      .set("Authorization", &auth_header)
      .map_err(|e| {
        console_error!("Failed to set Authorization: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;
  }

  // Forward the request body if present
  if req.method() != worker::Method::Get
    && let Ok(body) = req.text().await
  {
    init.body = Some(body.into());
  }

  let do_req = Request::new_with_init(&format!("https://internal/pigeon{do_path}"), &init)
    .map_err(|e| {
      console_error!("Failed to create DO request: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  stub.fetch_with_request(do_req).await
}

/// Bare internal GET against one pigeon's DO for the CoAP terminator's
/// PSK lookup (`/pigeon/internal/psk`). Deliberately NOT
/// `proxy_to_pigeon_do`: that forwards the caller's `Authorization` header
/// into the DO, which here would smuggle the terminator's service secret a
/// hop further than it needs to travel. No `X-User-Id`, no body -- the
/// gateway route has already authenticated the terminator before calling
/// this (same "gateway fully authorized first" trust argument as
/// `grant_org_acl_via_do` below).
pub async fn psk_lookup_via_do(stub: &worker::ObjectId<'_>) -> worker::Result<Response> {
  let stub = stub.get_stub().map_err(|e| {
    console_error!("Failed to get DO stub for pigeon {stub}: {e}");
    worker::Error::RustError("Bad Request".into())
  })?;

  let init = RequestInit::default();
  let do_req =
    Request::new_with_init("https://internal/pigeon/internal/psk", &init).map_err(|e| {
      console_error!("Failed to create DO request: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  stub.fetch_with_request(do_req).await
}

/// Writes an ORG-granted `pigeon_acl` row (`entity_id` = the org id, role
/// `owner` -- each member's effective rights are then derived from their
/// own role in that org, see `objects/pigeons.rs::authorize_dashboard`)
/// into one pigeon's Durable Object via the trusted-internal
/// `/pigeon/acl/grant` route. Dispatched by the flock-transfer route and
/// by pigeon-create inside an org-owned flock, both AFTER the gateway has
/// fully authorized the operation -- DOs are never internet-reachable, so
/// only this Worker can dispatch here.
///
/// NOT best-effort: callers treat a non-2xx here as a hard failure of the
/// primary operation -- the DO's ACL table is the authoritative
/// authorization store, unlike its best-effort Postgres mirror.
pub async fn grant_org_acl_via_do(
  stub: &worker::ObjectId<'_>,
  org_id: &str,
) -> worker::Result<Response> {
  let stub = stub.get_stub().map_err(|e| {
    console_error!("Failed to get DO stub for pigeon {stub}: {e}");
    worker::Error::RustError("Bad Request".into())
  })?;

  let body = serde_json::json!({ "entity_id": org_id, "role": "owner" });

  let mut init = RequestInit::default();
  init.with_method(worker::Method::Post);
  init.body = Some(body.to_string().into());

  let do_req = Request::new_with_init("https://internal/pigeon/acl/grant", &init).map_err(|e| {
    console_error!("Failed to create DO request: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  stub.fetch_with_request(do_req).await
}

/// Binary-safe counterpart to `proxy_to_pigeon_do`, used only by `POST
/// /device/pigeons/:id/logs`. `proxy_to_pigeon_do` forwards the body via
/// `req.text()`, which is fine for JSON bodies but silently mangles
/// non-UTF-8 bytes -- device dictionary-log chunks are arbitrary binary.
/// Otherwise identical (`Authorization` header forwarding, no `X-User-Id`
/// -- this is a device-facing route, see `is_authorized_device` in
/// `objects/pigeons.rs`).
pub async fn proxy_binary_to_pigeon_do(
  mut req: Request,
  stub: &worker::ObjectId<'_>,
  do_path: &str,
) -> worker::Result<Response> {
  let stub = stub.get_stub().map_err(|e| {
    console_error!("Failed to get DO stub for pigeon {stub}: {e}");
    worker::Error::RustError("Bad Request".into())
  })?;

  let mut init = RequestInit::default();
  init.with_method(req.method().clone());

  if let Ok(Some(auth_header)) = req.headers().get("Authorization") {
    init
      .headers
      .set("Authorization", &auth_header)
      .map_err(|e| {
        console_error!("Failed to set Authorization: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;
  }

  if req.method() != worker::Method::Get {
    let bytes = req.bytes().await.map_err(|e| {
      console_error!("Failed to read binary request body: {e}");
      worker::Error::RustError("Bad Request".into())
    })?;
    init.body = Some(js_sys::Uint8Array::from(bytes.as_slice()).into());
  }

  let do_req = Request::new_with_init(&format!("https://internal/pigeon{do_path}"), &init)
    .map_err(|e| {
      console_error!("Failed to create DO request: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  stub.fetch_with_request(do_req).await
}

/// WebSocket-upgrade counterpart to `proxy_to_pigeon_do`, used only by
/// `GET /device/pigeons/:id/ws`. GET, so no body to forward. No
/// `X-User-Id` -- same device-auth model as the other
/// `/device/pigeons/:id/*` routes; the DO verifies the bearer token itself
/// BEFORE accepting the socket (see
/// `is_authorized_device`/`accept_websocket_device` in
/// `objects/pigeons.rs`). The actual protocol upgrade is driven by the
/// `Response` the DO returns (`Response::from_websocket`, carrying the
/// `webSocket` field), not by which headers reach this internal
/// `Stub::fetch_with_request` dispatch -- but the handshake headers are
/// forwarded anyway, for parity with a real HTTP proxy.
pub async fn proxy_websocket_to_pigeon_do(
  req: Request,
  stub: &worker::ObjectId<'_>,
  do_path: &str,
) -> worker::Result<Response> {
  let stub = stub.get_stub().map_err(|e| {
    console_error!("Failed to get DO stub for pigeon {stub}: {e}");
    worker::Error::RustError("Bad Request".into())
  })?;

  let mut init = RequestInit::default();
  init.with_method(worker::Method::Get);

  for header in [
    "Authorization",
    "Upgrade",
    "Connection",
    "Sec-WebSocket-Key",
    "Sec-WebSocket-Version",
    "Sec-WebSocket-Protocol",
  ] {
    if let Ok(Some(value)) = req.headers().get(header) {
      init.headers.set(header, &value).map_err(|e| {
        console_error!("Failed to forward header {header}: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;
    }
  }

  let do_req = Request::new_with_init(&format!("https://internal/pigeon{do_path}"), &init)
    .map_err(|e| {
      console_error!("Failed to create DO request: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  stub.fetch_with_request(do_req).await
}

/// Lightweight counterpart to `proxy_to_pigeon_do`, used only by the
/// telemetry queue producer path (`POST /device/pigeons/:id/telemetry` in
/// `lib.rs`, when a telemetry queue is bound for this environment) to check
/// a device's bearer token against its owning DO *before* enqueueing
/// anything. Forwards just the `Authorization` header (no body, no
/// `X-User-Id`) to `do_path` and returns the DO's raw response so the
/// caller can inspect its status code.
pub async fn verify_device_via_do(
  auth_header: Option<String>,
  stub: &worker::ObjectId<'_>,
  do_path: &str,
) -> worker::Result<Response> {
  let stub = stub.get_stub().map_err(|e| {
    console_error!("Failed to get DO stub for pigeon {stub}: {e}");
    worker::Error::RustError("Bad Request".into())
  })?;

  let mut init = RequestInit::default();
  init.with_method(worker::Method::Post);
  if let Some(auth) = auth_header {
    init.headers.set("Authorization", &auth).map_err(|e| {
      console_error!("Failed to set Authorization: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;
  }

  let do_req = Request::new_with_init(&format!("https://internal/pigeon{do_path}"), &init)
    .map_err(|e| {
      console_error!("Failed to create DO request: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  stub.fetch_with_request(do_req).await
}

/// Idempotently ensures the `pigeons.board` column exists on the Postgres
/// mirror table -- same no-separate-migration-runner rationale as
/// `ensure_pigeons_telemetry_endpoint_column` (`helpers/telemetry.rs`).
/// Unlike that column, `board` is written unconditionally on every
/// `insert_pigeon_pg_db`/`update_pigeon_pg_db` call (not just a dedicated
/// opt-in route), so both must call this first or the write 500s with
/// "column does not exist" against an already-deployed database.
pub async fn ensure_pigeons_board_column(client: &Client) -> worker::Result<()> {
  client
    .batch_execute("ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS board TEXT;")
    .await
    .map_err(|e| {
      console_error!("pigeons.board column bootstrap error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })
}

/// Idempotently ensures the `pigeons.suspended_at` column exists on the
/// Postgres mirror table -- same no-migration-runner rationale as
/// `ensure_pigeons_board_column` above.
pub async fn ensure_pigeons_suspended_column(client: &Client) -> worker::Result<()> {
  client
    .batch_execute("ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS suspended_at TIMESTAMPTZ;")
    .await
    .map_err(|e| {
      console_error!("pigeons.suspended_at column bootstrap error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })
}

pub async fn insert_pigeon_pg_db(mut client: Client, pcr: &PigeonDetail) -> worker::Result<()> {
  ensure_pigeons_board_column(&client).await?;

  let tx = client.transaction().await.map_err(|e| {
    console_error!("Postgres transaction error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  let pigeon = &pcr.pigeon;
  let shadow = &pcr.shadow;
  let acl = &pcr.acl;

  let connector_json =
    serde_json::to_string(&pigeon.connector).unwrap_or_else(|_| "{}".to_string());

  tx.execute_typed(
    "INSERT INTO pigeons (id, flock_id, serial, name, tags, connector, board, updated_at, created_at)
     VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9)
     ON CONFLICT (id) DO UPDATE SET
       flock_id = EXCLUDED.flock_id,
       serial = EXCLUDED.serial,
       name = EXCLUDED.name,
       tags = EXCLUDED.tags,
       connector = EXCLUDED.connector,
       board = EXCLUDED.board,
       updated_at = EXCLUDED.updated_at;",
    &[
      (&pigeon.id, Type::TEXT),
      (&pigeon.flock_id, Type::UUID),
      (&pigeon.serial, Type::TEXT),
      (&pigeon.name, Type::TEXT),
      (&pigeon.tags, Type::TEXT),
      (&connector_json, Type::TEXT),
      (&pigeon.board, Type::TEXT),
      (&pigeon.updated_at, Type::TIMESTAMPTZ),
      (&pigeon.created_at, Type::TIMESTAMPTZ),
    ],
  )
  .await
  .map_err(|e| {
    console_error!("Postgres pigeons sync error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  tx.execute_typed(
    "INSERT INTO pigeon_acl (id, entity_id, role)
     VALUES ($1, $2, $3)
     ON CONFLICT (id, entity_id) DO UPDATE SET
       role = EXCLUDED.role;",
    &[
      (&pigeon.id, Type::TEXT),
      (&acl.entity_id, Type::UUID),
      (&acl.role, Type::TEXT),
    ],
  )
  .await
  .map_err(|e| {
    console_error!("Postgres pigeon_acl sync error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  tx.execute_typed(
    "INSERT INTO pigeon_shadow (id, target_version, current_version, target_config, current_config, updated_at)
     VALUES ($1, $2, $3, $4::jsonb, $5::jsonb, $6)
     ON CONFLICT (id) DO UPDATE SET
       target_version = EXCLUDED.target_version,
       current_version = EXCLUDED.current_version,
       target_config = EXCLUDED.target_config,
       current_config = EXCLUDED.current_config,
       updated_at = EXCLUDED.updated_at;",
    &[
      (&pigeon.id, Type::TEXT),
      (&shadow.target_version, Type::INT4),
      (&shadow.current_version, Type::INT4),
      (&shadow.target_config.to_string(), Type::TEXT),
      (&shadow.current_config.to_string(), Type::TEXT),
      (&shadow.updated_at, Type::INT8),
    ],
  )
  .await
  .map_err(|e| {
    console_error!("Postgres pigeon_shadow sync error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  tx.commit().await.map_err(|e| {
    console_error!("Postgres commit error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  Ok(())
}

/// Mirrors a pigeon's row into Postgres. `connector` is taken separately
/// rather than from `pigeon` because only the routes that mint credentials
/// hold one worth mirroring -- everything else answers with a
/// secret-stripped pigeon, and passing `None` leaves the column alone
/// instead of blanking the mirror's copy of a live device's PSK.
pub async fn update_pigeon_pg_db(
  client: Client,
  pigeon: &Pigeon,
  connector: Option<&Connector>,
) -> worker::Result<()> {
  ensure_pigeons_board_column(&client).await?;

  let connector_json =
    connector.map(|c| serde_json::to_string(c).unwrap_or_else(|_| "{}".to_string()));

  client
    .execute_typed(
      "UPDATE pigeons SET
         flock_id = $2,
         serial = $3,
         name = $4,
         tags = $5,
         connector = COALESCE($6::jsonb, connector),
         board = $7,
         updated_at = $8
       WHERE id = $1;",
      &[
        (&pigeon.id, Type::TEXT),
        (&pigeon.flock_id, Type::UUID),
        (&pigeon.serial, Type::TEXT),
        (&pigeon.name, Type::TEXT),
        (&pigeon.tags, Type::TEXT),
        (&connector_json, Type::TEXT),
        (&pigeon.board, Type::TEXT),
        (&pigeon.updated_at, Type::TIMESTAMPTZ),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Postgres pigeon update sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}

pub async fn update_shadow_pg_db(
  client: Client,
  pigeon_id: &str,
  shadow: &PigeonShadow,
) -> worker::Result<()> {
  client
    .execute_typed(
      "UPDATE pigeon_shadow SET
         target_version = $2,
         current_version = $3,
         target_config = $4::jsonb,
         current_config = $5::jsonb,
         updated_at = $6
       WHERE id = $1;",
      &[
        (&pigeon_id, Type::TEXT),
        (&shadow.target_version, Type::INT4),
        (&shadow.current_version, Type::INT4),
        (&shadow.target_config.to_string(), Type::TEXT),
        (&shadow.current_config.to_string(), Type::TEXT),
        (&shadow.updated_at, Type::INT8),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Postgres pigeon_shadow update sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}

/// Best-effort PG sync for the dedicated `PUT
/// /pigeons/:pigeon_id/telemetry-endpoint` route -- mirrors
/// `update_shadow_pg_db`'s shape (single-column update, called after the
/// DO's own write already succeeded). Calls
/// `ensure_pigeons_telemetry_endpoint_column` first since no environment
/// has a separate migration runner (see `helpers/telemetry.rs`).
pub async fn update_telemetry_endpoint_pg_db(
  client: Client,
  pigeon_id: &str,
  telemetry_endpoint: Option<&TelemetryEndpoint>,
) -> worker::Result<()> {
  ensure_pigeons_telemetry_endpoint_column(&client).await?;

  let endpoint_json = telemetry_endpoint.map(|e| serde_json::to_string(e).unwrap_or_default());

  client
    .execute_typed(
      "UPDATE pigeons SET telemetry_endpoint = $2::jsonb WHERE id = $1;",
      &[(&pigeon_id, Type::TEXT), (&endpoint_json, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Postgres telemetry_endpoint update sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}

/// PG sync for `PUT /pigeons/:pigeon_id/suspension`, single-column like
/// `update_telemetry_endpoint_pg_db`. Unlike the other mirrors this write
/// is load-bearing: both alert paths (`helpers/alerts.rs`) read the
/// Postgres column, so a failed sync leaves alerts evaluating a pigeon the
/// dashboard shows as suspended. The stamp is the DO's own, read back from
/// its row, rather than a second `now()`. The general update routes never
/// touch this column, so they cannot blank it.
pub async fn update_pigeon_suspension_pg_db(
  client: Client,
  pigeon_id: &str,
  suspended_at: Option<OffsetDateTime>,
) -> worker::Result<()> {
  ensure_pigeons_suspended_column(&client).await?;

  client
    .execute_typed(
      "UPDATE pigeons SET suspended_at = $2 WHERE id = $1;",
      &[(&pigeon_id, Type::TEXT), (&suspended_at, Type::TIMESTAMPTZ)],
    )
    .await
    .map_err(|e| {
      console_error!("Postgres suspended_at update sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}

pub async fn upsert_acl_pg_db(
  client: &Client,
  pigeon_id: &str,
  acl: &PigeonAcl,
) -> worker::Result<()> {
  client
    .execute_typed(
      "INSERT INTO pigeon_acl (id, entity_id, role)
       VALUES ($1, $2, $3)
       ON CONFLICT (id, entity_id) DO UPDATE SET
         role = EXCLUDED.role;",
      &[
        (&pigeon_id, Type::TEXT),
        (&acl.entity_id, Type::UUID),
        (&acl.role, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Postgres pigeon_acl upsert sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}

pub async fn delete_pigeon_pg_db(client: Client, pigeon_id: &str) -> worker::Result<()> {
  // CASCADE on the PG tables handles pigeon_acl and pigeon_shadow
  client
    .execute_typed(
      "DELETE FROM pigeons WHERE id = $1;",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Postgres pigeon delete sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}
