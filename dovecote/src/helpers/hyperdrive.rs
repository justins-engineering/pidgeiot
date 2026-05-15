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
