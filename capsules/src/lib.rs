use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[macro_export]
macro_rules! unwrap_or_return_response {
  ($expr:expr) => {
    match $expr {
      Ok(val) => val,
      Err(err_resp) => return err_resp,
    }
  };
}

pub fn deserialize_unix_float_to_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let raw = f64::deserialize(deserializer)?;
  Ok(raw as i64)
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Flock {
  pub id: Uuid,
  pub user_id: Uuid,
  pub name: String,
  pub service_plan: String,
  pub pigeon_ids: Vec<String>,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
}

impl Default for Flock {
  fn default() -> Flock {
    Flock {
      id: Uuid::default(),
      user_id: Uuid::default(),
      name: String::with_capacity(64),
      service_plan: "free".to_string(),
      pigeon_ids: Vec::default(),
      updated_at: OffsetDateTime::UNIX_EPOCH,
      created_at: OffsetDateTime::UNIX_EPOCH,
    }
  }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct FlockCreateRequest {
  pub name: String,
}

impl Default for FlockCreateRequest {
  fn default() -> FlockCreateRequest {
    FlockCreateRequest {
      name: String::with_capacity(64),
    }
  }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct FlockUpdateRequest {
  pub name: String,
  pub service_plan: String,
}

impl Default for FlockUpdateRequest {
  fn default() -> FlockUpdateRequest {
    FlockUpdateRequest {
      name: String::with_capacity(64),
      service_plan: String::with_capacity(8),
    }
  }
}

// DB model — deserializes from SQLite's integer timestamps
#[derive(Deserialize, Debug)]
pub struct PigeonRow {
  pub id: String,
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: String, // raw JSON text from SQLite
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub updated_at: i64,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub created_at: i64,
}

impl From<PigeonRow> for Pigeon {
  fn from(row: PigeonRow) -> Self {
    let connector = serde_json::from_str(&row.connector).unwrap_or_default();
    Self {
      id: row.id,
      flock_id: row.flock_id,
      serial: row.serial,
      name: row.name,
      tags: row.tags,
      connector,
      updated_at: OffsetDateTime::from_unix_timestamp(row.updated_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
      created_at: OffsetDateTime::from_unix_timestamp(row.created_at)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
    }
  }
}

// API model — serializes/deserializes as RFC 3339
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Pigeon {
  pub id: String,
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: Connector,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
}

impl Default for Pigeon {
  fn default() -> Pigeon {
    Pigeon {
      id: String::with_capacity(64),
      flock_id: Uuid::default(),
      serial: None,
      name: None,
      tags: None,
      connector: Connector::default(),
      updated_at: OffsetDateTime::UNIX_EPOCH,
      created_at: OffsetDateTime::UNIX_EPOCH,
    }
  }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PigeonCreateRequest {
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: Connector,
}

impl Default for PigeonCreateRequest {
  fn default() -> PigeonCreateRequest {
    PigeonCreateRequest {
      flock_id: Uuid::default(),
      serial: None,
      name: None,
      tags: None,
      connector: Connector::default(),
    }
  }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PigeonDetail {
  pub pigeon: Pigeon,
  pub acl: PigeonAcl,
  pub shadow: PigeonShadow,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct PigeonUpdateRequest {
  pub flock_id: Option<Uuid>,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: Option<Connector>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct PigeonAcl {
  pub entity_id: Uuid,
  pub role: String,
}

impl Default for PigeonAcl {
  fn default() -> PigeonAcl {
    PigeonAcl {
      entity_id: Uuid::default(),
      role: String::with_capacity(8),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PigeonAclUpdateRequest {
  pub entity_id: Uuid,
  pub role: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct JsonString(String);

impl JsonString {
  pub fn new(value: String) -> Result<Self, serde_json::Error> {
    serde_json::from_str::<serde_json::Value>(&value)?; // validate only
    Ok(Self(value))
  }

  pub fn into_inner(self) -> String {
    self.0
  }
}

impl std::fmt::Display for JsonString {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.0)
  }
}

impl JsonString {
  pub fn to_pretty(&self) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.0) {
      serde_json::to_string_pretty(&value).unwrap_or_else(|_| self.0.clone())
    } else {
      self.0.clone()
    }
  }
}

#[derive(Deserialize, Debug)]
pub struct PigeonShadowRow {
  pub target_version: i32,
  pub current_version: i32,
  pub target_config: JsonString,
  pub current_config: JsonString,
  #[serde(deserialize_with = "deserialize_unix_float_to_i64")]
  pub updated_at: i64,
}

impl From<PigeonShadowRow> for PigeonShadow {
  fn from(row: PigeonShadowRow) -> Self {
    Self {
      target_version: row.target_version,
      current_version: row.current_version,
      target_config: row.target_config,
      current_config: row.current_config,
      updated_at: row.updated_at,
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonShadow {
  pub target_version: i32,
  pub current_version: i32,
  pub target_config: JsonString,
  pub current_config: JsonString,
  pub updated_at: i64,
}

impl Default for PigeonShadow {
  fn default() -> PigeonShadow {
    PigeonShadow {
      target_version: i32::default(),
      current_version: i32::default(),
      target_config: JsonString("{}".to_string()),
      current_config: JsonString("{}".to_string()),
      updated_at: i64::default(),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PigeonShadowUpdateRequest {
  pub target_config: serde_json::Value,
}

impl Default for PigeonShadowUpdateRequest {
  fn default() -> PigeonShadowUpdateRequest {
    PigeonShadowUpdateRequest {
      target_config: serde_json::Value::default(),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct HttpsConfig {
  pub endpoint: String,
  pub token: String, // JWT — returned once, never stored again
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct CoapConfig {
  pub endpoint: String,
  pub token: String,
  pub dtls_psk_identity: Option<String>,
  pub dtls_psk_secret: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum Connector {
  Https(HttpsConfig),
  Coap(CoapConfig),
}

impl Default for Connector {
  fn default() -> Self {
    Connector::Https(HttpsConfig {
      endpoint: String::new(),
      token: String::new(),
    })
  }
}
