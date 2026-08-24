use crate::helpers::{LegacyTelemetryRow, TelemetryBlob};
use crate::objects::ws::{
  MAX_WS_FRAME_BYTES, WS_CLOSE_INGEST_PAUSED, WS_CLOSE_PIGEON_DELETED, WS_CLOSE_TOKEN_REVOKED,
  WS_DEVICE_TAG, WsInboundFrame, WsOutboundFrame, check_rate_limit,
};
use crate::objects::{mint_coap_psk, mint_device_credential, verify_device_token};
use crate::queue::TelemetryMessage;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use capsules::{
  CoapConfig, Connector, FirmwareTarget, HttpsConfig, MAX_LOG_CHUNK_BYTES, Pigeon, PigeonAcl,
  PigeonAclUpdateRequest, PigeonCreateRequest, PigeonDetail, PigeonLogChunk, PigeonLogChunkRow,
  PigeonRow, PigeonShadow, PigeonShadowReportRequest, PigeonShadowRow, PigeonShadowUpdateRequest,
  PigeonUpdateRequest, TelemetryEndpoint, unwrap_or_return_response,
};
use futures::FutureExt;
use futures::channel::oneshot;
use std::cell::RefCell;
use std::collections::HashMap;
use worker::{
  Date, DurableObject, Env, Request, Response, ResponseBuilder, Result, SqlStorage, State,
  WebSocket, WebSocketIncomingMessage, WebSocketPair, console_error, console_log, durable_object,
  wasm_bindgen,
};

/// Shared by every `pigeons` read/RETURNING statement so a column can't
/// silently go missing from one of the near-identical queries.
const PIGEON_COLUMNS: &str = "id, flock_id, serial, name, tags, connector, token_expires_at, telemetry_endpoint, board, updated_at, created_at";

// A missing DEVICE_API_HOST binding should degrade to prod's own host
// rather than emit a garbage endpoint.
const DEFAULT_DEVICE_API_HOST: &str = "api.pidgeiot.com";
const DEVICE_PIGEONS_PATH: &str = "/device/pigeons/";

/// The host a minted device endpoint points at -- deliberately NOT
/// `ROOT_URL`, which is the frontend's origin; the two differ per
/// environment. Read fresh per call: Durable Objects can outlive a single
/// Worker invocation.
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

/// Host minted into `coaps://` endpoints. `COAP_DEVICE_HOST`
/// ([vars]/[env.*.vars], wrangler.toml) when set and non-empty -- the CoAP
/// terminator (`loft`) is a separate deployment from the HTTP edge, on its
/// own DNS-only hostname (runbook in the loft repo) -- with the
/// pre-terminator fallback to `DEVICE_API_HOST` for any env that leaves it
/// empty (e.g. staging until a staging terminator exists).
fn coap_device_host(env: &Env) -> String {
  env
    .var("COAP_DEVICE_HOST")
    .map(|v| v.to_string())
    .ok()
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| device_api_host(env))
}

/// Mints the endpoint in the RFC 7252 `coaps://` (DTLS/UDP) form -- the
/// primary device transport. The terminator (`loft`) serves DTLS/UDP and
/// TLS/TCP on the same authority (one host, port 5684), and the device
/// client validates the endpoint scheme against its compiled transport,
/// failing loudly on a mismatch -- so the stored default must name the
/// primary transport; a TLS/TCP build keeps the authority and swaps the
/// scheme to `coaps+tcp://` (RFC 8323).
#[inline]
pub fn build_coap_endpoint(env: &Env, do_id: &str) -> String {
  let host = coap_device_host(env);
  let mut endpoint = String::with_capacity(8 + host.len() + DEVICE_PIGEONS_PATH.len() + 64);
  endpoint.push_str("coaps://");
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
  // In-flight `POST /pigeons/:id/shell` waiters, keyed by request_id and
  // resolved when the matching `shell_output` frame arrives. `RefCell` is
  // safe: a DO's async tasks share one thread under a cooperative
  // executor, so insert and resolve can't race. Not SQLite-backed on
  // purpose -- a pending command has no meaning across a DO eviction
  // (there's no in-flight HTTP handler left to resolve).
  shell_waiters: RefCell<HashMap<String, oneshot::Sender<ShellOutputPayload>>>,
}

/// Carrier for a device's `shell_output` frame fields, handed from
/// `websocket_message` to the `execute_shell_command` invocation awaiting
/// it. No `request_id` field -- the `shell_waiters` map key already is it.
struct ShellOutputPayload {
  output: String,
  exit_code: i32,
  truncated: bool,
}

/// `SqlCursor::one()` throws an uncaught JS exception (crashing the DO)
/// on zero rows instead of returning a catchable `Result::Err`;
/// `to_array()` never throws. Matters because `delete()` can leave this
/// DO's tables empty while the DO itself is still addressable.
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
          board TEXT DEFAULT NULL,
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

    // Column migration for DOs created before `telemetry_endpoint`
    // existed -- `CREATE TABLE IF NOT EXISTS` above is a no-op against an
    // existing table and SQLite has no `ADD COLUMN IF NOT EXISTS`, so a
    // "column already present" error is expected and ignored.
    let _ = sql.exec(
      "ALTER TABLE pigeons ADD COLUMN telemetry_endpoint TEXT DEFAULT NULL;",
      None,
    );

    // Same deal for `board`: the pigeon's Zephyr `CONFIG_BOARD_TARGET`
    // string (e.g. "circuitdojo_feather/nrf9160/ns"), operator-set at
    // provisioning time -- enforced in `check_firmware_board_compat`.
    let _ = sql.exec(
      "ALTER TABLE pigeons ADD COLUMN board TEXT DEFAULT NULL;",
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

    // Latest-value-per-key store, not a time-series log -- a telemetry
    // report overwrites, it doesn't append. History/range queries are
    // served from Postgres `pigeon_telemetry_history` instead.
    //
    // Every key lives in one JSON object in one row rather than a row per
    // key, because DO SQLite bills per row read and written and this is the
    // hottest write path on the platform. The shape and the per-key eviction
    // that bounds it live in `helpers::telemetry_latest`.
    //
    // The billing model is what dictates this table's shape, so three things
    // here are load-bearing rather than stylistic:
    //
    // 1. `id INTEGER PRIMARY KEY` is the rowid, which SQLite does NOT give a
    //    backing index. The retired per-key table's `key TEXT PRIMARY KEY`
    //    did get one (`sqlite_autoindex_...`, confirmed by querying
    //    `sqlite_master` for both shapes under workerd), and an index costs
    //    an extra written row whenever a write touches the indexed column,
    //    so every first report of a key used to pay two writes rather than
    //    one.
    // 2. Never add an index to this table. Rows read are rows scanned, and
    //    the one read is already a direct rowid seek: `EXPLAIN QUERY PLAN`
    //    reports `SEARCH pigeon_telemetry_latest USING INTEGER PRIMARY KEY
    //    (rowid=?)`, where the old shape's previous-value lookup reported a
    //    bare `SCAN`. An index could only add written rows, never save a
    //    read.
    // 3. The per-report write is an upsert that only ever changes `metrics`
    //    and `updated_at`, neither of them indexed, and an upsert bills as
    //    the one row it actually writes rather than as an insert plus an
    //    update. So a report costs one row read and one row written whatever
    //    its key count, where the old shape cost roughly two reads and one
    //    write per key.
    //
    // No `updated_at` trigger like the tables above -- the single writer
    // stamps it in the same statement, and every key carries its own
    // `reported_at` inside the blob anyway.
    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeon_telemetry_latest (
          id INTEGER PRIMARY KEY CHECK (id = 1),
          metrics TEXT NOT NULL DEFAULT '{}',
          updated_at INTEGER DEFAULT (unixepoch())
        );",
        None,
      )
      .expect("created pigeon_telemetry_latest table");

    migrate_legacy_telemetry(&sql);

    // Bounded ring buffer of opaque device log chunks, stored as base64
    // text (see `report_logs_device`). `id` is a plain autoincrement so
    // pruning can cheaply keep the newest N rows.
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

    Pigeons {
      sql,
      state,
      env,
      shell_waiters: RefCell::new(HashMap::new()),
    }
  }

  async fn fetch(&self, req: Request) -> Result<Response> {
    let path = req.path();

    match path.as_str() {
      "/pigeon/get" => get(self, req).await,
      "/pigeon/detail" => get_detail(self, req).await,
      "/pigeon/create" => create(self, req).await,
      "/pigeon/update" => update(self, req).await,
      "/pigeon/acl/get" => get_acl(self, req).await,
      "/pigeon/acl/list" => list_acl(self, req).await,
      "/pigeon/acl/update" => update_acl(self, req).await,
      "/pigeon/acl/grant" => grant_acl_internal(self, req).await,
      "/pigeon/shadow/get" => get_shadow(self, req).await,
      "/pigeon/device/shadow" => get_shadow_device(self, req).await,
      "/pigeon/device/shadow/report" => report_shadow_device(self, req).await,
      "/pigeon/device/ws" => accept_websocket_device(self, req).await,
      "/pigeon/device/firmware/target" => get_firmware_target_device(self, req).await,
      "/pigeon/device/telemetry" => report_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/verify" => verify_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/write" => write_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/endpoint" => read_telemetry_endpoint_device(self, req).await,
      "/pigeon/telemetry/get" => get_telemetry_latest(self, req).await,
      "/pigeon/demo/telemetry" => get_telemetry_latest_demo(self, req).await,
      "/pigeon/telemetry-endpoint/update" => update_telemetry_endpoint(self, req).await,
      "/pigeon/authz/check" => check_authorized(self, req).await,
      "/pigeon/device/logs" => report_logs_device(self, req).await,
      "/pigeon/logs/get" => get_logs(self, req).await,
      "/pigeon/shadow/update" => update_shadow(self, req).await,
      "/pigeon/token/refresh" => refresh_token(self, req).await,
      "/pigeon/internal/psk" => get_coap_psk_internal(self, req).await,
      "/pigeon/delete" => delete(self, req).await,
      "/pigeon/shell/execute" => execute_shell_command(self, req).await,
      _ => Response::error("Not Found", 404),
    }
  }

  /// Runs for every frame on a hibernation-accepted socket, including ones
  /// that woke this DO from eviction. Auth happened once, at
  /// `accept_websocket_device`, and isn't re-checked per frame. A frame
  /// gets the connection closed rather than processed when it isn't text
  /// (the protocol is JSON-text-only), exceeds `MAX_WS_FRAME_BYTES`, or
  /// fails the flood check.
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

    // Only the two frames that tally a billable message are fused; a
    // paused account can still ping and still answer a shell command,
    // neither of which is billed or stored.
    if frame.is_billable() && device_ingest_paused(self).await {
      console_error!("WS: ingest paused for pigeon {}, closing", self.state.id());
      let _ = ws.close(
        Some(WS_CLOSE_INGEST_PAUSED),
        Some("free tier message allowance exhausted"),
      );
      return Ok(());
    }

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
      WsInboundFrame::ShellOutput {
        request_id,
        output,
        exit_code,
        truncated,
      } => handle_ws_shell_output(self, &request_id, output, exit_code, truncated),
    }

    Ok(())
  }

  /// Must be overridden once any socket is accepted -- the trait default
  /// panics via `unimplemented!()`.
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
    clear_shell_waiters(self, "socket closed");
    Ok(())
  }

  /// Same as `websocket_close` -- without this override a transport error
  /// panics the DO instead of just logging.
  async fn websocket_error(&self, _ws: WebSocket, error: worker::Error) -> Result<()> {
    console_error!("WS error for pigeon {}: {error}", self.state.id());
    clear_shell_waiters(self, "socket error");
    Ok(())
  }
}

/// Drops every pending `shell_waiters` entry when the device's socket goes
/// away mid-command -- dropping a `oneshot::Sender` resolves its paired
/// `Receiver` with `Err(Canceled)`, so the awaiting request fails fast
/// instead of sitting out its timeout. Clearing the whole map is safe:
/// only one device socket per pigeon ever exists
/// (`accept_websocket_device`), so there's no second connection whose
/// waiters this could wrongly cancel.
fn clear_shell_waiters(pigeons: &Pigeons, reason: &str) {
  let mut waiters = pigeons.shell_waiters.borrow_mut();
  if !waiters.is_empty() {
    console_log!(
      "Shell: clearing {} pending waiter(s) for pigeon {} ({reason})",
      waiters.len(),
      pigeons.state.id()
    );
    waiters.clear();
  }
}

/// Closes every open device-class socket for this pigeon with `code`/
/// `reason`. Shared by the socket-steal path in `accept_websocket_device`
/// and the token-revoke/pigeon-delete paths below -- all three are the same
/// operation (evict whoever is currently holding this pigeon's one device
/// socket), just triggered by different events and reported with a
/// different code. `get_websockets_with_tag` returns at most one entry in
/// practice (one socket per pigeon is enforced at accept), but this walks
/// the whole set rather than assuming that invariant holds.
fn close_device_sockets(pigeons: &Pigeons, code: u16, reason: &str) {
  for socket in pigeons.state.get_websockets_with_tag(WS_DEVICE_TAG) {
    let _ = socket.close(Some(code), Some(reason));
  }
}

/// The two access levels a dashboard-facing DO route can require -- the
/// literal role string `"owner"` on a matching `pigeon_acl` row is the
/// only special-cased value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AclLevel {
  Member,
  Owner,
}

/// Every dashboard-facing check in this DO funnels through here (the
/// gateway-side counterpart is `helpers/orgs.rs::authorize_flock`), so a
/// future central-authz swap stays contained to one function.
///
/// Principal set: `X-User-Id` (the Kratos identity the gateway resolved)
/// plus every org id in the gateway-injected `X-Org-Roles` header. A
/// `pigeon_acl` row matches when its `entity_id` equals any principal --
/// org-shared pigeons simply carry a row whose `entity_id` IS the org id.
///
/// For org-matched rows the caller's role in the org caps what the row
/// confers: `owner`/`admin` may exercise up to the row's own role,
/// `member` is capped at member-level regardless. Full matrix in
/// `docs/api.md`.
///
/// Both headers are internal, gateway-set values (this DO is never
/// internet-reachable; `proxy_to_pigeon_do` builds them from the validated
/// session) -- a device or demo request never reaches this function.
fn authorize_dashboard(
  pigeons: &Pigeons,
  req: &Request,
  level: AclLevel,
) -> Result<(), Result<Response, worker::Error>> {
  let Ok(Some(requesting_user)) = req.headers().get("X-User-Id") else {
    return Err(Response::error("Request missing 'X-User-Id'", 400));
  };

  let org_roles: Vec<capsules::OrgRoleEntry> = req
    .headers()
    .get("X-Org-Roles")
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default();

  let rows = pigeons
    .sql
    .exec("SELECT entity_id, role FROM pigeon_acl;", None)
    .map_err(Err)?
    .to_array::<PigeonAcl>()
    .map_err(Err)?;

  let user_uuid = uuid::Uuid::parse_str(&requesting_user).ok();

  let allowed = rows.iter().any(|row| {
    if Some(row.entity_id) == user_uuid {
      // Direct per-user grant.
      return match level {
        AclLevel::Member => true,
        AclLevel::Owner => row.role == "owner",
      };
    }
    // Org grant: the row's entity_id must be an org the caller belongs
    // to, and the caller's role in that org caps what the row confers.
    let Some(entry) = org_roles.iter().find(|e| e.id == row.entity_id) else {
      return false;
    };
    match level {
      AclLevel::Member => true,
      AclLevel::Owner => row.role == "owner" && entry.role.is_manager(),
    }
  });

  if allowed {
    Ok(())
  } else {
    Err(match level {
      AclLevel::Member => Response::error("Forbidden: You do not have access to this Pigeon", 403),
      AclLevel::Owner => Response::error("Forbidden: Only owners can modify ACL", 403),
    })
  }
}

/// Thin wrappers -- all real logic lives in `authorize_dashboard`.
fn is_authorized(pigeons: &Pigeons, req: &Request) -> Result<(), Result<Response, worker::Error>> {
  authorize_dashboard(pigeons, req, AclLevel::Member)
}

fn is_owner(pigeons: &Pigeons, req: &Request) -> Result<(), Result<Response, worker::Error>> {
  authorize_dashboard(pigeons, req, AclLevel::Owner)
}

/// Strips every secret from a `Pigeon` before it leaves the DO via a GET
/// route -- the connector token/PSK and the telemetry endpoint's
/// `auth_token` are only ever returned by the request that sets them.
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

  // The caller's "own" ACL row: their per-user row when one exists, else
  // the org row their access came through -- an org-granted caller may
  // have no per-user row at all.
  let org_roles: Vec<capsules::OrgRoleEntry> = req
    .headers()
    .get("X-Org-Roles")
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default();

  let rows = match pigeons
    .sql
    .exec("SELECT entity_id, role FROM pigeon_acl;", None)
  {
    Ok(cursor) => match cursor.to_array::<PigeonAcl>() {
      Ok(rows) => rows,
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

  let user_uuid = uuid::Uuid::parse_str(&requesting_user).ok();
  let acl = rows
    .iter()
    .find(|r| Some(r.entity_id) == user_uuid)
    .or_else(|| {
      rows
        .iter()
        .find(|r| org_roles.iter().any(|e| e.id == r.entity_id))
    })
    .cloned();

  let Some(acl) = acl else {
    // Unreachable in practice: is_authorized above already matched one of
    // these same rows for this same principal set.
    console_error!("PigeonAcl lookup found no row for an authorized caller");
    return Response::error("Internal Server Error", 500);
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
    Connector::Coap(_) => {
      // Separate short PSK: the bearer token is too long to be a PSK on
      // constrained stacks (see mint_coap_psk). Minted here so one create/
      // refresh rotates both credentials together.
      let psk = match mint_coap_psk() {
        Ok(p) => p,
        Err(e) => {
          console_error!("CoAP PSK mint error: {e}");
          return Response::error("Internal Server Error", 500);
        }
      };
      Connector::Coap(CoapConfig {
        endpoint: build_coap_endpoint(&pigeons.env, &do_id),
        token: device_token,
        tls_psk_identity: Some(do_id.clone()),
        tls_psk_secret: Some(psk),
      })
    }
  };

  let connector_json = serde_json::to_string(&server_connector).unwrap_or_default();

  let pigeon = match pigeons.sql.exec(
  &format!("INSERT INTO pigeons (id, flock_id, serial, name, tags, connector, token_expires_at, device_public_key, board) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING {PIGEON_COLUMNS};"),
  vec![
    do_id.clone().into(),
    row.flock_id.to_string().into(),
    row.serial.into(),
    row.name.into(),
    row.tags.into(),
    connector_json.into(),
    expires_at.unix_timestamp().into(),
    public_key.into(),
    row.board.into(),
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

  if let Err(e) = pigeons.sql.exec(
    "INSERT INTO pigeon_acl (entity_id, role) VALUES (?, 'owner');",
    vec![user_id.into()],
  ) {
    console_error!("Pigeon ACL create execution error: {e}");
    return Response::error("Internal Server Error", 500);
  }

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

  // Read the current pigeon to keep its connector type.
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

  pigeon.connector = match &pigeon.connector {
    Connector::Https(_) => {
      let endpoint = build_http_endpoint(&pigeons.env, &do_id);
      Connector::Https(HttpsConfig {
        endpoint,
        token: device_token.clone(),
      })
    }
    Connector::Coap(_) => {
      let psk = match mint_coap_psk() {
        Ok(p) => p,
        Err(e) => {
          console_error!("CoAP PSK mint error: {e}");
          return Response::error("Internal Server Error", 500);
        }
      };
      Connector::Coap(CoapConfig {
        endpoint: build_coap_endpoint(&pigeons.env, &do_id),
        token: device_token,
        tls_psk_identity: Some(do_id.clone()),
        tls_psk_secret: Some(psk),
      })
    }
  };

  let connector_json = serde_json::to_string(&pigeon.connector).map_err(|e| {
    console_error!("Connector serialization error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  // Overwriting device_public_key is what revokes the previous token: the
  // old signature can never verify again, regardless of the token's own
  // embedded expiry.
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
      // The old key is already unverifiable at this point, but a socket
      // opened under it was authenticated once, at accept, not per frame
      // (see `websocket_message`) -- without this it would keep receiving
      // `shadow_update` pushes and relaying `shell_cmd` on the credential
      // this refresh was meant to kill. The device's WS client reconnects
      // on any close and re-authenticates with whatever token it currently
      // has, which is where a stale reconnect actually gets refused.
      close_device_sockets(pigeons, WS_CLOSE_TOKEN_REVOKED, "token revoked");

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
    }
    Err(e) => {
      console_error!("Pigeon token refresh error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Trusted-internal PSK lookup for the CoAP terminator (`loft`) --
/// dispatched at `/pigeon/internal/psk`, reached only from the gateway's
/// service-secret-gated `GET /internal/coap-psk/:pigeon_id` route (this DO
/// is never internet-reachable, same trust argument as
/// `grant_acl_internal` above; the gateway has already verified the
/// terminator's shared service secret before dispatching here). Returns
/// this pigeon's PSK pair plus its bearer token -- the ONE deliberate
/// exception to the strip-on-read rule for connector secrets, because a
/// PSK terminator can neither complete a handshake without the key nor act
/// upstream for the device without the token. Possession grants exactly
/// "act as this one device": every proxied request still passes the device
/// routes' own `verify_device_token` check end-to-end. 404 for an
/// Https-connector pigeon (nothing to hand out) and for a deleted/
/// never-created pigeon's empty DO (via `one_row`).
async fn get_coap_psk_internal(pigeons: &Pigeons, _req: Request) -> Result<Response> {
  let pigeon = match pigeons.sql.exec(
    &format!("SELECT {PIGEON_COLUMNS} FROM pigeons LIMIT 1;"),
    None,
  ) {
    Ok(cursor) => match one_row::<PigeonRow>(&cursor) {
      Ok(p) => Pigeon::from(p),
      Err(_) => {
        // Empty DO: unknown identity. No console_error -- this is the
        // expected outcome for a probe of a nonexistent pigeon id.
        return Response::error("Not Found", 404);
      }
    },
    Err(e) => {
      console_error!("Pigeons READ error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  match pigeon.connector {
    Connector::Coap(CoapConfig {
      tls_psk_identity: Some(identity),
      tls_psk_secret: Some(secret),
      token,
      ..
    }) => Response::from_json(&capsules::CoapPskLookup {
      identity,
      secret,
      token,
    }),
    _ => Response::error("Not Found", 404),
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

  // No FK to `pigeons`, so unlike `pigeon_shadow` this doesn't cascade off
  // the delete below -- a wiped pigeon would otherwise keep serving its old
  // readings to whoever the ACL let back in.
  if let Err(e) = pigeons
    .sql
    .exec("DELETE FROM pigeon_telemetry_latest;", None)
  {
    console_error!("Pigeon telemetry delete execution error: {e}");
    return Response::error("Internal Server Error", 500);
  }

  match pigeons.sql.exec(
    "DELETE FROM pigeons WHERE id = ?;",
    vec![pigeons.state.id().to_string().into()],
  ) {
    Ok(_) => {
      // The storage wipe above doesn't end an already-open socket -- a
      // hibernating connection has no tie to the SQL rows it reads/writes,
      // so left alone it would linger, still answering `ping` and silently
      // no-oping `telemetry`/`shadow_report` against now-empty tables,
      // until it happened to drop on its own. Close it explicitly so a
      // deleted pigeon stops looking connected.
      close_device_sockets(pigeons, WS_CLOSE_PIGEON_DELETED, "pigeon deleted");
      Response::ok("Pigeon Deleted")
    }
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

  let connector_json = row
    .connector
    .map(|c| serde_json::to_string(&c).unwrap_or_default());

  match pigeons.sql.exec(
    "UPDATE pigeons SET
      flock_id = COALESCE(?, flock_id),
      serial = COALESCE(?, serial),
      name = COALESCE(?, name),
      tags = COALESCE(?, tags),
      connector = COALESCE(?, connector),
      board = COALESCE(?, board)
    WHERE id = ?;",
    vec![
      row.flock_id.map(|u| u.to_string()).into(),
      row.serial.into(),
      row.name.into(),
      row.tags.into(),
      connector_json.into(), // None becomes SQL NULL, Some becomes JSON text
      row.board.into(),
      pigeons.state.id().to_string().into(),
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
    // The gateway parses this response as JSON `PigeonAcl` (same success
    // shape as the other DO write handlers) -- a plain-text body here
    // makes that parse fail and 500s the caller after the write already
    // succeeded.
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

/// Same write as `update_acl` but with NO `is_owner` gate of its own.
/// Safe because Durable Objects have no public address and the only
/// dispatcher of this path is this Worker's own gateway
/// (`helpers/pigeons.rs::grant_org_acl_via_do`), which authorizes first.
/// Exists because the authorizing user may hold no ACL row on this pigeon
/// at all (their right comes from the flock/org side), so the owner-gated
/// `update_acl` route can't serve these flows.
async fn grant_acl_internal(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  let acl = match req.json::<PigeonAclUpdateRequest>().await {
    Ok(data) => data,
    Err(e) => {
      console_error!("ACL GRANT json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  match pigeons.sql.exec(
    "INSERT INTO pigeon_acl (entity_id, role) VALUES (?, ?)
     ON CONFLICT(entity_id) DO UPDATE SET role = excluded.role;",
    vec![acl.entity_id.to_string().into(), acl.role.clone().into()],
  ) {
    Ok(_) => Response::from_json(&PigeonAcl {
      entity_id: acl.entity_id,
      role: acl.role,
    }),
    Err(e) => {
      console_error!("ACL GRANT execution error: {e}");
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

/// Device auth for the `/pigeon/device/*` routes. No `X-User-Id`/ACL — a
/// device has no Kratos user identity. Verifies the bearer token against
/// this pigeon's own stored `device_public_key`, which only this DO holds.
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

/// Whether this pigeon's account is a free-tier one that has spent its
/// monthly message allowance, in which case the caller must refuse the
/// report rather than store it.
///
/// Checked in here, not at the gateway, for the surfaces that reach the DO
/// with auth and write in one hop: this is the only point that is both
/// after `is_authorized_device` -- so a caller without a device token
/// never learns anything about the account's billing state -- and before
/// the write. The WS surfaces have no gateway to check at all. The
/// HTTP/CoAP telemetry route is the exception and keeps its own gateway
/// check, since it verifies auth in a separate hop and never reaches these
/// handlers.
///
/// Fail-open lives inside `check_ingest_fuse`, along with the note
/// on why Hyperdrive's result cache makes a per-report check affordable.
async fn device_ingest_paused(pigeons: &Pigeons) -> bool {
  matches!(
    crate::helpers::check_ingest_fuse(&pigeons.env, &pigeons.state.id().to_string()).await,
    crate::helpers::IngestFuse::Pause
  )
}

/// Device confirms it applied `target_config` by writing its own
/// `current_config` plus the `target_version` it applied (echoed into
/// `current_version`) — never re-derived from `target_version` here, since
/// the device may still be catching up to a newer target.
async fn report_shadow_device(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  if device_ingest_paused(pigeons).await {
    return Response::error(crate::helpers::INGEST_PAUSED_MESSAGE, 429);
  }

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

/// Shared SQL for a device's shadow report-back, used by both the HTTP
/// route and the WS `shadow_report` frame. SQL only -- callers own their
/// auth and any Postgres sync/response shape around it.
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

/// Shadow read with no request/response of its own to shape around --
/// backs `accept_websocket_device`'s initial push.
fn read_shadow(pigeons: &Pigeons) -> Result<PigeonShadow> {
  let cursor = pigeons.sql.exec(
    "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
    None,
  )?;
  one_row::<PigeonShadowRow>(&cursor).map(PigeonShadow::from)
}

/// Device WebSocket upgrade -- the real-time channel for non-cellular
/// devices instead of polling HTTPS. Bearer auth happens exactly once,
/// here, BEFORE the socket is accepted; there is no per-frame re-check
/// (see `DurableObject::websocket_message` above).
///
/// Accepted via the hibernation API (`accept_websocket_with_tags`), not
/// the in-memory `WebSocket::accept()` -- an idle connection survives DO
/// eviction between messages, while `accept()` would keep this DO pinned
/// in memory (billed) for the whole connection. Tagged `WS_DEVICE_TAG` so
/// another socket class can coexist without either class's close/broadcast
/// logic touching the other's sockets.
///
/// Pushes an immediate `shadow_update` snapshot on the fresh socket so a
/// reconnecting device doesn't wait for the next dashboard PUT (or fall
/// back to its own HTTPS GET) to catch up on a `target_config` it missed
/// while disconnected. Sending on `pair.server` before the upgrade
/// response returns is fine -- `accept_websocket_with_tags` only registers
/// the socket, it doesn't gate sending. Best-effort like
/// `broadcast_shadow_update`: a failed read or send is logged, never fails
/// the upgrade.
async fn accept_websocket_device(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  // Refusing the upgrade is what actually makes the fuse hold on this
  // transport. The per-frame check in `websocket_message` only closes a
  // socket that was already open when the allowance ran out; without this,
  // the device would reconnect immediately and be closed again on its next
  // frame, forever. A 429 at the upgrade instead hands it the same signal
  // the HTTP ingest routes do, which its own back-off already understands.
  if device_ingest_paused(pigeons).await {
    return Response::error(crate::helpers::INGEST_PAUSED_MESSAGE, 429);
  }

  // One active device socket per pigeon: a new connection (e.g. a device
  // reconnecting after a network blip, before its old socket has timed out)
  // replaces the old one rather than coexisting with it.
  close_device_sockets(pigeons, 4009, "replaced by new connection");

  let pair = WebSocketPair::new()?;
  pigeons
    .state
    .accept_websocket_with_tags(&pair.server, &[WS_DEVICE_TAG]);

  match read_shadow(pigeons) {
    Ok(shadow) => {
      if let Err(e) = pair.server.send(&WsOutboundFrame::ShadowUpdate { shadow }) {
        console_error!(
          "WS accept: initial shadow push failed for pigeon {}: {e}",
          pigeons.state.id()
        );
      }
    }
    Err(e) => {
      console_error!(
        "WS accept: shadow read failed for pigeon {}: {e}",
        pigeons.state.id()
      );
    }
  }

  Response::from_websocket(pair.client)
}

#[derive(serde::Deserialize)]
struct ShellExecuteRequest {
  cmd: String,
  #[serde(default)]
  timeout_ms: Option<u32>,
}

#[derive(serde::Serialize)]
struct ShellExecuteResponse {
  output: String,
  exit_code: i32,
  truncated: bool,
}

// Plain constants, not per-env `wrangler.toml` vars -- protocol-level
// limits have no reason to differ by environment.
const SHELL_TIMEOUT_DEFAULT_MS: u64 = 10_000;
const SHELL_TIMEOUT_MAX_MS: u64 = 30_000;

/// Remote shell relay: sends one `ShellCmd` frame down the device's
/// existing WS connection, waits for the matching `ShellOutput` reply (or
/// a timeout), and returns it as an ordinary HTTP response. The dashboard
/// side is a plain HTTP client, not a second WebSocket.
///
/// Gated by `is_owner`, not `is_authorized` -- a shell command is RCE on
/// physical hardware by design.
///
/// The waiter handoff is race-free without a lock: this WASM environment
/// is single-threaded and async tasks only interleave at `.await` points,
/// so nothing runs between the waiter insert and the `select!` below.
/// Once we await the oneshot, the runtime may run the `websocket_message`
/// call that resolves it -- the intended handoff, not a race.
async fn execute_shell_command(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_owner(pigeons, &req));

  let body = match req.json::<ShellExecuteRequest>().await {
    Ok(b) => b,
    Err(e) => {
      console_error!("Shell EXECUTE json parse error: {e}");
      return Response::error("Bad Request: Invalid JSON", 400);
    }
  };

  if body.cmd.trim().is_empty() {
    return Response::error("Bad Request: Empty command", 400);
  }

  // No socket means no live channel to relay over -- a cellular/HTTPS-only
  // device, or a WS-capable one that isn't currently connected.
  let Some(ws) = pigeons
    .state
    .get_websockets_with_tag(WS_DEVICE_TAG)
    .into_iter()
    .next()
  else {
    return Response::error("Conflict: Device has no open WebSocket connection", 409);
  };

  // One command in flight at a time per pigeon, mirrored by a depth-1
  // queue on the device -- refuse rather than send a `ShellCmd` the device
  // will just reject anyway.
  if !pigeons.shell_waiters.borrow().is_empty() {
    return Response::error(
      "Conflict: A shell command is already in flight for this pigeon",
      409,
    );
  }

  let request_id = uuid::Uuid::now_v7().to_string();
  let timeout_ms = body
    .timeout_ms
    .map(u64::from)
    .unwrap_or(SHELL_TIMEOUT_DEFAULT_MS)
    .min(SHELL_TIMEOUT_MAX_MS);

  let (tx, rx) = oneshot::channel::<ShellOutputPayload>();
  pigeons
    .shell_waiters
    .borrow_mut()
    .insert(request_id.clone(), tx);

  // Log-only audit trail: user, pigeon, and command text, before the
  // command is ever sent to the device.
  let requesting_user = req.headers().get("X-User-Id").ok().flatten();
  console_log!(
    "Shell EXEC: pigeon {} user={:?} request_id={request_id} cmd={:?}",
    pigeons.state.id(),
    requesting_user,
    body.cmd
  );

  if let Err(e) = ws.send(&WsOutboundFrame::ShellCmd {
    request_id: request_id.clone(),
    cmd: body.cmd,
  }) {
    pigeons.shell_waiters.borrow_mut().remove(&request_id);
    console_error!(
      "Shell EXEC: send failed for pigeon {}: {e}",
      pigeons.state.id()
    );
    return Response::error("Internal Server Error", 500);
  }

  let mut reply = rx.fuse();
  let mut timeout = worker::Delay::from(std::time::Duration::from_millis(timeout_ms)).fuse();

  futures::select! {
    result = reply => match result {
      Ok(payload) => {
        console_log!(
          "Shell EXEC: pigeon {} request_id={request_id} exit_code={} truncated={}",
          pigeons.state.id(),
          payload.exit_code,
          payload.truncated
        );
        Response::from_json(&ShellExecuteResponse {
          output: payload.output,
          exit_code: payload.exit_code,
          truncated: payload.truncated,
        })
      }
      // Sender dropped without sending -- `clear_shell_waiters` ran
      // because the socket closed/errored while this request was waiting.
      Err(_) => Response::error("Bad Gateway: Device disconnected before replying", 502),
    },
    _ = timeout => {
      pigeons.shell_waiters.borrow_mut().remove(&request_id);
      console_error!(
        "Shell EXEC: timed out after {timeout_ms}ms for pigeon {} request_id={request_id}",
        pigeons.state.id()
      );
      Response::error("Gateway Timeout: Device did not reply in time", 504)
    }
  }
}

/// Resolves the `shell_waiters` entry for an inbound `shell_output`
/// frame. No matching waiter (already timed out, or a stray/duplicate
/// reply) is logged and dropped.
fn handle_ws_shell_output(
  pigeons: &Pigeons,
  request_id: &str,
  output: String,
  exit_code: i32,
  truncated: bool,
) {
  match pigeons.shell_waiters.borrow_mut().remove(request_id) {
    Some(sender) => {
      let _ = sender.send(ShellOutputPayload {
        output,
        exit_code,
        truncated,
      });
    }
    None => {
      console_error!(
        "Shell OUTPUT: no waiter for request_id={request_id} on pigeon {} (late or duplicate reply)",
        pigeons.state.id()
      );
    }
  }
}

/// WS counterpart to `report_shadow_device`, reusing
/// `write_shadow_report`. No gateway route is in the loop to sync Postgres
/// afterward (frames go straight into this DO), so this does the
/// best-effort sync itself. Errors are logged and swallowed -- there's no
/// HTTP response to carry them back, and a failed report shouldn't kill
/// the connection.
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

  // An accepted report-back is one billable device message, matching the
  // HTTP report-back route and the WS telemetry frame (whose tally rides
  // the shared queue consumer). Same best-effort convention as the PG
  // sync below: a failed tally undercounts, never disturbs the socket.
  crate::helpers::count_billable_message(&pigeons.env, &pigeon_id).await;

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

/// WS counterpart to the HTTP telemetry route. Upserts the DO's own
/// latest-value store synchronously first (like every telemetry entry
/// point), then either enqueues onto `TELEMETRY_QUEUE` for the shared
/// consumer path (see `queue.rs`), or -- with no queue bound (dev) --
/// writes history directly so WS telemetry doesn't silently skip what the
/// HTTP route would have recorded. No auth round trip: the bearer token
/// was verified once, at socket accept.
async fn handle_ws_telemetry(
  pigeons: &Pigeons,
  metrics: std::collections::HashMap<String, String>,
) {
  if metrics.is_empty() {
    return;
  }

  // A telemetry frame has no reply of its own (see `docs/api.md`), so an
  // over-cap frame is logged and dropped where the HTTP route would answer
  // 400. Dropping one frame beats closing a working socket over a report
  // the device can simply send fewer keys in.
  if let Err(message) = crate::helpers::validate_telemetry_report(&metrics) {
    console_error!(
      "WS telemetry: rejected report from pigeon {}: {message}",
      pigeons.state.id()
    );
    return;
  }

  // `write_telemetry` captures the previous values before merging this
  // report in. In the queue-bound branch below that capture rides the
  // outgoing `TelemetryMessage` -- recomputing it in `queue.rs`, after the
  // write just below, would see the *new* value where the previous one is
  // needed, silently defeating RateOfChange for every WS-sourced report.
  let Ok(previous_values) = write_telemetry(pigeons, &metrics) else {
    return;
  };

  let pigeon_id = pigeons.state.id().to_string();

  match pigeons.env.queue("TELEMETRY_QUEUE") {
    Ok(queue) => {
      let Ok(metrics_json) = serde_json::to_string(&metrics) else {
        console_error!("WS telemetry: failed to serialize metrics for pigeon {pigeon_id}");
        return;
      };

      // Pre-serialized to a JSON string for the same reason as
      // `metrics_json` -- a raw `HashMap` field hits the
      // serde-wasm-bindgen -> JS `Map` -> `JSON.stringify` == `{}` bug.
      // On a (shouldn't-happen) serialization failure, `None` makes
      // `queue.rs` fall back to the HTTP-sourced path for that one message
      // rather than dropping a report that already landed in this DO.
      let previous_values_json = match serde_json::to_string(&previous_values) {
        Ok(json) => Some(json),
        Err(e) => {
          console_error!(
            "WS telemetry: failed to serialize previous_values for pigeon {pigeon_id}: {e}"
          );
          None
        }
      };

      let message = TelemetryMessage {
        pigeon_id: pigeon_id.clone(),
        metrics_json,
        reported_at_ms: Date::now().as_millis(),
        previous_values_json,
      };

      if queue.send(message).await.is_err() {
        console_error!("WS telemetry: enqueue failed for pigeon {pigeon_id}");
      }
    }
    Err(e) => {
      // A frame has no reply of its own, so the HTTP route's 500 has no
      // spelling here -- but the fault it exists to surface is the same
      // one, and taking the dev fallback silently in a deployed
      // environment would move every WS report's history write back onto
      // the socket's own critical path. Loud and dropped is what makes a
      // lost binding visible; the DO's own latest-value store was already
      // written above, so the dashboard still shows current values while
      // only history is missing.
      if !crate::helpers::is_local_dev(&pigeons.env) {
        console_error!(
          "WS telemetry: TELEMETRY_QUEUE binding unavailable in a deployed environment ({e}); dropping the history write for pigeon {pigeon_id}"
        );
        return;
      }

      // No TELEMETRY_QUEUE bound in this environment (dev) -- match the
      // HTTP route's no-queue fallback by writing the default history
      // target directly instead of silently dropping it.
      let reported_at_ms = Date::now().as_millis();
      if let Err(e) =
        crate::helpers::write_telemetry_default(&pigeons.env, &pigeon_id, &metrics, reported_at_ms)
          .await
      {
        console_error!("WS telemetry: default write failed for pigeon {pigeon_id}: {e}");
      }

      // Best-effort, same as the default write above.
      if let Err(e) = crate::helpers::check_telemetry_alerts(
        &pigeons.env,
        &pigeon_id,
        &metrics,
        &previous_values,
        reported_at_ms,
      )
      .await
      {
        console_error!("WS telemetry: alert evaluation failed for pigeon {pigeon_id}: {e}");
      }
    }
  }
}

#[derive(serde::Deserialize)]
struct TargetConfigRow {
  target_config: String,
}

/// Reads this pigeon's `target_config` and extracts the `firmware` key so
/// the gateway can resolve which R2 object to stream in one DO round trip
/// — the firmware bytes themselves never pass through this DO (SQLite is
/// no place for MB-sized blobs). 404 when no firmware is assigned.
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

/// One telemetry key's stored value + timestamp from immediately before
/// this report's merge overwrote it -- feeds the `RateOfChange` alert
/// condition, which needs the prior value the merge replaces.
/// `reported_at` is the *previous* report's own unix-seconds timestamp,
/// used to enforce `RateOfChange::window_secs`. Doubles as the per-key
/// entry of the stored blob itself (`helpers::TelemetryBlob`), since a
/// key's stored entry is exactly what the next report hands the evaluator.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct PreviousTelemetryValue {
  pub value: String,
  pub reported_at: i64,
}

#[derive(serde::Deserialize)]
struct TelemetryBlobRow {
  metrics: String,
}

#[derive(serde::Deserialize)]
struct SqliteTableName {
  #[allow(dead_code)]
  name: String,
}

/// The whole latest-value store in one row read. A blob that fails to parse
/// is logged and treated as empty rather than failing the request: the
/// stored text is already unreadable, and refusing every later report would
/// not bring it back -- this way the next report rebuilds the store.
fn read_telemetry_blob(sql: &SqlStorage) -> Result<TelemetryBlob> {
  let cursor = sql.exec(
    "SELECT metrics FROM pigeon_telemetry_latest WHERE id = 1;",
    None,
  )?;
  // `to_array`, not `one()` (see `one_row`): a pigeon that has never
  // reported telemetry has no row here at all.
  let Some(row) = cursor.to_array::<TelemetryBlobRow>()?.into_iter().next() else {
    return Ok(TelemetryBlob::new());
  };

  match crate::helpers::parse_blob(&row.metrics) {
    Ok(blob) => Ok(blob),
    Err(e) => {
      console_error!("Telemetry blob parse error: {e}");
      Ok(TelemetryBlob::new())
    }
  }
}

fn write_telemetry_blob(sql: &SqlStorage, blob: &TelemetryBlob) -> Result<()> {
  let metrics = serde_json::to_string(blob)
    .map_err(|e| worker::Error::RustError(format!("telemetry blob serialize failed: {e}")))?;

  // Bound as a parameter rather than formatted into the statement text:
  // DO SQLite caps a statement at 100 KB, but a bound value only has to
  // stay under the 2 MB row limit, which `MAX_TELEMETRY_KEYS` and the
  // per-value cap already keep it far below.
  sql.exec(
    "INSERT INTO pigeon_telemetry_latest (id, metrics, updated_at)
     VALUES (1, ?, unixepoch())
     ON CONFLICT(id) DO UPDATE SET metrics = excluded.metrics, updated_at = unixepoch();",
    vec![metrics.into()],
  )?;
  Ok(())
}

/// One report's entire write: read the store, capture the previous values
/// for exactly the keys about to be overwritten, merge this report in, write
/// it back. Reading once and using that same read for both jobs is the point
/// of the single-row shape -- the previous values `RateOfChange` needs are
/// already in hand, so they cost no extra read. Keys the report doesn't
/// carry keep their own value and timestamp (see `helpers::merge_report`).
fn write_telemetry(
  pigeons: &Pigeons,
  metrics: &HashMap<String, String>,
) -> Result<HashMap<String, PreviousTelemetryValue>> {
  let mut blob = read_telemetry_blob(&pigeons.sql)?;
  let previous = crate::helpers::previous_values(&blob, metrics);

  // Unix seconds, matching what `unixepoch()` wrote when this was a table
  // of rows, so nothing downstream had to change how it reads a timestamp.
  let now_secs = (Date::now().as_millis() / 1000) as i64;
  crate::helpers::merge_report(&mut blob, metrics, now_secs);

  write_telemetry_blob(&pigeons.sql, &blob)?;
  Ok(previous)
}

/// Folds a DO provisioned before the single-row store into it and drops the
/// per-key table. There is no fleet-wide migration step and no maintenance
/// window: every pigeon's DO does this on its next wake, production pigeons
/// included, so it runs against live data and has to survive being
/// interrupted. Order is fold-then-drop, and `fold_legacy_rows` lets the
/// blob win per key, so a rerun that finds both tables resumes instead of
/// overwriting newer values with the legacy copies. Every failure path
/// leaves the legacy table alone for the next wake to retry rather than
/// dropping data it could not carry over.
fn migrate_legacy_telemetry(sql: &SqlStorage) {
  let legacy_present = match sql.exec(
    "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'pigeon_telemetry';",
    None,
  ) {
    Ok(cursor) => match cursor.to_array::<SqliteTableName>() {
      Ok(rows) => !rows.is_empty(),
      Err(e) => {
        console_error!("Telemetry migration: legacy table probe failed: {e}");
        return;
      }
    },
    Err(e) => {
      console_error!("Telemetry migration: legacy table probe failed: {e}");
      return;
    }
  };

  if !legacy_present {
    return;
  }

  let rows = match sql.exec(
    "SELECT key, value, reported_at FROM pigeon_telemetry;",
    None,
  ) {
    Ok(cursor) => match cursor.to_array::<LegacyTelemetryRow>() {
      Ok(rows) => rows,
      Err(e) => {
        console_error!("Telemetry migration: legacy READ error: {e}");
        return;
      }
    },
    Err(e) => {
      console_error!("Telemetry migration: legacy READ error: {e}");
      return;
    }
  };

  if !rows.is_empty() {
    let mut blob = match read_telemetry_blob(sql) {
      Ok(blob) => blob,
      Err(e) => {
        console_error!("Telemetry migration: blob READ error: {e}");
        return;
      }
    };
    let folded = rows.len();
    crate::helpers::fold_legacy_rows(&mut blob, rows);

    if let Err(e) = write_telemetry_blob(sql, &blob) {
      console_error!("Telemetry migration: blob WRITE error: {e}");
      return;
    }
    console_log!("Telemetry migration: folded {folded} legacy rows into the latest-value blob");
  }

  if let Err(e) = sql.exec("DROP TABLE IF EXISTS pigeon_telemetry;", None) {
    console_error!("Telemetry migration: legacy DROP error: {e}");
  }
}

/// Device-facing telemetry ingestion: auth + write in one call, used where
/// no telemetry queue is bound. Where one *is* bound the gateway calls
/// `verify_telemetry_device` + enqueues instead, and the queue consumer
/// reaches `write_telemetry_device` below. Body is a flat JSON object of
/// string key/value pairs (matches the device library's wire shape). Since
/// this only runs in the no-queue case, it also best-effort writes history
/// directly -- same fallback as `handle_ws_telemetry` -- so the HTTP route
/// doesn't silently skip history the queue would have recorded.
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

  if let Err(message) = crate::helpers::validate_telemetry_report(&metrics) {
    return Response::error(message, 400);
  }

  let Ok(previous_values) = write_telemetry(pigeons, &metrics) else {
    return Response::error("Internal Server Error", 500);
  };

  let pigeon_id = pigeons.state.id().to_string();
  let reported_at_ms = Date::now().as_millis();
  if let Err(e) =
    crate::helpers::write_telemetry_default(&pigeons.env, &pigeon_id, &metrics, reported_at_ms)
      .await
  {
    console_error!("HTTP telemetry: default write failed for pigeon {pigeon_id}: {e}");
  }

  // Best-effort, same as the default write above.
  if let Err(e) = crate::helpers::check_telemetry_alerts(
    &pigeons.env,
    &pigeon_id,
    &metrics,
    &previous_values,
    reported_at_ms,
  )
  .await
  {
    console_error!("HTTP telemetry: alert evaluation failed for pigeon {pigeon_id}: {e}");
  }

  Response::from_json(&metrics)
}

/// Verifies the device's bearer token WITHOUT writing anything, so the
/// gateway can confirm a report is genuine before it reaches the queue --
/// the queue itself has no authentication of its own. No response body;
/// the caller only inspects the status code.
async fn verify_telemetry_device(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));
  Response::ok("")
}

/// Response shape of `write_telemetry_device` below -- the trusted-internal
/// write path with NO auth check of its own. Safe because it's reachable
/// only from this Worker's own queue consumer (`src/queue.rs`), which only
/// dispatches messages that passed `verify_telemetry_device` at enqueue
/// time; Durable Objects have no public address. Only HTTP-sourced queue
/// messages land there -- WS-sourced ones already upserted synchronously
/// in `handle_ws_telemetry` and go to `read_telemetry_endpoint_device`
/// instead.
///
/// Besides confirming what got written, this hands the queue consumer the
/// pigeon's `telemetry_endpoint` (forward externally vs. write our own PG
/// history) and `previous_values` (for `RateOfChange`) without a second
/// DO round trip.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TelemetryWriteResult {
  pub metrics: std::collections::HashMap<String, String>,
  pub telemetry_endpoint: Option<TelemetryEndpoint>,
  pub previous_values: std::collections::HashMap<String, PreviousTelemetryValue>,
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

  // The gateway already refused an over-cap report before enqueueing it;
  // this is the same check on the trusted-internal path, so a message that
  // somehow reaches the consumer without it can't grow the stored blob past
  // what eviction expects.
  if let Err(message) = crate::helpers::validate_telemetry_report(&metrics) {
    return Response::error(message, 400);
  }

  let Ok(previous_values) = write_telemetry(pigeons, &metrics) else {
    return Response::error("Internal Server Error", 500);
  };

  Response::from_json(&TelemetryWriteResult {
    metrics,
    telemetry_endpoint: read_telemetry_endpoint(pigeons),
    previous_values,
  })
}

/// Deliberately its own small type rather than reusing
/// `TelemetryWriteResult`, so the WS-sourced queue path can't accidentally
/// read a `metrics`/`previous_values` field this route never populates.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TelemetryEndpointLookup {
  pub telemetry_endpoint: Option<TelemetryEndpoint>,
}

/// Read-only counterpart to `write_telemetry_device` for WS-originated
/// queue messages. `handle_ws_telemetry` already upserted synchronously
/// and captured the true previous values before enqueueing -- re-running
/// the upsert here would be redundant, and re-reading "previous" values
/// would see the value already written. Fetches only `telemetry_endpoint`,
/// the one other piece of state the queue consumer needs. No auth check
/// for the same reason as `write_telemetry_device`: reachable only from
/// our own queue consumer, never the internet.
async fn read_telemetry_endpoint_device(pigeons: &Pigeons, _req: Request) -> Result<Response> {
  Response::from_json(&TelemetryEndpointLookup {
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
/// /pigeons/:id/telemetry` in `lib.rs`) -- every key currently in this DO's
/// own latest-value store, unlike the history routes which read
/// `pigeon_telemetry_history` from Postgres.
async fn get_telemetry_latest(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  match read_telemetry_blob(&pigeons.sql) {
    Ok(blob) => Response::from_json(&crate::helpers::to_latest(&blob)),
    Err(e) => {
      console_error!("Telemetry latest READ error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Unauthenticated counterpart to `get_telemetry_latest`, backing the
/// public demo route. Safe: this DO is never internet-reachable, and the
/// gateway only proxies here after confirming the pigeon is in
/// `DEMO_PIGEON_IDS`.
async fn get_telemetry_latest_demo(pigeons: &Pigeons, _req: Request) -> Result<Response> {
  match read_telemetry_blob(&pigeons.sql) {
    Ok(blob) => Response::from_json(&crate::helpers::to_latest(&blob)),
    Err(e) => {
      console_error!("Telemetry latest READ error (demo): {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Dashboard-facing setter for the per-pigeon telemetry forwarding
/// target. `is_authorized`, not `is_owner` -- any ACL entry may configure
/// it. A `None` body clears the endpoint: a direct `SET`, not `COALESCE`,
/// so `None` truly means NULL -- there is no "leave unchanged" for this
/// single-field route.
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
    vec![endpoint_json.into(), pigeons.state.id().to_string().into()],
  ) {
    Ok(_) => Response::from_json(&body.telemetry_endpoint),
    Err(e) => {
      console_error!("Telemetry endpoint UPDATE execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}

/// Ring-buffer cap for `pigeon_log_chunks` -- chunks are small (capped at
/// `MAX_LOG_CHUNK_BYTES` on the way in), but an unbounded stream would
/// grow this DO's SQLite storage without limit.
const MAX_STORED_LOG_CHUNKS: i64 = 200;

/// Device-facing log chunk ingestion. The body is the raw binary chunk,
/// not JSON -- the gateway forwards it via `proxy_binary_to_pigeon_do`;
/// the UTF-8 text proxy would corrupt arbitrary bytes. Stored as base64
/// text rather than a BLOB column since it's handed back to the dashboard
/// as base64 anyway.
async fn report_logs_device(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized_device(pigeons, &req));

  if device_ingest_paused(pigeons).await {
    return Response::error(crate::helpers::INGEST_PAUSED_MESSAGE, 429);
  }

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

  // Prune beyond the ring-buffer cap, oldest first. Non-fatal -- the
  // chunk itself is already durably stored.
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

/// Every stored chunk, oldest first, as base64 for host-side decode --
/// Zephyr's dictionary-log tooling decodes against the firmware's own
/// ELF, which the backend has no access to.
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

#[derive(serde::Deserialize)]
struct PigeonBoardFlockRow {
  flock_id: String,
  board: Option<String>,
}

/// Board/geometry compatibility check for firmware assignments: an image
/// built for one board's flash/partition geometry, assigned to a device
/// with a different one, halts the device (TF-M fails safe, but only
/// after the device already tried to apply the image). MCUboot's image
/// header carries no usable geometry fact for swap-mode builds
/// (`ih_load_addr` is unset), so the check compares metadata declared by
/// both sides -- this pigeon's `board` column vs. `flock_firmware.board`
/// looked up by the sha256 in the incoming `target_config.firmware` --
/// before the shadow write is ever accepted.
///
/// Fail-closed on purpose: pigeon board unset, image board unset, no
/// matching `flock_firmware` row for this sha256 in this pigeon's flock,
/// or an explicit mismatch all reject with 400. Only a confirmed match
/// passes -- both the pigeon and the image must be tagged with a board
/// before assigning firmware at all.
async fn check_firmware_board_compat(
  pigeons: &Pigeons,
  firmware_value: &serde_json::Value,
) -> Result<(), Result<Response, worker::Error>> {
  let target = match serde_json::from_value::<FirmwareTarget>(firmware_value.clone()) {
    Ok(t) => t,
    Err(e) => {
      console_error!("Shadow UPDATE: malformed firmware target: {e}");
      return Err(Response::error(
        "Bad Request: Malformed firmware target",
        400,
      ));
    }
  };

  let pigeon_row = match pigeons
    .sql
    .exec("SELECT flock_id, board FROM pigeons LIMIT 1;", None)
  {
    Ok(cursor) => match one_row::<PigeonBoardFlockRow>(&cursor) {
      Ok(row) => row,
      Err(e) => {
        console_error!("Shadow UPDATE: pigeon board READ error: {e}");
        return Err(Response::error("Internal Server Error", 500));
      }
    },
    Err(e) => {
      console_error!("Shadow UPDATE: pigeon board READ error: {e}");
      return Err(Response::error("Internal Server Error", 500));
    }
  };

  let client = match crate::helpers::get_db_client(&pigeons.env).await {
    Ok(client) => client,
    Err(e) => {
      console_error!("Shadow UPDATE: board check skipped, Hyperdrive connection failed: {e}");
      return Err(Response::error("Internal Server Error", 500));
    }
  };

  let image_board =
    match crate::helpers::get_firmware_board(&client, &pigeon_row.flock_id, &target.sha256).await {
      Ok(board) => board,
      Err(e) => {
        console_error!("Shadow UPDATE: firmware board lookup failed: {e}");
        return Err(Response::error("Internal Server Error", 500));
      }
    };

  match (&pigeon_row.board, &image_board) {
    (Some(p), Some(i)) if p == i => Ok(()),
    (p, i) => {
      console_error!(
        "Shadow UPDATE: REJECTED firmware assignment for pigeon {} -- pigeon board={p:?}, image board={i:?} (sha256={})",
        pigeons.state.id(),
        target.sha256
      );
      Err(Response::error(
        "Bad Request: Firmware/pigeon board mismatch or unset (fail-closed) -- tag both the pigeon and the firmware image with a matching board before assigning",
        400,
      ))
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

  // Only when this PUT's target_config actually carries a `firmware` key;
  // every other shadow write (telemetry_interval, log, reboot, ...) is
  // unaffected.
  if let Some(firmware_value) = shadow.target_config.get("firmware") {
    unwrap_or_return_response!(check_firmware_board_compat(pigeons, firmware_value).await);
  }

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
/// any -- without this, a device only learns about a new `target_config`
/// on its next poll. Scoped to `WS_DEVICE_TAG` so other socket classes
/// never receive a frame meant for the device protocol. Best-effort: a
/// `send` failure is logged and ignored -- the shadow write already
/// succeeded and is the primary result of this request.
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
