use worker::{Env, Result, SecureTransport, Socket, console_error, postgres_tls::PassthroughTls};

pub async fn get_hyperdrive_conn(
  env: &Env,
) -> Result<(
  tokio_postgres::Client,
  tokio_postgres::Connection<Socket, Socket>,
)> {
  let map_err = |e: String| -> worker::Error {
    console_error!("Failed to connect to hyperdrive, Error: {e}");
    // We construct a worker::Error representing the HTTP 500 response
    worker::Error::RustError("Internal Server Error".to_string())
  };

  let hyperdrive = env
    .hyperdrive("YugabyteDB")
    .map_err(|e| map_err(e.to_string()))?;

  let socket = Socket::builder()
    .secure_transport(SecureTransport::StartTls)
    .connect(hyperdrive.host(), hyperdrive.port())
    .map_err(|e| map_err(e.to_string()))?;

  let config = hyperdrive
    .connection_string()
    .parse::<tokio_postgres::Config>()
    .map_err(|e| map_err(e.to_string()))?;

  let (client, connection) = config
    .connect_raw(socket, PassthroughTls)
    .await
    .map_err(|e| map_err(e.to_string()))?;

  Ok((client, connection))
}

/// Establishes a Hyperdrive connection, spawns the background driver, and hands back a ready-to-use Client.
pub async fn get_db_client(env: &Env) -> worker::Result<tokio_postgres::Client> {
  let (client, connection) = crate::get_hyperdrive_conn(env).await?;

  // Abstract the Wasm background task away from the route handlers
  worker::wasm_bindgen_futures::spawn_local(async move {
    if let Err(e) = connection.await {
      console_error!("Postgres connection error: {}", e);
    }
  });

  Ok(client)
}
