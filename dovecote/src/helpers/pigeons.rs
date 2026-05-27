use capsules::Pigeon;
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

pub async fn sync_pigeon_to_db(client: &Client, pigeon: &Pigeon) -> worker::Result<()> {
  client
    .query_typed(
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
      console_error!("Postgres sync error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  Ok(())
}
