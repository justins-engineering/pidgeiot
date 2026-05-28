use capsules::PigeonCreateResponse;
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

pub async fn sync_pigeon_to_db(
  mut client: Client,
  pcr: &PigeonCreateResponse,
) -> worker::Result<()> {
  let tx = client.transaction().await.map_err(|e| {
    console_error!("Postgres transaction error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  let pigeon = &pcr.pigeon;
  let shadow = &pcr.shadow;
  let acl = &pcr.acl;

  tx.execute_typed(
    "INSERT INTO pigeons (id, flock_id, serial, name, tags, connector, updated_at, created_at)
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
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
      (&pigeon.connector, Type::TEXT),
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
    "INSERT INTO pigeon_shadow (id, status, config, updated_at)
     VALUES ($1, $2, $3::jsonb, $4)
     ON CONFLICT (id) DO UPDATE SET
       status = EXCLUDED.status,
       config = EXCLUDED.config,
       updated_at = EXCLUDED.updated_at;",
    &[
      (&pigeon.id, Type::TEXT),
      (&shadow.status, Type::TEXT),
      (&shadow.config.to_string(), Type::TEXT), // bind as TEXT, cast in SQL
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
