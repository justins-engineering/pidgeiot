use crate::objects::{mint_device_credential, verify_device_token};
use capsules::{
  CoapConfig, Connector, HttpsConfig, Pigeon, PigeonAcl, PigeonAclUpdateRequest,
  PigeonCreateRequest, PigeonDetail, PigeonRow, PigeonShadow, PigeonShadowReportRequest,
  PigeonShadowRow, PigeonShadowUpdateRequest, PigeonUpdateRequest, TelemetryEndpoint,
  TelemetryLatest, TelemetryLatestRow, unwrap_or_return_response,
};
use worker::{
  DurableObject, Env, Request, Response, ResponseBuilder, Result, SqlStorage, State, console_error,
  durable_object, wasm_bindgen,
};

/// Selected pigeon column list shared by every `pigeons` read/RETURNING
/// statement -- keeps `telemetry_endpoint` (and any future column) from
/// silently going missing from one of the several near-identical queries.
const PIGEON_COLUMNS: &str = "id, flock_id, serial, name, tags, connector, token_expires_at, telemetry_endpoint, updated_at, created_at";

const HTTP_ENDPOINT: &str = "https://api.pidgeiot.com/device/pigeons/";
const COAP_ENDPOINT: &str = "coaps+tcp://api.pidgeiot.com/device/pigeons/";

#[inline]
pub fn build_http_endpoint(do_id: &str) -> String {
  let mut endpoint = String::with_capacity(HTTP_ENDPOINT.len() + 64);
  endpoint.push_str(HTTP_ENDPOINT);
  endpoint.push_str(do_id);
  endpoint
}

#[inline]
pub fn build_coap_endpoint(do_id: &str) -> String {
  let mut endpoint = String::with_capacity(COAP_ENDPOINT.len() + 64);
  endpoint.push_str(COAP_ENDPOINT);
  endpoint.push_str(do_id);
  endpoint
}

#[durable_object]
pub struct Pigeons {
  sql: SqlStorage,
  #[allow(unused)]
  state: State,
  #[allow(unused)]
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
      "/pigeon/device/telemetry" => report_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/verify" => verify_telemetry_device(self, req).await,
      "/pigeon/device/telemetry/write" => write_telemetry_device(self, req).await,
      "/pigeon/telemetry/get" => get_telemetry_latest(self, req).await,
      "/pigeon/telemetry-endpoint/update" => update_telemetry_endpoint(self, req).await,
      "/pigeon/authz/check" => check_authorized(self, req).await,
      "/pigeon/shadow/update" => update_shadow(self, req).await,
      "/pigeon/token/refresh" => refresh_token(self, req).await,
      "/pigeon/delete" => delete(self, req).await,
      _ => Response::error("Not Found", 404),
    }
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
      endpoint: build_http_endpoint(&do_id),
      token: device_token,
    }),
    Connector::Coap(_) => Connector::Coap(CoapConfig {
      endpoint: build_coap_endpoint(&do_id),
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
      let endpoint = build_http_endpoint(&do_id);
      Connector::Https(HttpsConfig {
        endpoint,
        token: device_token.clone(),
      })
    }
    Connector::Coap(_) => {
      let endpoint = build_coap_endpoint(&do_id);
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
    vec![acl.entity_id.to_string().into(), acl.role.into()],
  ) {
    Ok(_) => Response::ok("ACL Updated"),
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

  let config_str = serde_json::to_string(&report.current_config).map_err(|e| {
    console_error!("Shadow config serialization error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  match pigeons.sql.exec(
    "UPDATE pigeon_shadow SET current_config = ?, current_version = ? WHERE id = (SELECT id FROM pigeons LIMIT 1);",
    vec![config_str.into(), report.current_version.into()],
  ) {
    Ok(_) => match pigeons.sql.exec(
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
    },
    Err(e) => {
      console_error!("Shadow REPORT execution error: {e}");
      Response::error("Internal Server Error", 500)
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
/// consumer (`src/queue.rs`) reaches `write_telemetry_device` below.
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
#[derive(serde::Serialize)]
struct TelemetryWriteResult {
  metrics: std::collections::HashMap<String, String>,
  telemetry_endpoint: Option<TelemetryEndpoint>,
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
    Err(e) => {
      console_error!("Shadow UPDATE execution error: {e}");
      Response::error("Internal Server Error", 500)
    }
  }
}
