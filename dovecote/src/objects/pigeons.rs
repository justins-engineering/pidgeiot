use crate::objects::ws::{
  MAX_WS_FRAME_BYTES, WS_DEVICE_TAG, WsInboundFrame, WsOutboundFrame, check_rate_limit,
};
use crate::objects::{mint_device_credential, verify_device_token};
use crate::queue::TelemetryMessage;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use capsules::{
  CoapConfig, Connector, FirmwareTarget, HttpsConfig, MAX_LOG_CHUNK_BYTES, Pigeon, PigeonAcl,
  PigeonAclUpdateRequest, PigeonCreateRequest, PigeonDetail, PigeonLogChunk, PigeonLogChunkRow,
  PigeonRow, PigeonShadow, PigeonShadowReportRequest, PigeonShadowRow, PigeonShadowUpdateRequest,
  PigeonUpdateRequest, TelemetryEndpoint, TelemetryLatest, TelemetryLatestRow,
  unwrap_or_return_response,
};
use worker::{
  Date, DurableObject, Env, Request, Response, ResponseBuilder, Result, SqlStorage, State,
  WebSocket, WebSocketIncomingMessage, WebSocketPair, console_error, console_log, durable_object,
  wasm_bindgen,
};

/// Selected pigeon column list shared by every `pigeons` read/RETURNING
/// statement -- keeps `telemetry_endpoint` (and any future column) from
/// silently going missing from one of the several near-identical queries.
const PIGEON_COLUMNS: &str = "id, flock_id, serial, name, tags, connector, token_expires_at, telemetry_endpoint, updated_at, created_at";

// Falls back to the production host if `DEVICE_API_HOST` isn't set for some
// reason -- every environment ([vars]/[env.staging.vars]/[env.dev.vars] in
// wrangler.toml) sets it explicitly today, but a missing binding should
// degrade to prod's own host rather than emit a garbage endpoint.
const DEFAULT_DEVICE_API_HOST: &str = "api.pidgeiot.com";
const DEVICE_PIGEONS_PATH: &str = "/device/pigeons/";

/// The host a minted device endpoint should point at -- deliberately NOT
/// `ROOT_URL` (that's the frontend's own origin, e.g. pidgeiot.com, not the
/// device-facing API host, and the two differ per environment: prod shares
/// a domain across both, staging and dev do not). Read fresh per call
/// instead of cached, since Durable Objects can outlive a single Worker
/// invocation and `env` is cheap to read.
fn device_api_host(env: &Env) -> String {
  env
    .var("DEVICE_API_HOST")
    .map(|v| v.to_string())
    .unwrap_or_else(|_| DEFAULT_DEVICE_API_HOST.to_string())
}

#[inline]
pub fn build_http_endpoint(env: &Env, do_id: &str) -> String {
  let host = device_api_host(env);
  let mut endpoint = String::with_capacity(8 + host.len() + DEVICE_PIGEONS_PATH.len() + 64);
  endpoint.push_str("https://");
  endpoint.push_str(&host);
  endpoint.push_str(DEVICE_PIGEONS_PATH);
  endpoint.push_str(do_id);
  endpoint
}

#[inline]
pub fn build_coap_endpoint(env: &Env, do_id: &str) -> String {
  let host = device_api_host(env);
  let mut endpoint = String::with_capacity(12 + host.len() + DEVICE_PIGEONS_PATH.len() + 64);
  endpoint.push_str("coaps+tcp://");
  endpoint.push_str(&host);
  endpoint.push_str(DEVICE_PIGEONS_PATH);
  endpoint.push_str(do_id);
  endpoint
}

#[durable_object]
pub struct Pigeons {
  sql: SqlStorage,
  state: State,
  env: Env,
}

#[derive(serde::Deserialize)]
struct ExistsResult {
  exists_flag: i64,
}

/// `SqlCursor::one()` throws an uncaught JS exception (crashing the DO)
/// on zero rows instead of returning a catchable `Result::Err` —
/// `to_array()` never throws, so route "no rows" through the same
/// catchable-error path callers already use for real query/deserialization
/// failures. This matters once `delete()` can legitimately leave this DO's
/// tables empty while the DO itself is still addressable.
fn one_row<T: serde::de::DeserializeOwned>(cursor: &worker::SqlCursor) -> Result<T> {
  match cursor.to_array::<T>()?.into_iter().next() {
    Some(row) => Ok(row),
    None => Err(worker::Error::RustError("Pigeon not found".into())),
  }
}

impl DurableObject for Pigeons {
  fn new(state: State, env: Env) -> Pigeons {
    let sql = state.storage().sql();
    sql
      .exec("PRAGMA foreign_keys = ON;", None)
      .expect("enabled foreign keys");

    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeons (
          id TEXT NOT NULL PRIMARY KEY,
          flock_id TEXT NOT NULL,
          serial TEXT,
          name TEXT,
          tags TEXT,
          connector TEXT NOT NULL,
          token_expires_at INTEGER DEFAULT 0,
          device_public_key TEXT NOT NULL DEFAULT '',
          telemetry_endpoint TEXT DEFAULT NULL,
          updated_at INTEGER DEFAULT (unixepoch()),
          created_at INTEGER DEFAULT (unixepoch())
        );


        CREATE TRIGGER IF NOT EXISTS prevent_immutable_updates_on_pigeons
        BEFORE UPDATE OF id, created_at ON pigeons
        WHEN OLD.id IS NOT NEW.id
          OR OLD.created_at IS NOT NEW.created_at
        BEGIN
          SELECT RAISE(ABORT, 'Error: id and created_at columns are immutable.');
        END;

        CREATE TRIGGER IF NOT EXISTS set_updated_at
        AFTER UPDATE ON pigeons
        FOR EACH ROW
        WHEN NEW.updated_at = OLD.updated_at
        BEGIN
          UPDATE pigeons SET updated_at = unixepoch() WHERE id = OLD.id;
        END;",
        None,
      )
      .expect("created pigeons table");

    // Column migration for DOs created before `telemetry_endpoint` existed
    // (task #18) -- `CREATE TABLE IF NOT EXISTS` above is a no-op against
    // an already-existing `pigeons` table, and SQLite has no `ADD COLUMN
    // IF NOT EXISTS`. Errors here (column already present, e.g. every DO
    // created after this change) are expected and ignored on purpose --
    // unlike every other statement in this constructor, this one is
    // allowed to fail.
    let _ = sql.exec(
      "ALTER TABLE pigeons ADD COLUMN telemetry_endpoint TEXT DEFAULT NULL;",
      None,
    );

    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeon_shadow (
          id TEXT PRIMARY KEY REFERENCES pigeons(id) ON DELETE CASCADE,
          target_version INTEGER DEFAULT 0,
          current_version INTEGER DEFAULT 0,
          target_config TEXT DEFAULT '{}',
          current_config TEXT DEFAULT '{}',
          updated_at INTEGER DEFAULT (unixepoch())
        );

        CREATE TRIGGER IF NOT EXISTS increment_pigeon_target_version
        AFTER UPDATE OF target_config ON pigeon_shadow
        FOR EACH ROW
        WHEN NEW.target_config IS NOT OLD.target_config
        BEGIN
          UPDATE pigeon_shadow
          SET target_version = OLD.target_version + 1
          WHERE id = OLD.id;
        END;

        CREATE TRIGGER IF NOT EXISTS set_shadow_updated_at
        AFTER UPDATE ON pigeon_shadow
        FOR EACH ROW
        WHEN NEW.updated_at = OLD.updated_at
        BEGIN
          UPDATE pigeon_shadow SET updated_at = unixepoch() WHERE id = OLD.id;
        END;",
        None,
      )
      .expect("created pigeon_shadow table");

    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeon_acl (
          entity_id TEXT PRIMARY KEY NOT NULL,
          role TEXT NOT NULL
        );",
        None,
      )
      .expect("created pigeon_acl table");

    // Latest-value-per-key store (mirrors AWS IoT's reported-state
    // simplicity, not a time-series log) -- a device's telemetry report
    // overwrites, it doesn't append. History/range queries are served from
    // Postgres's `pigeon_telemetry_history` instead (task #18, written by
    // the queue consumer) -- this DO-local table intentionally stays
    // latest-value-only.
    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeon_telemetry (
          key TEXT PRIMARY KEY NOT NULL,
          value TEXT NOT NULL,
          reported_at INTEGER DEFAULT (unixepoch())
        );",
        None,
      )
      .expect("created pigeon_telemetry table");

    // Bounded ring buffer of device dictionary-log chunks (task #18, part
    // 3) -- opaque binary blobs stored as base64 text (see
    // `report_logs_device`'s doc comment for why not a BLOB column). `id`
    // is a plain autoincrement so pruning can cheaply keep "the newest N
    // rows" without a separate ordering column.
    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeon_log_chunks (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          data TEXT NOT NULL,
          received_at INTEGER DEFAULT (unixepoch())
        );",
        None,
      )
      .expect("created pigeon_log_chunks table");

    Pigeons { sql, state, env }
  }

  async fn fetch(&self, req: Request) -> Result<Response> {
    // Use path parsing that ignores potential trailing slashes for robustness
    let path = req.path();

    match path.as_str() {
      "/pigeon/get" => get(self, req).await,
      "/pigeon/detail" => get_detail(self, req).await,
      "/pigeon/create" => create(self, req).await,
      "/pigeon/update" => update(self, req).await,
      "/pigeon/acl/get" => get_acl(self, req).await,
      "/pigeon/acl/list" => list_acl(self, req).await,
      "/pigeon/acl/update" => update_acl(self, req).await,
      "/pigeon/shadow/get" => get_shadow(self, req).await,
      "/pigeon/device/shadow" => get_shadow_device(self, req).await,
      "/pigeon/device/shadow/report" => report_shadow_device(self, req).await,
      "/pigeon/device/ws" => accept_websocket_device(self, req).await,
      "/pigeon/device/firmware/target" => get_firmware_target_device(self, req).await,
      "/pigeon/device/telemetry" => report_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/verify" => verify_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/write" => write_telemetry_device(self, req).await,
      "/pigeon/telemetry/get" => get_telemetry_latest(self, req).await,
      "/pigeon/telemetry-endpoint/update" => update_telemetry_endpoint(self, req).await,
      "/pigeon/authz/check" => check_authorized(self, req).await,
      "/pigeon/device/logs" => report_logs_device(self, req).await,
      "/pigeon/logs/get" => get_logs(self, req).await,
      "/pigeon/shadow/update" => update_shadow(self, req).await,
      "/pigeon/token/refresh" => refresh_token(self, req).await,
      "/pigeon/delete" => delete(self, req).await,
      _ => Response::error("Not Found", 404),
    }
  }

  /// Dispatched by the runtime for every text/binary frame on a
  /// hibernation-accepted socket (task #32) -- including ones that woke this
  /// DO from eviction, transparently. Auth already happened once, at
  /// `accept_websocket_device`, and isn't re-checked per frame; a device
  /// controls its own connection, so anything landing here is trusted to
  /// belong to this pigeon. Three ways a frame gets the connection closed
  /// rather than processed: not text (this protocol is JSON-text-only, see
  /// `docs/api.md`), over `MAX_WS_FRAME_BYTES`, or failing the sliding-window
  /// flood check (`check_rate_limit`) -- all three are logged before
  /// closing, matching this file's existing "log, then respond" convention.
  async fn websocket_message(
    &self,
    ws: WebSocket,
    message: WebSocketIncomingMessage,
  ) -> Result<()> {
    let WebSocketIncomingMessage::String(text) = message else {
      console_error!("WS: binary frame from pigeon {}, closing", self.state.id());
      let _ = ws.close(Some(4001), Some("binary frames not supported"));
      return Ok(());
    };

    if text.len() > MAX_WS_FRAME_BYTES {
      console_error!(
        "WS: oversize frame ({} bytes) from pigeon {}, closing",
        text.len(),
        self.state.id()
      );
      let _ = ws.close(Some(4002), Some("frame too large"));
      return Ok(());
    }

    if !check_rate_limit(&ws) {
      console_error!("WS: frame flood from pigeon {}, closing", self.state.id());
      let _ = ws.close(Some(4008), Some("rate limit exceeded"));
      return Ok(());
    }

    let frame = match serde_json::from_str::<WsInboundFrame>(&text) {
      Ok(f) => f,
      Err(e) => {
        console_error!("WS: malformed frame from pigeon {}: {e}", self.state.id());
        let _ = ws.close(Some(4003), Some("malformed frame"));
        return Ok(());
      }
    };

    match frame {
      WsInboundFrame::Telemetry { metrics } => handle_ws_telemetry(self, metrics).await,
      WsInboundFrame::ShadowReport {
        current_version,
        current_config,
      } => {
        handle_ws_shadow_report(
          self,
          &PigeonShadowReportRequest {
            current_version,
            current_config,
          },
        )
        .await
      }
      WsInboundFrame::Ping => {
        if let Err(e) = ws.send(&WsOutboundFrame::Pong) {
          console_error!("WS: pong send failed for pigeon {}: {e}", self.state.id());
        }
      }
      WsInboundFrame::Pong => {}
    }

    Ok(())
  }

  /// The runtime calls this once a hibernatable socket actually closes
  /// (client-initiated, our own `ws.close()` calls elsewhere in this file,
  /// or a network drop) -- overriding the default (which panics via
  /// `unimplemented!()`) is required, not optional, once any socket is ever
  /// accepted here.
  async fn websocket_close(
    &self,
    _ws: WebSocket,
    code: usize,
    reason: String,
    was_clean: bool,
  ) -> Result<()> {
    console_log!(
      "WS closed for pigeon {}: code={code} reason={reason} clean={was_clean}",
      self.state.id()
    );
    Ok(())
  }

  /// Same rationale as `websocket_close` above -- must be overridden once
  /// sockets are accepted, or a transport-level error panics the DO instead
  /// of just logging.
  async fn websocket_error(&self, _ws: WebSocket, error: worker::Error) -> Result<()> {
    console_error!("WS error for pigeon {}: {error}", self.state.id());
    Ok(())
  }
}

fn is_authorized(pigeons: &Pigeons, req: &Request) -> Result<(), Result<Response, worker::Error>> {
  let Ok(Some(requesting_user)) = req.headers().get("X-User-Id") else {
    return Err(Response::error("Request missing 'X-User-Id'", 400));
  };

  let result = pigeons
    .sql
    .exec(
      "SELECT EXISTS(SELECT 1 FROM pigeon_acl WHERE entity_id = ?1) AS exists_flag",
      vec![requesting_user.into()],
    )
    .map_err(Err)?
    .one::<ExistsResult>()
    .map_err(Err)?;

  if result.exists_flag != 0 {
    Ok(())
  } else {
    Err(Response::error(
      "Forbidden: You do not have access to this Pigeon",
      403,
    ))
  }
}

fn is_owner(pigeons: &Pigeons, req: &Request) -> Result<(), Result<Response, worker::Error>> {
  let Ok(Some(requesting_user)) = req.headers().get("X-User-Id") else {
    return Err(Response::error("Request missing 'X-User-Id'", 400));
  };

  let result = pigeons
  .sql
  .exec(
    "SELECT EXISTS(SELECT 1 FROM pigeon_acl WHERE entity_id = ?1 AND role = 'owner') AS exists_flag",
    vec![requesting_user.into()],
  )
  .map_err(Err)?
  .one::<ExistsResult>()
  .map_err(Err)?;

  if result.exists_flag != 0 {
    Ok(())
  } else {
    Err(Response::error(
      "Forbidden: Only owners can modify ACL",
      403,
    ))
  }
}

/// Strips every secret from a `Pigeon` before it leaves the DO via a GET
/// route -- the connector token/PSK (existing behavior) and now the
/// telemetry endpoint's `auth_token` (task #18), same rule: never returned
/// except immediately after the request that sets it (`create`/
/// `token/refresh` for the connector, the telemetry-endpoint update route
/// for `auth_token`).
fn strip_secrets(pigeon: &mut Pigeon) {
  pigeon.connector = match pigeon.connector.clone() {
    Connector::Https(c) => Connector::Https(HttpsConfig {
      endpoint: c.endpoint,
      token: String::new(),
    }),
    Connector::Coap(c) => Connector::Coap(CoapConfig {
      endpoint: c.endpoint,
      token: String::new(),
      tls_psk_identity: c.tls_psk_identity,
      tls_psk_secret: None,
    }),
  };

  if let Some(endpoint) = pigeon.telemetry_endpoint.as_mut() {
    endpoint.auth_token = None;
  }
}

async fn get(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  match pigeons.sql.exec(
    &format!("SELECT {PIGEON_COLUMNS} FROM pigeons LIMIT 1;"),
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
      Ok(p) => {
        let mut pigeon = Pigeon::from(p);
        strip_secrets(&mut pigeon);
        Response::from_json(&pigeon)
      }
      Err(e) => {
        console_error!("Pigeon deserialization error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("Pigeons READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn get_detail(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  let mut pigeon = match pigeons.sql.exec(
    &format!("SELECT {PIGEON_COLUMNS} FROM pigeons LIMIT 1;"),
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
      Ok(p) => Pigeon::from(p),
      Err(e) => {
        console_error!("Pigeon deserialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("Pigeons READ error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  strip_secrets(&mut pigeon);

  let shadow = match pigeons.sql.exec(
    "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonShadowRow>(&cursor) {
      Ok(s) => PigeonShadow::from(s),
      Err(e) => {
        console_error!("PigeonShadow deserialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("PigeonShadow READ error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  let Ok(Some(requesting_user)) = req.headers().get("X-User-Id") else {
    return Response::error("Request missing 'X-User-Id'", 400);
  };

  let acl = match pigeons.sql.exec(
    "SELECT entity_id, role FROM pigeon_acl WHERE entity_id = ?;",
    vec![requesting_user.into()],
  ) {
    Ok(cursor) => match one_row::<PigeonAcl>(&cursor) {
      Ok(a) => a,
      Err(e) => {
        console_error!("PigeonAcl deserialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("PigeonAcl READ error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  Response::from_json(&PigeonDetail {
    pigeon,
    shadow,
    acl,
  })
}

async fn create(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  let Ok(Some(user_id)) = req.headers().get("X-User-Id") else {
    return Response::error("Request missing 'X-User-Id'", 400);
  };

  let user_uuid = uuid::Uuid::parse_str(&user_id).map_err(|e| {
    console_error!("Invalid X-User-Id format: {e}");
    worker::Error::RustError("Bad Request: Invalid X-User-Id format".into())
  })?;

  let row = match req.json::<PigeonCreateRequest>().await {
    Ok(data) => data,
    Err(e) => {
      console_error!("Pigeons CREATE json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  let do_id = pigeons.state.id().to_string();

  let (public_key, device_token, expires_at) = match mint_device_credential() {
    Ok(t) => t,
    Err(e) => {
      console_error!("Device credential mint error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  let server_connector = match row.connector {
    Connector::Https(_) => Connector::Https(HttpsConfig {
      endpoint: build_http_endpoint(&pigeons.env, &do_id),
      token: device_token,
    }),
    Connector::Coap(_) => Connector::Coap(CoapConfig {
      endpoint: build_coap_endpoint(&pigeons.env, &do_id),
      token: device_token.clone(),
      tls_psk_identity: Some(do_id.clone()),
      tls_psk_secret: Some(device_token),
    }),
  };

  let connector_json = serde_json::to_string(&server_connector).unwrap_or_default();

  let pigeon = match pigeons.sql.exec(
  &format!("INSERT INTO pigeons (id, flock_id, serial, name, tags, connector, token_expires_at, device_public_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING {PIGEON_COLUMNS};"),
  vec![
    do_id.clone().into(),
    row.flock_id.to_string().into(),
    row.serial.into(),
    row.name.into(),
    row.tags.into(),
    connector_json.into(),
    expires_at.unix_timestamp().into(),
    public_key.into(),
  ],
) {
    Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
      Ok(p) => Pigeon::from(p),
      Err(e) => {
        console_error!("Pigeon deserialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("Pigeons create execution error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  // Insert ACL
  if let Err(e) = pigeons.sql.exec(
    "INSERT INTO pigeon_acl (entity_id, role) VALUES (?, 'owner');",
    vec![user_id.into()],
  ) {
    console_error!("Pigeon ACL create execution error: {e}");
    return Response::error("Internal Server Error", 500);
  }

  // Insert shadow
  let shadow = match pigeons.sql.exec(
    "INSERT INTO pigeon_shadow (id) VALUES (?) RETURNING target_version, current_version, target_config, current_config, updated_at;",
    vec![do_id.into()],
  ) {
    Ok(cursor) => match one_row::<PigeonShadowRow>(&cursor) {
      Ok(s) => PigeonShadow::from(s),
      Err(e) => {
        console_error!("PigeonShadow deserialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("Pigeon shadow create execution error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  // Return directly — pigeon from DB already has correct connector
  let response = PigeonDetail {
    pigeon,
    acl: PigeonAcl {
      entity_id: user_uuid,
      role: "owner".to_string(),
    },
    shadow,
  };

  let mut location = String::with_capacity(72);
  location.push_str("/pigeons/");
  location.push_str(&response.pigeon.id);

  ResponseBuilder::new()
    .with_status(201)
    .with_header("Location", &location)?
    .from_json(&response)
}

async fn refresh_token(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_owner(pigeons, &req));

  let do_id = pigeons.state.id().to_string();

  let (public_key, device_token, expires_at) = match mint_device_credential() {
    Ok(t) => t,
    Err(e) => {
      console_error!("Device credential mint error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  // Read current pigeon to determine connector type
  let mut pigeon = match pigeons.sql.exec(
    &format!("SELECT {PIGEON_COLUMNS} FROM pigeons LIMIT 1;"),
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
      Ok(p) => Pigeon::from(p),
      Err(e) => {
        console_error!("Pigeon deserialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("Pigeons READ error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  // Build new connector with refreshed token
  pigeon.connector = match &pigeon.connector {
    Connector::Https(_) => {
      let endpoint = build_http_endpoint(&pigeons.env, &do_id);
      Connector::Https(HttpsConfig {
        endpoint,
        token: device_token.clone(),
      })
    }
    Connector::Coap(_) => {
      let endpoint = build_coap_endpoint(&pigeons.env, &do_id);
      Connector::Coap(CoapConfig {
        endpoint,
        token: device_token.clone(),
        tls_psk_identity: Some(do_id.clone()),
        tls_psk_secret: Some(device_token),
      })
    }
  };

  // Serialize and update
  let connector_json = serde_json::to_string(&pigeon.connector).map_err(|e| {
    console_error!("Connector serialization error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  // Overwriting device_public_key here is what revokes the previous token:
  // once the old key is gone, its signature can never verify again,
  // regardless of the token's own embedded expiry.
  match pigeons.sql.exec(
    "UPDATE pigeons SET connector = ?, token_expires_at = ?, device_public_key = ? WHERE id = ?;",
    vec![
      connector_json.into(),
      expires_at.unix_timestamp().into(),
      public_key.into(),
      do_id.into(),
    ],
  ) {
    Ok(_) => {
      match pigeons.sql.exec(
        &format!("SELECT {PIGEON_COLUMNS} FROM pigeons LIMIT 1;"),
        None,
      ) {
        Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
          Ok(p) => Response::from_json(&Pigeon::from(p)),
          Err(e) => {
            console_error!("Pigeon token refresh error: {e}");
            Response::error("Internal Server Error", 500)
          }
        },
        Err(e) => {
          console_error!("Pigeon token refresh error: {e}");
          Response::error("Internal Server Error", 500)
        }
      }
    },
    Err(e) => {
      console_error!("Pigeon token refresh error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Deletes this pigeon. Durable Objects have no explicit "delete yourself"
/// API — an object becomes eligible for eviction once its storage is
/// empty — so this wipes every row this DO owns instead. `pigeon_shadow`
/// cascades via its foreign key; `pigeon_acl` has none (it's a flat table
/// scoped to this DO's single pigeon, not keyed by pigeon id), so it's
/// cleared explicitly.
async fn delete(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_owner(pigeons, &req));

  if let Err(e) = pigeons.sql.exec("DELETE FROM pigeon_acl;", None) {
    console_error!("Pigeon ACL delete execution error: {e}");
    return Response::error("Internal Server Error", 500);
  }

  match pigeons.sql.exec(
    "DELETE FROM pigeons WHERE id = ?;",
    vec![pigeons.state.id().to_string().into()],
  ) {
    Ok(_) => Response::ok("Pigeon Deleted"),
    Err(e) => {
      console_error!("Pigeon delete execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn update(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  let row = match req.json::<PigeonUpdateRequest>().await {
    Ok(data) => data,
    Err(e) => {
      console_error!("Pigeon UPDATE json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  // Serialize connector to JSON string if present
  let connector_json = row
    .connector
    .map(|c| serde_json::to_string(&c).unwrap_or_default());

  match pigeons.sql.exec(
    "UPDATE pigeons SET
      flock_id = COALESCE(?, flock_id),
      serial = COALESCE(?, serial),
      name = COALESCE(?, name),
      tags = COALESCE(?, tags),
      connector = COALESCE(?, connector)
    WHERE id = ?;",
    vec![
      row.flock_id.map(|u| u.to_string()).into(),
      row.serial.into(),
      row.name.into(),
      row.tags.into(),
      connector_json.into(), // Now Option<String> — None becomes null, Some becomes JSON text
      pigeons.state.id().to_string().into(),
    ],
  ) {
    Ok(_) => {
      // Read back the updated row to return
      match pigeons.sql.exec(
        &format!("SELECT {PIGEON_COLUMNS} FROM pigeons LIMIT 1;"),
        None,
      ) {
        Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
          Ok(p) => Response::from_json(&Pigeon::from(p)),
          Err(e) => {
            console_error!("Pigeon deserialization error: {e}");
            Response::error("Internal Server Error", 500)
          }
        },
        Err(e) => {
          console_error!("Pigeons READ error: {e}");
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("Pigeon UPDATE execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn get_acl(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  let Ok(Some(requesting_user)) = req.headers().get("X-User-Id") else {
    return Response::error("Request missing 'X-User-Id'", 400);
  };

  match pigeons.sql.exec(
    "SELECT entity_id, role FROM pigeon_acl WHERE entity_id = ?;",
    vec![requesting_user.into()],
  ) {
    Ok(cursor) => match one_row::<PigeonAcl>(&cursor) {
      Ok(acl) => Response::from_json(&acl),
      Err(e) => {
        console_error!("PigeonAcl deserialization error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("PigeonAcl READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn update_acl(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_owner(pigeons, &req));

  let acl = match req.json::<PigeonAclUpdateRequest>().await {
    Ok(data) => data,
    Err(e) => {
      console_error!("PigeonAcl UPDATE json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  match pigeons.sql.exec(
    "INSERT INTO pigeon_acl (entity_id, role) VALUES (?, ?)
     ON CONFLICT(entity_id) DO UPDATE SET role = excluded.role;",
    vec![acl.entity_id.to_string().into(), acl.role.clone().into()],
  ) {
    // The gateway route (POST /pigeons/:id/acl, lib.rs) parses this
    // response body as JSON `PigeonAcl` (matching every other DO write
    // handler's success shape -- update_shadow, update_telemetry_endpoint,
    // refresh_token, etc.) via parse_do_response::<PigeonAcl>(..).await?.
    // A plain-text "ACL Updated" body here made that parse fail every
    // time, which propagated through `?` into the top-level catch-all and
    // always 500'd the caller even though this INSERT had already
    // succeeded -- the write worked, only the HTTP response was wrong.
    Ok(_) => Response::from_json(&PigeonAcl {
      entity_id: acl.entity_id,
      role: acl.role,
    }),
    Err(e) => {
      console_error!("PigeonAcl UPDATE execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn list_acl(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_owner(pigeons, &req));

  match pigeons
    .sql
    .exec("SELECT entity_id, role FROM pigeon_acl;", None)
  {
    Ok(cursor) => match cursor.to_array::<PigeonAcl>() {
      Ok(acls) => Response::from_json(&acls),
      Err(e) => {
        console_error!("PigeonAcl LIST error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("PigeonAcl LIST error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn get_shadow(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  match pigeons.sql.exec(
    "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonShadowRow>(&cursor) {
      Ok(s) => Response::from_json(&PigeonShadow::from(s)),
      Err(e) => {
        console_error!("PigeonShadow deserialization error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("PigeonShadow READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

#[derive(serde::Deserialize)]
struct DevicePublicKeyRow {
  device_public_key: String,
}

/// Device auth for the `/pigeon/device/*` routes. Mirrors `is_authorized`/
/// `is_owner`'s early-return convention (`unwrap_or_return_response!`) but
/// checks no `X-User-Id`/ACL — a device has no Kratos user identity.
/// Instead it verifies the request's bearer token against this pigeon's own
/// stored `device_public_key`, which only this DO (the source of truth for
/// the pigeon's current credential) holds.
fn is_authorized_device(
  pigeons: &Pigeons,
  req: &Request,
) -> std::result::Result<(), Result<Response>> {
  let Ok(Some(auth_header)) = req.headers().get("Authorization") else {
    return Err(Response::error(
      "Unauthorized: Missing Authorization header",
      401,
    ));
  };

  let Some(token) = auth_header.strip_prefix("Bearer ") else {
    return Err(Response::error("Unauthorized: Missing Bearer token", 401));
  };

  let public_key = match pigeons
    .sql
    .exec("SELECT device_public_key FROM pigeons LIMIT 1;", None)
  {
    Ok(cursor) => match one_row::<DevicePublicKeyRow>(&cursor) {
      Ok(row) => row.device_public_key,
      Err(e) => {
        console_error!("Pigeon public key deserialization error: {e}");
        return Err(Response::error("Internal Server Error", 500));
      }
    },
    Err(e) => {
      console_error!("Pigeon public key READ error: {e}");
      return Err(Response::error("Internal Server Error", 500));
    }
  };

  if !verify_device_token(token, &public_key) {
    return Err(Response::error("Unauthorized: Invalid token", 401));
  }

  Ok(())
}

/// Device-facing shadow read. Unlike `get_shadow`, this is not gated by
/// `is_authorized`/`X-User-Id` — see `is_authorized_device`.
async fn get_shadow_device(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  match pigeons.sql.exec(
    "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonShadowRow>(&cursor) {
      Ok(s) => Response::from_json(&PigeonShadow::from(s)),
      Err(e) => {
        console_error!("PigeonShadow deserialization error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("PigeonShadow READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Device-facing shadow report-back. Same device-auth model as
/// `get_shadow_device` (bearer token verified against this pigeon's own
/// `device_public_key`, no `X-User-Id`/ACL) — lets the device confirm it
/// applied `target_config` by writing its own `current_config` plus the
/// `target_version` it just applied (echoed back from an earlier GET, into
/// `current_version`) — the SQL layer never re-derives this from
/// `target_version` itself, since the device might still be catching up to
/// a newer target by the time this lands.
async fn report_shadow_device(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  let report = match req.json::<PigeonShadowReportRequest>().await {
    Ok(data) => data,
    Err(e) => {
      console_error!("Shadow REPORT json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  match write_shadow_report(pigeons, &report) {
    Ok(shadow) => Response::from_json(&shadow),
    Err(e) => {
      console_error!("Shadow REPORT execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Shared SQL for a device confirming it applied `target_config` --
/// factored out of `report_shadow_device` (the HTTP route,
/// `POST /device/pigeons/:id/shadow`) so `handle_ws_shadow_report` (the
/// WebSocket `shadow_report` frame, task #32) can reuse the exact same
/// write instead of duplicating the query text. Only the SQL; callers are
/// responsible for their own auth (already done once for the WS case, at
/// socket accept) and for whatever Postgres sync/response shape they need
/// around it.
fn write_shadow_report(
  pigeons: &Pigeons,
  report: &PigeonShadowReportRequest,
) -> Result<PigeonShadow> {
  let config_str = serde_json::to_string(&report.current_config).map_err(|e| {
    console_error!("Shadow config serialization error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  pigeons.sql.exec(
    "UPDATE pigeon_shadow SET current_config = ?, current_version = ? WHERE id = (SELECT id FROM pigeons LIMIT 1);",
    vec![config_str.into(), report.current_version.into()],
  )?;

  let cursor = pigeons.sql.exec(
    "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
    None,
  )?;
  one_row::<PigeonShadowRow>(&cursor).map(PigeonShadow::from)
}

/// Device WebSocket upgrade (task #32) -- the real-time channel for
/// non-cellular (WiFi/mains-powered) devices, replacing the HTTP shadow
/// poll/telemetry-POST pattern with a persistent connection. Bearer auth
/// happens exactly once, here, BEFORE the socket is ever accepted -- unlike
/// the HTTP `/pigeon/device/*` routes there is no per-frame re-check
/// afterward (see `DurableObject::websocket_message` above); a device holds
/// the connection, not a fresh credential each time.
///
/// Accepted via the Durable Object *hibernation* API
/// (`State::accept_websocket_with_tags`), not the in-memory
/// `WebSocket::accept()` -- an idle connection (a device that only reports
/// every few minutes) can be evicted from this DO's memory between messages
/// without being torn down; the runtime transparently re-wakes this DO and
/// re-dispatches to `websocket_message`/`websocket_close`/`websocket_error`
/// on the next event for that socket. `WebSocket::accept()` would keep this
/// DO pinned in memory (billed) for the entire connection lifetime instead.
///
/// Tagged `WS_DEVICE_TAG` (rather than left untagged) so a future second
/// socket class -- the remote-shell relay, task #34 -- can coexist without
/// either class's "close the old one"/broadcast logic touching the other's
/// sockets; see the tag-scoped `get_websockets_with_tag` calls here and in
/// `update_shadow`.
async fn accept_websocket_device(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  // One active device socket per pigeon: a new connection (e.g. a device
  // reconnecting after a network blip, before its old socket has timed out)
  // replaces the old one rather than coexisting with it.
  for existing in pigeons.state.get_websockets_with_tag(WS_DEVICE_TAG) {
    let _ = existing.close(Some(4009), Some("replaced by new connection"));
  }

  let pair = WebSocketPair::new()?;
  pigeons
    .state
    .accept_websocket_with_tags(&pair.server, &[WS_DEVICE_TAG]);

  Response::from_websocket(pair.client)
}

/// Backs the WebSocket `shadow_report` frame (task #32) -- the WS
/// counterpart to `report_shadow_device` above, reusing `write_shadow_report`
/// for the actual write. Unlike that HTTP route, there is no gateway route
/// left in the loop to best-effort sync the result to Postgres afterward
/// (frames go straight into this DO once the socket is established), so
/// this does that sync itself, matching what the gateway route
/// (`POST /device/pigeons/:id/shadow` in `lib.rs`) does via
/// `update_shadow_pg_db`. Errors at any step are logged and otherwise
/// swallowed -- there's no HTTP response to carry them back to the device,
/// and a malformed/failed report shouldn't kill the connection.
async fn handle_ws_shadow_report(pigeons: &Pigeons, report: &PigeonShadowReportRequest) {
  let shadow = match write_shadow_report(pigeons, report) {
    Ok(s) => s,
    Err(e) => {
      console_error!(
        "WS shadow report: write failed for pigeon {}: {e}",
        pigeons.state.id()
      );
      return;
    }
  };

  let pigeon_id = pigeons.state.id().to_string();
  match crate::helpers::get_db_client(&pigeons.env).await {
    Ok(client) => {
      if let Err(e) = crate::helpers::update_shadow_pg_db(client, &pigeon_id, &shadow).await {
        console_error!("WS shadow report: PG sync failed for pigeon {pigeon_id}: {e}");
      }
    }
    Err(e) => {
      console_error!(
        "WS shadow report: PG sync skipped for pigeon {pigeon_id}: Hyperdrive connection failed: {e}"
      );
    }
  }
}

/// Backs the WebSocket `telemetry` frame (task #32) -- the WS counterpart to
/// `report_telemetry_device`/the queue-producer path in `lib.rs`. Always
/// does the DO's own latest-value upsert synchronously first (same as every
/// other telemetry entry point), then either enqueues onto
/// `TELEMETRY_QUEUE` for the same consumer path the HTTP route uses (PG
/// history or line-protocol forward, decided by `telemetry_endpoint` --
/// see `queue.rs`), or, in an environment with no queue bound (dev today),
/// writes PG history directly so telemetry sent over the socket doesn't
/// silently skip history the HTTP route would have recorded in that same
/// environment. Unlike the HTTP route, no separate auth round trip is
/// needed before enqueueing -- the bearer token was already verified once,
/// at socket accept, and this frame could only have arrived on an already-
/// authenticated connection.
async fn handle_ws_telemetry(
  pigeons: &Pigeons,
  metrics: std::collections::HashMap<String, String>,
) {
  if metrics.is_empty() {
    return;
  }

  if upsert_telemetry(pigeons, &metrics).is_err() {
    return;
  }

  let pigeon_id = pigeons.state.id().to_string();

  match pigeons.env.queue("TELEMETRY_QUEUE") {
    Ok(queue) => {
      let Ok(metrics_json) = serde_json::to_string(&metrics) else {
        console_error!("WS telemetry: failed to serialize metrics for pigeon {pigeon_id}");
        return;
      };

      let message = TelemetryMessage {
        pigeon_id: pigeon_id.clone(),
        metrics_json,
        reported_at_ms: Date::now().as_millis(),
      };

      if queue.send(message).await.is_err() {
        console_error!("WS telemetry: enqueue failed for pigeon {pigeon_id}");
      }
    }
    Err(_) => {
      // No TELEMETRY_QUEUE bound in this environment (dev) -- match the
      // HTTP route's own no-queue fallback (report_telemetry_device, which
      // is auth+write in one call with no queue involved) by writing PG
      // history directly here instead of silently dropping it.
      if let Err(e) =
        crate::helpers::write_telemetry_history(&pigeons.env, &pigeon_id, &metrics).await
      {
        console_error!("WS telemetry: PG history write failed for pigeon {pigeon_id}: {e}");
      }
    }
  }
}

#[derive(serde::Deserialize)]
struct TargetConfigRow {
  target_config: String,
}

/// Firmware shadow-key lookup for the device-facing firmware download route
/// (`GET /device/pigeons/:id/firmware` in `lib.rs`, task #23). Same
/// device-auth model as `get_shadow_device`/`report_shadow_device` (bearer
/// token verified against this pigeon's own `device_public_key`, no
/// `X-User-Id`/ACL). Reads this pigeon's own `target_config` and extracts
/// the `firmware` key (see `capsules::FirmwareTarget`'s doc comment for the
/// coordinated shape) so the gateway can resolve which R2 object to stream
/// back in one DO round trip instead of two — the firmware bytes
/// themselves never pass through this DO (SQLite is not acceptable for
/// MB-sized blobs; see this crate's `CLAUDE.md`). 404 if this pigeon's
/// shadow currently has no `firmware` key set (nothing assigned yet).
async fn get_firmware_target_device(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  let target_config = match pigeons
    .sql
    .exec("SELECT target_config FROM pigeon_shadow LIMIT 1;", None)
  {
    Ok(cursor) => match one_row::<TargetConfigRow>(&cursor) {
      Ok(row) => row.target_config,
      Err(e) => {
        console_error!("Shadow target_config READ error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    Err(e) => {
      console_error!("Shadow target_config READ error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  let parsed: serde_json::Value = match serde_json::from_str(&target_config) {
    Ok(v) => v,
    Err(e) => {
      console_error!("target_config JSON parse error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  let Some(firmware_value) = parsed.get("firmware") else {
    return Response::error("Not Found: No firmware assigned to this pigeon", 404);
  };

  match serde_json::from_value::<FirmwareTarget>(firmware_value.clone()) {
    Ok(target) => Response::from_json(&target),
    Err(e) => {
      console_error!("Malformed firmware target in shadow: {e}");
      Response::error("Bad Request: Malformed firmware target in shadow", 400)
    }
  }
}

/// Shared upsert loop behind all three telemetry-write entry points below
/// (`report_telemetry_device`, `write_telemetry_device`) -- each key
/// overwrites its own row in `pigeon_telemetry`; this is a
/// latest-value-per-key store, not a time-series log (see the table's
/// creation comment in `DurableObject::new`).
fn upsert_telemetry(
  pigeons: &Pigeons,
  metrics: &std::collections::HashMap<String, String>,
) -> Result<()> {
  for (key, value) in metrics {
    if let Err(e) = pigeons.sql.exec(
      "INSERT INTO pigeon_telemetry (key, value) VALUES (?, ?)
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, reported_at = unixepoch();",
      vec![key.clone().into(), value.clone().into()],
    ) {
      console_error!("Telemetry UPSERT error for key '{key}': {e}");
      return Err(e);
    }
  }
  Ok(())
}

/// Device-facing telemetry ingestion. Same device-auth model as
/// `get_shadow_device`/`report_shadow_device` (bearer token verified against
/// this pigeon's own `device_public_key`, no `X-User-Id`/ACL). Body is a
/// flat JSON object of string key/value pairs (matches pigeon's
/// pigeon_set_shadow_param/pigeon_shadow_flush wire shape). Used directly
/// (auth + write in one call) in environments with no telemetry queue bound
/// -- see the gateway route in `lib.rs`; where a queue *is* bound, that
/// route calls `verify_telemetry_device` + enqueues instead, and the queue
/// consumer (`src/queue.rs`) reaches `write_telemetry_device` below. Since
/// this handler is only ever dispatched from the no-queue fallback branch
/// of the gateway route, it also best-effort writes `pigeon_telemetry_history`
/// directly here -- the same fallback `handle_ws_telemetry` already does for
/// the WebSocket `telemetry` frame -- so the HTTP route doesn't silently skip
/// history that environment would otherwise have recorded via the queue.
async fn report_telemetry_device(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  let metrics = match req
    .json::<std::collections::HashMap<String, String>>()
    .await
  {
    Ok(data) => data,
    Err(e) => {
      console_error!("Telemetry json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  if metrics.is_empty() {
    return Response::error("Bad Request: Empty telemetry report", 400);
  }

  if upsert_telemetry(pigeons, &metrics).is_err() {
    return Response::error("Internal Server Error", 500);
  }

  let pigeon_id = pigeons.state.id().to_string();
  if let Err(e) =
    crate::helpers::write_telemetry_history(&pigeons.env, &pigeon_id, &metrics).await
  {
    console_error!("HTTP telemetry: PG history write failed for pigeon {pigeon_id}: {e}");
  }

  Response::from_json(&metrics)
}

/// Auth-only counterpart to `report_telemetry_device`, used by the gateway
/// route (`lib.rs`) when a telemetry queue is bound for this environment:
/// verifies the device's bearer token against this pigeon's stored public
/// key WITHOUT writing anything, so the gateway can confirm the report is
/// genuine before it ever reaches the queue -- the queue itself has no
/// authentication of its own. No response body; the caller only inspects
/// the status code.
async fn verify_telemetry_device(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));
  Response::ok("")
}

/// Trusted-internal write counterpart to `verify_telemetry_device`:
/// performs the same upsert as `report_telemetry_device` but with NO auth
/// check of its own. Safe only because it is reachable exclusively from
/// this Worker's own queue consumer (`src/queue.rs`), which only ever
/// dispatches messages that already passed `verify_telemetry_device` at
/// enqueue time -- Durable Objects have no public internet-facing address,
/// so there is no path for an unauthenticated caller to reach this route
/// directly.
/// Response body for `write_telemetry_device`: besides confirming what got
/// written, it hands the queue consumer (`src/queue.rs`) this pigeon's
/// `telemetry_endpoint` (task #18) so it can decide, without a second DO
/// round trip, whether to forward the report externally (line protocol to
/// a user-configured endpoint) or write it into our own
/// `pigeon_telemetry_history` Postgres mirror.
/// `pub` (and its fields) so the queue consumer (`queue.rs`) can deserialize
/// this same shape from the DO's write response and decide, without
/// duplicating the type, whether to forward externally or write our own PG
/// history (task #18, part 2).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TelemetryWriteResult {
  pub metrics: std::collections::HashMap<String, String>,
  pub telemetry_endpoint: Option<TelemetryEndpoint>,
}

#[derive(serde::Deserialize)]
struct TelemetryEndpointRow {
  telemetry_endpoint: Option<String>,
}

fn read_telemetry_endpoint(pigeons: &Pigeons) -> Option<TelemetryEndpoint> {
  let cursor = pigeons
    .sql
    .exec("SELECT telemetry_endpoint FROM pigeons LIMIT 1;", None)
    .ok()?;
  let row = one_row::<TelemetryEndpointRow>(&cursor).ok()?;
  row
    .telemetry_endpoint
    .and_then(|s| serde_json::from_str(&s).ok())
}

async fn write_telemetry_device(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  let metrics = match req
    .json::<std::collections::HashMap<String, String>>()
    .await
  {
    Ok(data) => data,
    Err(e) => {
      console_error!("Telemetry json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  if metrics.is_empty() {
    return Response::error("Bad Request: Empty telemetry report", 400);
  }

  if upsert_telemetry(pigeons, &metrics).is_err() {
    return Response::error("Internal Server Error", 500);
  }

  Response::from_json(&TelemetryWriteResult {
    metrics,
    telemetry_endpoint: read_telemetry_endpoint(pigeons),
  })
}

/// Bare ACL probe for gateway routes whose data lives outside this DO
/// (telemetry history is in Postgres) but whose authorization still lives
/// in this pigeon's local `pigeon_acl` table.
async fn check_authorized(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));
  Response::ok("authorized")
}

/// ACL-gated latest-value read for the dashboard (`GET
/// /pigeons/:id/telemetry` in `lib.rs`) -- every key currently in the DO's
/// `pigeon_telemetry` table, unlike the history routes which read
/// `pigeon_telemetry_history` from Postgres.
async fn get_telemetry_latest(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  match pigeons
    .sql
    .exec("SELECT key, value, reported_at FROM pigeon_telemetry;", None)
  {
    Ok(cursor) => match cursor.to_array::<TelemetryLatestRow>() {
      Ok(rows) => {
        let latest: Vec<TelemetryLatest> = rows.into_iter().map(TelemetryLatest::from).collect();
        Response::from_json(&latest)
      }
      Err(e) => {
        console_error!("Telemetry latest LIST error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("Telemetry latest READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Dashboard-facing setter for the per-pigeon telemetry forwarding target
/// (task #18). Same authorization level as the generic pigeon `update`
/// route (`is_authorized`, not `is_owner` -- any ACL entry, not just the
/// owner, may configure this). A `None` body clears the endpoint, reverting
/// to our own Postgres history; unlike `update`'s `PigeonUpdateRequest`
/// fields this is a direct `SET`, not `COALESCE`, so `None` truly means
/// NULL rather than "leave unchanged" -- there is no "leave unchanged"
/// notion for this dedicated single-field route.
async fn update_telemetry_endpoint(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  let body = match req
    .json::<capsules::PigeonTelemetryEndpointUpdateRequest>()
    .await
  {
    Ok(data) => data,
    Err(e) => {
      console_error!("Telemetry endpoint UPDATE json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  let endpoint_json = match &body.telemetry_endpoint {
    Some(endpoint) => match serde_json::to_string(endpoint) {
      Ok(s) => Some(s),
      Err(e) => {
        console_error!("Telemetry endpoint serialization error: {e}");
        return Response::error("Internal Server Error", 500);
      }
    },
    None => None,
  };

  match pigeons.sql.exec(
    "UPDATE pigeons SET telemetry_endpoint = ? WHERE id = ?;",
    vec![
      endpoint_json.into(),
      pigeons.state.id().to_string().into(),
    ],
  ) {
    Ok(_) => Response::from_json(&body.telemetry_endpoint),
    Err(e) => {
      console_error!("Telemetry endpoint UPDATE execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Bounded ring-buffer cap for `pigeon_log_chunks` (task #18, part 3) --
/// each device log chunk is small (Zephyr dictionary-log records capped at
/// `capsules::MAX_LOG_CHUNK_BYTES` on the way in), but an unbounded stream
/// would grow this DO's SQLite storage without limit. After every insert,
/// rows beyond this count (oldest first) are pruned.
const MAX_STORED_LOG_CHUNKS: i64 = 200;

/// Device-facing dictionary-log chunk ingestion (task #18, part 3). Same
/// device-auth model as the other `/pigeon/device/*` routes (bearer token
/// verified against this pigeon's own `device_public_key`, no
/// `X-User-Id`/ACL). The body is the raw binary chunk, not JSON -- the
/// gateway route (`lib.rs`) forwards it via `proxy_binary_to_pigeon_do`
/// rather than `proxy_to_pigeon_do`, which reads the body as UTF-8 text and
/// would corrupt arbitrary binary bytes. Stored as base64 text (matches
/// this codebase's existing convention for binary data -- see
/// `device_public_key`, device tokens -- rather than a SQLite BLOB column,
/// since it's handed back to the dashboard as base64 anyway; see
/// `get_logs`/`capsules::PigeonLogChunk`).
async fn report_logs_device(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  let bytes = match req.bytes().await {
    Ok(b) => b,
    Err(e) => {
      console_error!("Log chunk body read error: {e}");
      return Response::error("Bad Request: Failed to read body", 400);
    }
  };

  if bytes.is_empty() {
    return Response::error("Bad Request: Empty log chunk", 400);
  }

  if bytes.len() > MAX_LOG_CHUNK_BYTES {
    return Response::error("Payload Too Large: Log chunk exceeds size cap", 413);
  }

  let data_b64 = STANDARD.encode(&bytes);

  if let Err(e) = pigeons.sql.exec(
    "INSERT INTO pigeon_log_chunks (data) VALUES (?);",
    vec![data_b64.into()],
  ) {
    console_error!("Log chunk INSERT error: {e}");
    return Response::error("Internal Server Error", 500);
  }

  // Prune beyond the ring-buffer cap, oldest first. Non-fatal if this
  // fails -- the chunk itself is already durably stored.
  if let Err(e) = pigeons.sql.exec(
    "DELETE FROM pigeon_log_chunks WHERE id NOT IN (
       SELECT id FROM pigeon_log_chunks ORDER BY id DESC LIMIT ?
     );",
    vec![MAX_STORED_LOG_CHUNKS.into()],
  ) {
    console_error!("Log chunk prune error: {e}");
  }

  Response::ok("")
}

/// ACL-gated read for the dashboard (`GET /pigeons/:id/logs` in `lib.rs`) --
/// every currently-stored chunk, oldest first, as base64 text for
/// host-side decode (Zephyr's dictionary-log tooling decodes off the
/// firmware's own ELF, which the backend has no access to).
async fn get_logs(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  match pigeons.sql.exec(
    "SELECT id, data, received_at FROM pigeon_log_chunks ORDER BY id ASC;",
    None,
  ) {
    Ok(cursor) => match cursor.to_array::<PigeonLogChunkRow>() {
      Ok(rows) => {
        let chunks: Vec<PigeonLogChunk> = rows.into_iter().map(PigeonLogChunk::from).collect();
        Response::from_json(&chunks)
      }
      Err(e) => {
        console_error!("Log chunk LIST error: {e}");
        Response::error("Internal Server Error", 500)
      }
    },
    Err(e) => {
      console_error!("Log chunk READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

async fn update_shadow(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  let shadow = match req.json::<PigeonShadowUpdateRequest>().await {
    Ok(data) => data,
    Err(e) => {
      console_error!("Shadow UPDATE json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  let config_str = serde_json::to_string(&shadow.target_config).map_err(|e| {
    console_error!("Shadow config serialization error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  match pigeons.sql.exec(
    "UPDATE pigeon_shadow SET target_config = ? WHERE id = (SELECT id FROM pigeons LIMIT 1);",
    vec![config_str.into()],
  ) {
    Ok(_) => {
      match pigeons.sql.exec(
        "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
        None,
      ) {
        Ok(cursor) => match one_row::<PigeonShadowRow>(&cursor) {
          Ok(s) => {
            let shadow = PigeonShadow::from(s);
            broadcast_shadow_update(pigeons, &shadow);
            Response::from_json(&shadow)
          }
          Err(e) => {
            console_error!("PigeonShadow deserialization error: {e}");
            Response::error("Internal Server Error", 500)
          }
        },
        Err(e) => {
          console_error!("PigeonShadow READ error: {e}");
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("Shadow UPDATE execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Pushes the new shadow to this pigeon's connected device WebSocket, if
/// any (task #32) -- the headline latency win this endpoint exists for:
/// without it, a device only learns about a new `target_config` on its next
/// poll. Scoped to `WS_DEVICE_TAG` so a future remote-shell socket (task
/// #34) never receives a frame meant for the device-telemetry protocol.
/// Best-effort: a `send` failure (socket mid-close, buffer full, etc.) is
/// logged and otherwise ignored, matching this codebase's established
/// best-effort-sync convention -- the shadow write itself already succeeded
/// and is the primary result of this request either way.
fn broadcast_shadow_update(pigeons: &Pigeons, shadow: &PigeonShadow) {
  for ws in pigeons.state.get_websockets_with_tag(WS_DEVICE_TAG) {
    if let Err(e) = ws.send(&WsOutboundFrame::ShadowUpdate {
      shadow: shadow.clone(),
    }) {
      console_error!(
        "WS shadow push failed for pigeon {}: {e}",
        pigeons.state.id()
      );
    }
  }
}
