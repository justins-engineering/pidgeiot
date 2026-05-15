use capsules::Flock;
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Error, Result};

pub async fn get_user_flocks(client: &Client, user_id_str: &str) -> Result<Vec<Flock>> {
  let parsed_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid UUID format: {e}")))?;

  // Use query_typed and explicitly declare the parameter as a UUID
  let rows = client
    .query_typed(
      "SELECT
         id, user_id, name, service_plan, created_at, updated_at,
         (SELECT COUNT(*) FROM pigeons WHERE flock_id = flocks.id) as pigeon_count
       FROM flocks WHERE user_id = $1",
      &[(&parsed_uuid, Type::UUID)], // <-- Type supplied here
    )
    .await
    .map_err(|e| Error::RustError(format!("DB Query Error: {e}")))?;

  let mut flocks = Vec::new();

  for row in rows {
    let id: Uuid = row.get("id");
    let u_id: Uuid = row.get("user_id");
    let name: String = row.get("name");
    let service_plan: String = row.get("service_plan");
    // Subqueries returning COUNT() in Postgres always return an i64
    let pigeon_count: i64 = row.get("pigeon_count");
    let updated_at: Option<time::OffsetDateTime> = row.get("updated_at");
    let created_at: Option<time::OffsetDateTime> = row.get("created_at");

    flocks.push(Flock {
      id: id.to_string(),
      user_id: u_id.to_string(),
      name,
      service_plan: Some(service_plan),
      pigeon_count,
      updated_at,
      created_at,
    });
  }

  Ok(flocks)
}

/// Inserts a new flock into the database and returns the fully populated record
pub async fn create_user_flock(
  client: &Client,
  user_id_str: &str,
  flock_name: &str,
) -> Result<Flock> {
  let parsed_uuid = Uuid::parse_str(user_id_str)
    .map_err(|e| Error::RustError(format!("Invalid UUID format: {e}")))?;

  // Use query_typed_one for single row inserts
  let row = client
    .query_typed_one(
      "INSERT INTO flocks (user_id, name, service_plan)
       VALUES ($1, $2, 'free')
       RETURNING id, user_id, name, service_plan, created_at, updated_at",
      &[
        (&parsed_uuid, Type::UUID),
        (&flock_name, Type::TEXT), // explicitly tell Postgres this is a TEXT column
      ],
    )
    .await
    .map_err(|e| Error::RustError(format!("Failed to insert flock: {e}")))?;

  // Extract the newly generated data
  let id: Uuid = row.get("id");
  let u_id: Uuid = row.get("user_id");
  let name: String = row.get("name");
  let service_plan: String = row.get("service_plan");
  let updated_at: Option<time::OffsetDateTime> = row.get("updated_at");
  let created_at: Option<time::OffsetDateTime> = row.get("created_at");

  Ok(Flock {
    id: id.to_string(),
    user_id: u_id.to_string(),
    name,
    service_plan: Some(service_plan),
    pigeon_count: 0,
    updated_at,
    created_at,
  })
}
