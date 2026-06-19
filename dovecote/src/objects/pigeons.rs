use crate::objects::sign_device_token;
use capsules::{
  CoapConfig, Connector, HttpsConfig, Pigeon, PigeonAcl, PigeonAclUpdateRequest,
  PigeonCreateRequest, PigeonDetail, PigeonRow, PigeonShadow, PigeonShadowRow,
  PigeonShadowUpdateRequest, PigeonUpdateRequest, unwrap_or_return_response,
};
use worker::{
  DurableObject, Env, Request, Response, ResponseBuilder, Result, SqlStorage, State, console_error,
  durable_object, wasm_bindgen,
};

const HTTP_ENDPOINT: &str = "https://api.pidgeiot.com/device/pigeons/";
const COAP_ENDPOINT: &str = "coaps://api.pidgeiot.com/device/pigeons/";

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
      "/pigeon/shadow/update" => update_shadow(self, req).await,
      "/pigeon/token/refresh" => refresh_token(self, req).await,
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

async fn get(pigeons: &Pigeons, req: Request) -> Result<Response> {
  unwrap_or_return_response!(is_authorized(pigeons, &req));

  match pigeons.sql.exec(
    "SELECT id, flock_id, serial, name, tags, connector, token_expires_at, updated_at, created_at FROM pigeons LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match cursor.one::<PigeonRow>() {
      Ok(p) => {
        let mut pigeon = Pigeon::from(p);

        // Strip token for security — never return it except on create/refresh
        pigeon.connector = match pigeon.connector {
          Connector::Https(c) => Connector::Https(HttpsConfig {
            endpoint: c.endpoint,
            token: String::new(),
          }),
          Connector::Coap(c) => Connector::Coap(CoapConfig {
            endpoint: c.endpoint,
            token: String::new(),
            dtls_psk_identity: c.dtls_psk_identity,
            dtls_psk_secret: None,
          }),
        };

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
    "SELECT id, flock_id, serial, name, tags, connector, token_expires_at, updated_at, created_at FROM pigeons LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match cursor.one::<PigeonRow>() {
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

  // Strip token for security — never return it except on create/refresh
  pigeon.connector = match pigeon.connector {
    Connector::Https(c) => Connector::Https(HttpsConfig {
      endpoint: c.endpoint,
      token: String::new(),
    }),
    Connector::Coap(c) => Connector::Coap(CoapConfig {
      endpoint: c.endpoint,
      token: String::new(),
      dtls_psk_identity: c.dtls_psk_identity,
      dtls_psk_secret: None,
    }),
  };

  let shadow = match pigeons.sql.exec(
    "SELECT target_version, current_version, target_config, current_config, updated_at FROM pigeon_shadow LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match cursor.one::<PigeonShadowRow>() {
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
    Ok(cursor) => match cursor.one::<PigeonAcl>() {
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

  let (device_token, expires_at) = match sign_device_token(&do_id, &pigeons.env) {
    Ok(t) => t,
    Err(e) => {
      console_error!("JWT signing error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  let server_connector = match row.connector {
    Connector::Https(_) => Connector::Https(HttpsConfig {
      endpoint: format!("https://api.pidgeiot.com/device/pigeons/{}", do_id),
      token: device_token,
    }),
    Connector::Coap(_) => Connector::Coap(CoapConfig {
      endpoint: format!("coaps://api.pidgeiot.com/device/pigeons/{}", do_id),
      token: device_token.clone(),
      dtls_psk_identity: Some(do_id.clone()),
      dtls_psk_secret: Some(device_token),
    }),
  };

  let connector_json = serde_json::to_string(&server_connector).unwrap_or_default();

  let pigeon = match pigeons.sql.exec(
  "INSERT INTO pigeons (id, flock_id, serial, name, tags, connector, token_expires_at) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id, flock_id, serial, name, tags, connector, token_expires_at, updated_at, created_at;",
  vec![
    do_id.clone().into(),
    row.flock_id.to_string().into(),
    row.serial.into(),
    row.name.into(),
    row.tags.into(),
    connector_json.into(),
    expires_at.unix_timestamp().into(),
  ],
) {
    Ok(cursor) => match cursor.one::<PigeonRow>() {
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
    Ok(cursor) => match cursor.one::<PigeonShadowRow>() {
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

  let (device_token, expires_at) = match sign_device_token(&do_id, &pigeons.env) {
    Ok(t) => t,
    Err(e) => {
      console_error!("JWT signing error: {e}");
      return Response::error("Internal Server Error", 500);
    }
  };

  // Read current pigeon to determine connector type
  let mut pigeon = match pigeons.sql.exec(
    "SELECT id, flock_id, serial, name, tags, connector, token_expires_at, updated_at, created_at FROM pigeons LIMIT 1;",
    None,
  ) {
    Ok(cursor) => match cursor.one::<PigeonRow>() {
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
        dtls_psk_identity: Some(do_id.clone()),
        dtls_psk_secret: Some(device_token),
      })
    }
  };

  // Serialize and update
  let connector_json = serde_json::to_string(&pigeon.connector).map_err(|e| {
    console_error!("Connector serialization error: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })?;

  match pigeons.sql.exec(
    "UPDATE pigeons SET connector = ?, token_expires_at = ? WHERE id = ?;",
    vec![connector_json.into(), expires_at.unix_timestamp().into(), do_id.into()],
  ) {
    Ok(_) => {
      match pigeons.sql.exec(
        "SELECT id, flock_id, serial, name, tags, connector, token_expires_at, updated_at, created_at FROM pigeons LIMIT 1;",
        None,
      ) {
        Ok(cursor) => match cursor.one::<PigeonRow>() {
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
        "SELECT id, flock_id, serial, name, tags, connector, token_expires_at, updated_at, created_at FROM pigeons LIMIT 1;",
        None,
      ) {
        Ok(cursor) => match cursor.one::<PigeonRow>() {
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
    Ok(cursor) => match cursor.one::<PigeonAcl>() {
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
    Ok(cursor) => match cursor.one::<PigeonShadowRow>() {
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
        Ok(cursor) => match cursor.one::<PigeonShadowRow>() {
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
