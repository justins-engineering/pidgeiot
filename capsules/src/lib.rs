use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

static SERVICE_PLAN: &str = "free";
static CONNECTOR: &str = "HTTPS";

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Flock {
  pub id: String,
  pub user_id: String,
  pub name: String,
  pub service_plan: Option<String>,
  pub pigeon_count: i64,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub updated_at: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub created_at: Option<OffsetDateTime>,
}

impl Default for Flock {
  fn default() -> Flock {
    Flock {
      id: String::with_capacity(64),
      user_id: String::with_capacity(64),
      name: String::with_capacity(64),
      service_plan: Some(SERVICE_PLAN.to_string()),
      pigeon_count: i64::default(),
      updated_at: Option::default(),
      created_at: Option::default(),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
pub struct CreateFlockPayload {
  pub name: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Pigeon {
  pub id: i64,
  pub flock_id: i64,
  pub name: String,
  pub serial: Option<String>,
  pub tags: Option<String>,
  pub connector: Option<String>,
  pub location: Option<String>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub last_connected: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub updated_at: Option<OffsetDateTime>,
  #[serde(default, with = "time::serde::rfc3339::option")]
  pub created_at: Option<OffsetDateTime>,
}

impl Default for Pigeon {
  fn default() -> Pigeon {
    Pigeon {
      id: i64::default(),
      flock_id: i64::default(),
      name: String::with_capacity(64),
      serial: Option::default(),
      tags: Option::default(),
      connector: Some(CONNECTOR.to_string()),
      location: Option::default(),
      last_connected: Option::default(),
      updated_at: Option::default(),
      created_at: Option::default(),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
pub struct PigeonMessage {
  pub id: i64,
  pub pigeon_id: i64,
  pub message: String,
  pub timestamp: i64,
}
