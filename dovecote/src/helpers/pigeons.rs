use capsules::{Pigeon, PigeonAcl, PigeonDetail, PigeonShadow};
use tokio_postgres::{Client, types::Type};
use worker::{Request, RequestInit, Response, console_error};

pub async fn proxy_to_pigeon_do(
  mut req: Request,
  user_id_str: &str,
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

pub async fn insert_pigeon_pg_db(mut client: Client, pcr: &PigeonDetail) -> worker::Result<()> {
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
    "INSERT INTO pigeons (id, flock_id, serial, name, tags, connector, updated_at, created_at)
     VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8)
     ON CONFLICT (id) DO UPDATE SET
       flock_id = EXCLUDED.flock_id,
       serial = EXCLUDED.serial,
       name = EXCLUDED.name,
       tags = EXCLUDED.tags,
       connector = EXCLUDED.connector,
       updated_at = EXCLUDED.updated_at;",
    &[
      (&pigeon.id, Type::TEXT),
      (&pigeon.flock_id, Type::UUID),
      (&pigeon.serial, Type::TEXT),
      (&pigeon.name, Type::TEXT),
      (&pigeon.tags, Type::TEXT),
      (&connector_json, Type::TEXT),
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

pub async fn update_pigeon_pg_db(client: Client, pigeon: &Pigeon) -> worker::Result<()> {
  let connector_json =
    serde_json::to_string(&pigeon.connector).unwrap_or_else(|_| "{}".to_string());

  client
    .execute_typed(
      "UPDATE pigeons SET
         flock_id = $2,
         serial = $3,
         name = $4,
         tags = $5,
         connector = $6::jsonb,
         updated_at = $7
       WHERE id = $1;",
      &[
        (&pigeon.id, Type::TEXT),
        (&pigeon.flock_id, Type::UUID),
        (&pigeon.serial, Type::TEXT),
        (&pigeon.name, Type::TEXT),
        (&pigeon.tags, Type::TEXT),
        (&connector_json, Type::TEXT),
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

pub async fn upsert_acl_pg_db(
  client: Client,
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
