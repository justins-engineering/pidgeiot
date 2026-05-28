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

pub fn deserialize_unix_to_datetime<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let raw_seconds = i64::deserialize(deserializer)?;

  OffsetDateTime::from_unix_timestamp(raw_seconds)
    .map_err(|err| serde::de::Error::custom(format!("Invalid Unix timestamp: {err}")))
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Flock {
  pub id: Uuid,
  pub user_id: Uuid,
  pub name: String,
  pub service_plan: String,
  pub pigeon_count: i64,
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
      pigeon_count: i64::default(),
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Pigeon {
  pub id: String,
  pub flock_id: Uuid,
  pub serial: Option<String>,
  pub name: Option<String>,
  pub tags: Option<String>,
  pub connector: String,
  #[serde(
    deserialize_with = "deserialize_unix_to_datetime",
    serialize_with = "time::serde::rfc3339::serialize"
  )]
  pub updated_at: OffsetDateTime,
  #[serde(
    deserialize_with = "deserialize_unix_to_datetime",
    serialize_with = "time::serde::rfc3339::serialize"
  )]
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
      connector: "HTTPS".to_string(),
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
  pub connector: String,
}

impl Default for PigeonCreateRequest {
  fn default() -> PigeonCreateRequest {
    PigeonCreateRequest {
      flock_id: Uuid::default(),
      serial: None,
      name: None,
      tags: None,
      connector: String::with_capacity(8),
    }
  }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PigeonCreateResponse {
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
  pub connector: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PigeonShadow {
  pub status: String,
  pub updated_at: i64,
  pub config: JsonString,
}

impl Default for PigeonShadow {
  fn default() -> PigeonShadow {
    PigeonShadow {
      status: "provisioning".to_string(),
      updated_at: i64::default(),
      config: JsonString("{}".to_string()),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PigeonShadowUpdateRequest {
  pub status: String,
  pub config: serde_json::Value,
}

impl Default for PigeonShadowUpdateRequest {
  fn default() -> PigeonShadowUpdateRequest {
    PigeonShadowUpdateRequest {
      status: String::with_capacity(16),
      config: serde_json::Value::default(),
    }
  }
}
