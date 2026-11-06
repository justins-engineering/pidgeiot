//! The gateway and queue halves of flock-level config (`capsules::flock_config` holds the merge):
//! the Postgres layer on the `flocks` row, the checks a flock write runs against its members, the
//! fan-out of one apply message per member, and the decision each pigeon's Durable Object makes
//! when one arrives.

use std::sync::atomic::{AtomicBool, Ordering};

use capsules::{
  FirmwareTarget, FlockConfig, JsonString, NIDD_MAX_TARGET_CONFIG_BYTES, PigeonShadow,
  effective_text, firmware_from_flock, flock_firmware_allowed,
};
use serde_json::{Map, Value};
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Env, Error, Result, console_error};

use crate::helpers::missing_schema_objects;
use crate::helpers::rules::FIELD_PIGEONS;

/// The fan-out queue's binding. A queue of its own, so a large flock's fan-out never sits in
/// front of live telemetry.
pub const FLOCK_CONFIG_QUEUE: &str = "FLOCK_CONFIG_QUEUE";

/// Every environment's flock-config queue name starts with this; the one queue handler routes on
/// it.
pub const FLOCK_CONFIG_QUEUE_PREFIX: &str = "pidgeiot-flock-config";

/// Messages per `sendBatch` call, and the bytes one call may carry, kept under Cloudflare's
/// published 100 messages and 256 KB per batch.
const SEND_BATCH_MESSAGES: usize = 100;
const SEND_BATCH_BYTES: usize = 200 * 1024;

/// One member's apply, both the queue message and the body of the pigeon object's internal
/// `/pigeon/shadow/flock` route; the move route sends it to `/pigeon/flock/update` too.
///
/// Every field is a string or an integer: `Queue::send` serializes through serde-wasm-bindgen,
/// which turns a Rust map into a JS `Map` that the queue then stringifies as `{}`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct FlockConfigApply {
  pub pigeon_id: String,
  pub flock_id: String,
  pub flock_version: i32,
  /// The flock layer as stored.
  pub config_json: String,
  /// The board of the image the layer's `firmware` names, when it names one.
  #[serde(default)]
  pub firmware_board: Option<String>,
  /// Keys to delete from this pigeon's layer, a JSON array; `None` for none.
  #[serde(default)]
  pub adopt_json: Option<String>,
}

/// What a pigeon's object did with an apply, for the consumer: it mirrors `shadow` when one is
/// answered and acks every outcome, since none of them changes on a retry.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct FlockApplyAnswer {
  pub outcome: String,
  #[serde(default)]
  pub shadow: Option<PigeonShadow>,
  pub flock_version: i32,
}

/// The pigeon object's answer to a move: the pigeon, and the shadow the gateway mirrors.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct FlockMoveAnswer {
  pub pigeon: capsules::Pigeon,
  pub shadow: capsules::PigeonShadowView,
}

/// Whether `pigeon_id` is one of the production signs' pigeons, whose config is only ever set by
/// hand: a flock write over one is refused and an apply to one is dropped.
pub fn is_field_pigeon(pigeon_id: &str) -> bool {
  FIELD_PIGEONS.contains(&pigeon_id)
}

/// One member as a flock write checks it. `variant` is the connector's tag alone.
#[derive(serde::Deserialize, Debug, Clone, PartialEq)]
pub struct FlockMember {
  pub id: String,
  #[serde(default)]
  pub board: Option<String>,
  #[serde(default)]
  pub variant: Option<String>,
}

/// The checks a flock write runs against its members, as `(status, message)`: no field pigeon,
/// the NIDD floor when any member is a Nidd pigeon, and for a layer naming firmware
/// (`image_board` is `Some`) one tagged board across the image and every member.
pub fn check_flock_write(
  members: &[FlockMember],
  layer_bytes: usize,
  image_board: Option<Option<&str>>,
) -> std::result::Result<(), (u16, &'static str)> {
  if members.iter().any(|member| is_field_pigeon(&member.id)) {
    return Err((
      409,
      "Conflict: this flock holds a production sign whose config is only set by hand",
    ));
  }
  let has_nidd = members
    .iter()
    .any(|member| member.variant.as_deref() == Some("Nidd"));
  if has_nidd && layer_bytes > NIDD_MAX_TARGET_CONFIG_BYTES {
    return Err((
      413,
      "Payload Too Large: a flock holding a NIDD pigeon takes a target_config of at most 1319 bytes",
    ));
  }
  if let Some(image_board) = image_board {
    let boards: Vec<Option<&str>> = members.iter().map(|m| m.board.as_deref()).collect();
    if !flock_firmware_allowed(image_board, &boards) {
      return Err((
        400,
        "Bad Request: flock firmware needs the image and every pigeon tagged with one board",
      ));
    }
  }
  Ok(())
}

/// The `sha256` a layer's `firmware` names, `Ok(None)` for no firmware, `Err` for a malformed one.
pub fn layer_firmware_sha(layer: &Map<String, Value>) -> std::result::Result<Option<String>, ()> {
  match layer.get("firmware") {
    None => Ok(None),
    Some(value) => serde_json::from_value::<FirmwareTarget>(value.clone())
      .map(|target| Some(target.sha256))
      .map_err(|_| ()),
  }
}

/// One apply per member, all carrying the same layer.
pub fn apply_messages(
  flock_id: &Uuid,
  flock_version: i32,
  config_json: &str,
  firmware_board: Option<&str>,
  adopt: &[String],
  member_ids: &[String],
) -> Vec<FlockConfigApply> {
  let flock_id = flock_id.to_string();
  let adopt_json = (!adopt.is_empty()).then(|| Value::from(adopt.to_vec()).to_string());
  member_ids
    .iter()
    .map(|pigeon_id| FlockConfigApply {
      pigeon_id: pigeon_id.clone(),
      flock_id: flock_id.clone(),
      flock_version,
      config_json: config_json.to_string(),
      firmware_board: firmware_board.map(str::to_string),
      adopt_json: adopt_json.clone(),
    })
    .collect()
}

/// How many messages of `message_bytes` go in one `sendBatch` call.
pub fn send_batch_size(message_bytes: usize) -> usize {
  (SEND_BATCH_BYTES / message_bytes.max(1)).clamp(1, SEND_BATCH_MESSAGES)
}

/// Enqueues `applies` in as few `sendBatch` calls as the limits allow. An error means some may
/// not have been sent; a second write of the same layer re-sends them all.
pub async fn enqueue_flock_applies(env: &Env, applies: Vec<FlockConfigApply>) -> Result<()> {
  if applies.is_empty() {
    return Ok(());
  }
  let queue = env.queue(FLOCK_CONFIG_QUEUE)?;
  let size = send_batch_size(applies[0].config_json.len() + 512);
  let mut applies = applies.into_iter().peekable();
  while applies.peek().is_some() {
    let batch: Vec<FlockConfigApply> = applies.by_ref().take(size).collect();
    queue.send_batch(batch).await?;
  }
  Ok(())
}

/// What a pigeon's object makes of one apply, decided from what it holds.
pub struct ApplyInput<'a> {
  pub pigeon_id: &'a str,
  /// The message names this pigeon's flock, or the apply is a move into it.
  pub flock_matches: bool,
  /// The message's version is newer than the one stored, or the apply is a move.
  pub newer: bool,
  pub flock_layer: &'a Map<String, Value>,
  /// The stored pigeon layer; `None` when it is not a JSON object.
  pub pigeon_layer: Option<Map<String, Value>>,
  pub adopt: &'a [String],
  pub pigeon_board: Option<&'a str>,
  pub firmware_board: Option<&'a str>,
  /// For a Nidd pigeon, the largest `target_config` a write that moves its version may carry.
  pub nidd_cap: Option<usize>,
  pub current_target: &'a str,
}

#[derive(Debug, PartialEq)]
pub enum ApplyDecision {
  /// Nothing to do and nothing recorded: a field pigeon, or a stale or repeated message.
  Drop(&'static str),
  /// The layer cannot apply here; the reason is recorded and the old layer kept.
  Refuse(&'static str),
  Apply {
    pigeon_layer: Map<String, Value>,
    effective: String,
    changed: bool,
  },
}

pub const REFUSED_NOT_AN_OBJECT: &str = "this pigeon's own config is not a JSON object";
pub const REFUSED_BOARD: &str = "the flock's firmware is not built for this pigeon's board";
pub const REFUSED_NIDD_CAP: &str = "the merged config is too large for one NIDD downlink frame";

/// Decides one apply. A pigeon layer that names its own `firmware`, or opts out with `null`, is
/// not checked against the flock image's board: its own write ran the per-pigeon check.
pub fn decide_apply(input: ApplyInput<'_>) -> ApplyDecision {
  if is_field_pigeon(input.pigeon_id) {
    return ApplyDecision::Drop("field");
  }
  if !input.flock_matches || !input.newer {
    return ApplyDecision::Drop("stale");
  }
  let Some(mut pigeon_layer) = input.pigeon_layer else {
    return ApplyDecision::Refuse(REFUSED_NOT_AN_OBJECT);
  };
  for key in input.adopt {
    pigeon_layer.remove(key);
  }
  if firmware_from_flock(input.flock_layer, &pigeon_layer)
    && (input.firmware_board.is_none() || input.pigeon_board != input.firmware_board)
  {
    return ApplyDecision::Refuse(REFUSED_BOARD);
  }
  let effective = effective_text(input.flock_layer, &pigeon_layer);
  let changed = effective != input.current_target;
  if changed && input.nidd_cap.is_some_and(|cap| effective.len() > cap) {
    return ApplyDecision::Refuse(REFUSED_NIDD_CAP);
  }
  ApplyDecision::Apply {
    pigeon_layer,
    effective,
    changed,
  }
}

/// Set once this isolate has found the flock-config columns.
static FLOCK_CONFIG_READY: AtomicBool = AtomicBool::new(false);

/// Ensures `flocks.target_config`, `flocks.config_version` and `pigeon_shadow.flock_version`
/// once per isolate, logging them if missing: `infra/migrations/2026-11-06-flock-config.sql`
/// should already have created them.
pub async fn ensure_flock_config_columns(client: &Client) -> Result<()> {
  if FLOCK_CONFIG_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  let missing = missing_schema_objects(
    client,
    &[],
    &[
      ("flocks", "target_config"),
      ("flocks", "config_version"),
      ("pigeon_shadow", "flock_version"),
    ],
  )
  .await?;
  if missing.is_empty() {
    FLOCK_CONFIG_READY.store(true, Ordering::Relaxed);
    return Ok(());
  }
  console_error!(
    "ensure_flock_config_columns: creating missing {}",
    missing.join(", ")
  );
  client
    .batch_execute(
      "ALTER TABLE flocks ADD COLUMN IF NOT EXISTS target_config JSONB NOT NULL DEFAULT '{}';
       ALTER TABLE flocks ADD COLUMN IF NOT EXISTS config_version INTEGER NOT NULL DEFAULT 0;
       ALTER TABLE pigeon_shadow ADD COLUMN IF NOT EXISTS flock_version INTEGER NOT NULL DEFAULT 0;",
    )
    .await
    .map_err(|e| {
      console_error!("ensure_flock_config_columns failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  FLOCK_CONFIG_READY.store(true, Ordering::Relaxed);
  Ok(())
}

fn db_error(context: &str, e: tokio_postgres::Error) -> Error {
  console_error!("{context}: {e}");
  Error::RustError("Internal Server Error".into())
}

/// A flock's layer with its member counts, `None` for an unknown flock. A plain read, so the
/// counts may be served from Hyperdrive's cache for about a minute.
pub async fn read_flock_config(client: &Client, flock_id: &Uuid) -> Result<Option<FlockConfig>> {
  ensure_flock_config_columns(client).await?;
  let rows = client
    .query_typed(
      "SELECT f.target_config::text AS target_config, f.config_version,
         count(p.id) AS members,
         count(p.id) FILTER (WHERE s.flock_version = f.config_version) AS members_on_version
       FROM flocks f
       LEFT JOIN pigeons p ON p.flock_id = f.id
       LEFT JOIN pigeon_shadow s ON s.id = p.id
       WHERE f.id = $1
       GROUP BY f.id;",
      &[(flock_id, Type::UUID)],
    )
    .await
    .map_err(|e| db_error("Flock config read error", e))?;
  let Some(row) = rows.into_iter().next() else {
    return Ok(None);
  };
  let text: String = row.get("target_config");
  Ok(Some(FlockConfig {
    flock_id: *flock_id,
    target_config: JsonString::new(text).map_err(|e| Error::RustError(e.to_string()))?,
    config_version: row.get("config_version"),
    members: row.get("members"),
    members_on_version: row.get("members_on_version"),
    queued: None,
  }))
}

/// What a flock write stored, and the members it found under the flock's row lock.
pub struct FlockWrite {
  pub config_version: i32,
  pub members: Vec<FlockMember>,
  /// The board of the image the layer's `firmware` names, when it names a tagged one.
  pub image_board: Option<String>,
}

/// Writes a flock layer after `check` passes against the members, in one transaction:
/// the flock row is locked first, which waits out any pigeon create or move into the flock (both
/// take a key-share lock on it through their foreign key), so every member is listed and none
/// joins unlisted before the commit. The member list and the image's board come back from the
/// write statement itself, never from a cached read. `Ok(Err(..))` is a refusal, nothing written;
/// `Ok(Ok(None))` an unknown flock.
pub async fn write_flock_config<F>(
  client: &mut Client,
  flock_id: &Uuid,
  config_json: &str,
  firmware_sha: Option<&str>,
  check: F,
) -> Result<std::result::Result<Option<FlockWrite>, (u16, &'static str)>>
where
  F: FnOnce(&[FlockMember], Option<Option<&str>>) -> std::result::Result<(), (u16, &'static str)>,
{
  ensure_flock_config_columns(client).await?;
  crate::helpers::ensure_flock_firmware_table(client).await?;

  let tx = client
    .transaction()
    .await
    .map_err(|e| db_error("Flock config transaction error", e))?;

  tx.query_typed(
    "SELECT id FROM flocks WHERE id = $1 FOR UPDATE;",
    &[(flock_id, Type::UUID)],
  )
  .await
  .map_err(|e| db_error("Flock config lock error", e))?;

  let rows = tx
    .query_typed(
      "UPDATE flocks SET target_config = $2::jsonb, config_version = config_version + 1
       WHERE id = $1
       RETURNING config_version,
         (SELECT COALESCE(json_agg(json_build_object(
             'id', p.id,
             'board', p.board,
             'variant', (SELECT k FROM jsonb_object_keys(p.connector) AS k LIMIT 1)
           )), '[]')::text
          FROM pigeons p WHERE p.flock_id = $1) AS members,
         (SELECT board FROM flock_firmware WHERE flock_id = $1 AND sha256 = $3) AS image_board;",
      &[
        (flock_id, Type::UUID),
        (&config_json, Type::TEXT),
        (&firmware_sha, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| db_error("Flock config write error", e))?;

  let Some(row) = rows.into_iter().next() else {
    return Ok(Ok(None));
  };
  let members_text: String = row.get("members");
  let members: Vec<FlockMember> =
    serde_json::from_str(&members_text).map_err(|e| Error::RustError(e.to_string()))?;
  let image_board: Option<String> = row.get("image_board");

  if let Err(refused) = check(&members, firmware_sha.map(|_| image_board.as_deref())) {
    // Dropping the transaction rolls it back; nothing was committed.
    return Ok(Err(refused));
  }

  tx.commit()
    .await
    .map_err(|e| db_error("Flock config commit error", e))?;

  Ok(Ok(Some(FlockWrite {
    config_version: row.get("config_version"),
    members,
    image_board,
  })))
}

/// A flock's layer and version, `None` for an unknown flock. A plain read: a caller that must
/// not act on a stale one checks again with [`mirror_flock_apply_pg_db`]'s answer.
pub async fn read_flock_layer(client: &Client, flock_id: &Uuid) -> Result<Option<(String, i32)>> {
  ensure_flock_config_columns(client).await?;
  let rows = client
    .query_typed(
      "SELECT target_config::text AS target_config, config_version FROM flocks WHERE id = $1;",
      &[(flock_id, Type::UUID)],
    )
    .await
    .map_err(|e| db_error("Flock layer read error", e))?;
  Ok(
    rows
      .into_iter()
      .next()
      .map(|row| (row.get("target_config"), row.get("config_version"))),
  )
}

/// The board of the image a stored layer's `firmware` names in the flock's catalog, `None` when
/// it names none or the image is untagged.
pub async fn layer_firmware_board(
  client: &Client,
  flock_id: &Uuid,
  config_json: &str,
) -> Result<Option<String>> {
  let Some(layer) = capsules::parse_layer(config_json) else {
    return Ok(None);
  };
  match layer_firmware_sha(&layer) {
    Ok(Some(sha)) => crate::helpers::get_firmware_board(client, &flock_id.to_string(), &sha).await,
    _ => Ok(None),
  }
}

/// Mirrors a pigeon's shadow and the flock version it took, and answers the layer its flock
/// holds now. A write, so the answer is never a cached one: a move compares it with the version
/// it installed, which closes the window a flock write could otherwise slip through.
pub async fn mirror_flock_apply_pg_db(
  client: &Client,
  pigeon_id: &str,
  flock_id: &Uuid,
  shadow: &PigeonShadow,
  flock_version: i32,
) -> Result<Option<(String, i32)>> {
  ensure_flock_config_columns(client).await?;
  let rows = client
    .query_typed(
      "UPDATE pigeon_shadow SET
         target_version = $2,
         current_version = $3,
         target_config = $4::jsonb,
         current_config = $5::jsonb,
         updated_at = $6,
         flock_version = $7
       WHERE id = $1
       RETURNING
         (SELECT target_config::text FROM flocks WHERE id = $8) AS flock_config,
         (SELECT config_version FROM flocks WHERE id = $8) AS config_version;",
      &[
        (&pigeon_id, Type::TEXT),
        (&shadow.target_version, Type::INT4),
        (&shadow.current_version, Type::INT4),
        (&shadow.target_config.to_string(), Type::TEXT),
        (&shadow.current_config.to_string(), Type::TEXT),
        (&shadow.updated_at, Type::INT8),
        (&flock_version, Type::INT4),
        (flock_id, Type::UUID),
      ],
    )
    .await
    .map_err(|e| db_error("Postgres flock apply mirror error", e))?;
  Ok(rows.into_iter().next().and_then(|row| {
    let config: Option<String> = row.get("flock_config");
    let version: Option<i32> = row.get("config_version");
    config.zip(version)
  }))
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  const FIELD: &str = "fa8a80551f1818392b2929f8dc4cf0efb05155e7b7316692ce944eae6ce7a33f";
  const BOARD: &str = "circuitdojo_feather/nrf9160/ns";

  fn map(value: Value) -> Map<String, Value> {
    match value {
      Value::Object(map) => map,
      _ => panic!("test layer is not an object"),
    }
  }

  fn member(id: &str, board: Option<&str>, variant: &str) -> FlockMember {
    FlockMember {
      id: id.to_string(),
      board: board.map(str::to_string),
      variant: Some(variant.to_string()),
    }
  }

  fn input<'a>(flock: &'a Map<String, Value>, pigeon: Value, current: &'a str) -> ApplyInput<'a> {
    ApplyInput {
      pigeon_id: "p1",
      flock_matches: true,
      newer: true,
      flock_layer: flock,
      pigeon_layer: match pigeon {
        Value::Object(map) => Some(map),
        _ => None,
      },
      adopt: &[],
      pigeon_board: Some(BOARD),
      firmware_board: Some(BOARD),
      nidd_cap: None,
      current_target: current,
    }
  }

  #[test]
  fn every_field_pigeon_is_recognized_and_nothing_else() {
    for id in FIELD_PIGEONS {
      assert!(is_field_pigeon(id));
    }
    assert!(
      !is_field_pigeon(&FIELD[..8]),
      "matched whole, never by prefix"
    );
    assert!(!is_field_pigeon("p1"));
  }

  #[test]
  fn a_flock_write_refuses_a_field_pigeon_member() {
    let members = [member("p1", None, "Https"), member(FIELD, None, "Https")];
    assert_eq!(check_flock_write(&members, 10, None).unwrap_err().0, 409);
  }

  #[test]
  fn a_nidd_member_holds_the_layer_to_the_frame_floor() {
    let members = [member("p1", None, "Https"), member("p2", None, "Nidd")];
    assert!(check_flock_write(&members, NIDD_MAX_TARGET_CONFIG_BYTES, None).is_ok());
    assert_eq!(
      check_flock_write(&members, NIDD_MAX_TARGET_CONFIG_BYTES + 1, None)
        .unwrap_err()
        .0,
      413
    );
    let https = [member("p1", None, "Https")];
    assert!(check_flock_write(&https, 4096, None).is_ok());
  }

  #[test]
  fn flock_firmware_needs_every_member_on_the_image_board() {
    let same = [
      member("p1", Some(BOARD), "Https"),
      member("p2", Some(BOARD), "Coap"),
    ];
    assert!(check_flock_write(&same, 10, Some(Some(BOARD))).is_ok());
    assert_eq!(check_flock_write(&same, 10, Some(None)).unwrap_err().0, 400);
    let mixed = [
      member("p1", Some(BOARD), "Https"),
      member("p2", None, "Https"),
    ];
    assert_eq!(
      check_flock_write(&mixed, 10, Some(Some(BOARD)))
        .unwrap_err()
        .0,
      400
    );
    assert!(
      check_flock_write(&mixed, 10, None).is_ok(),
      "no firmware, no board rule"
    );
  }

  #[test]
  fn a_malformed_firmware_key_is_told_apart_from_none() {
    assert_eq!(layer_firmware_sha(&map(json!({}))), Ok(None));
    let good = json!({"firmware": {"version": "1.0.0", "size": 10, "sha256": "ab"}});
    assert_eq!(layer_firmware_sha(&map(good)), Ok(Some("ab".to_string())));
    assert!(layer_firmware_sha(&map(json!({"firmware": "1.0.0"}))).is_err());
  }

  #[test]
  fn one_message_per_member_with_the_adopt_list_as_text() {
    let flock = Uuid::nil();
    let ids = ["a".to_string(), "b".to_string()];
    let none = apply_messages(&flock, 3, "{}", None, &[], &ids);
    assert_eq!(none.len(), 2);
    assert_eq!(none[1].pigeon_id, "b");
    assert_eq!(none[0].adopt_json, None);

    let adopt = apply_messages(&flock, 3, "{}", Some(BOARD), &["stop_id".to_string()], &ids);
    assert_eq!(adopt[0].adopt_json.as_deref(), Some(r#"["stop_id"]"#));
    assert_eq!(adopt[0].firmware_board.as_deref(), Some(BOARD));
    let text = serde_json::to_string(&adopt[0]).unwrap();
    assert_eq!(
      serde_json::from_str::<FlockConfigApply>(&text).unwrap(),
      adopt[0]
    );
  }

  #[test]
  fn a_send_batch_stays_inside_both_limits() {
    assert_eq!(send_batch_size(600), 100);
    assert_eq!(send_batch_size(4096 + 512), 44);
    assert_eq!(send_batch_size(usize::MAX), 1);
    assert_eq!(send_batch_size(0), 100);
  }

  #[test]
  fn a_field_pigeon_drops_every_apply() {
    let flock = map(json!({"a": 1}));
    let mut field = input(&flock, json!({}), "{}");
    field.pigeon_id = FIELD;
    assert_eq!(decide_apply(field), ApplyDecision::Drop("field"));
  }

  #[test]
  fn a_message_for_another_flock_or_an_older_version_is_dropped() {
    let flock = map(json!({"a": 1}));
    let mut moved = input(&flock, json!({}), "{}");
    moved.flock_matches = false;
    assert_eq!(decide_apply(moved), ApplyDecision::Drop("stale"));
    let mut repeat = input(&flock, json!({}), "{}");
    repeat.newer = false;
    assert_eq!(decide_apply(repeat), ApplyDecision::Drop("stale"));
  }

  #[test]
  fn a_newer_layer_merges_and_reports_whether_the_text_changed() {
    let flock = map(json!({"telemetry_interval": 60}));
    let ApplyDecision::Apply {
      effective, changed, ..
    } = decide_apply(input(&flock, json!({"stop_id": "1"}), "{}"))
    else {
      panic!("not applied");
    };
    assert_eq!(effective, r#"{"stop_id":"1","telemetry_interval":60}"#);
    assert!(changed);

    let same = decide_apply(input(&flock, json!({"stop_id": "1"}), &effective));
    assert!(matches!(same, ApplyDecision::Apply { changed: false, .. }));
  }

  #[test]
  fn a_pigeon_that_overrides_the_changed_key_sees_no_change() {
    let flock = map(json!({"telemetry_interval": 30}));
    let current = r#"{"telemetry_interval":10}"#;
    let decision = decide_apply(input(&flock, json!({"telemetry_interval": 10}), current));
    assert!(matches!(
      decision,
      ApplyDecision::Apply { changed: false, .. }
    ));
  }

  #[test]
  fn adopt_deletes_the_named_keys_from_the_pigeon_layer() {
    let flock = map(json!({"telemetry_interval": 60, "stop_id": "flock"}));
    let mut adopting = input(
      &flock,
      json!({"telemetry_interval": 10, "stop_id": "7"}),
      "{}",
    );
    let adopt = ["telemetry_interval".to_string()];
    adopting.adopt = &adopt;
    let ApplyDecision::Apply {
      pigeon_layer,
      effective,
      ..
    } = decide_apply(adopting)
    else {
      panic!("not applied");
    };
    assert_eq!(pigeon_layer, map(json!({"stop_id": "7"})));
    assert_eq!(effective, r#"{"stop_id":"7","telemetry_interval":60}"#);
  }

  #[test]
  fn flock_firmware_is_checked_against_this_pigeons_board() {
    let flock = map(json!({"firmware": {"version": "1", "size": 1, "sha256": "a"}}));
    let mut other = input(&flock, json!({}), "{}");
    other.pigeon_board = Some("esp32c6_devkitc");
    assert_eq!(decide_apply(other), ApplyDecision::Refuse(REFUSED_BOARD));

    let mut untagged = input(&flock, json!({}), "{}");
    untagged.pigeon_board = None;
    untagged.firmware_board = None;
    assert_eq!(decide_apply(untagged), ApplyDecision::Refuse(REFUSED_BOARD));

    assert!(matches!(
      decide_apply(input(&flock, json!({}), "{}")),
      ApplyDecision::Apply { changed: true, .. }
    ));
  }

  #[test]
  fn a_pigeons_own_firmware_or_opt_out_skips_the_flock_board_check() {
    let flock = map(json!({"firmware": {"version": "1", "size": 1, "sha256": "a"}}));
    for pigeon in [
      json!({"firmware": {"version": "0", "size": 1, "sha256": "b"}}),
      json!({"firmware": null}),
    ] {
      let mut own = input(&flock, pigeon, "{}");
      own.pigeon_board = Some("esp32c6_devkitc");
      assert!(matches!(decide_apply(own), ApplyDecision::Apply { .. }));
    }
  }

  #[test]
  fn a_nidd_pigeon_refuses_a_merged_config_over_its_cap_only_when_it_changes() {
    let flock = map(json!({"pad": "x".repeat(40)}));
    let mut nidd = input(&flock, json!({"stop_id": "1"}), "{}");
    nidd.nidd_cap = Some(20);
    assert_eq!(decide_apply(nidd), ApplyDecision::Refuse(REFUSED_NIDD_CAP));

    let effective = effective_text(&flock, &map(json!({"stop_id": "1"})));
    let mut same = input(&flock, json!({"stop_id": "1"}), &effective);
    same.nidd_cap = Some(20);
    assert!(matches!(
      decide_apply(same),
      ApplyDecision::Apply { changed: false, .. }
    ));
  }

  #[test]
  fn a_pigeon_layer_that_is_not_an_object_refuses() {
    let flock = map(json!({"a": 1}));
    assert_eq!(
      decide_apply(input(&flock, json!([1]), "[1]")),
      ApplyDecision::Refuse(REFUSED_NOT_AN_OBJECT)
    );
  }
}
