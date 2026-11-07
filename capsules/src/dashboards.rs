//! Custom dashboards: named, ordered pages of widgets owned by one person or
//! one organization, never both.
//!
//! A layout holds only references (flock ids, pigeon ids, telemetry keys,
//! ranges). Each widget fetches its own data with the viewer's session, so
//! sharing a dashboard inside an organization never shares data the viewer
//! could not already read.
//!
//! Every field added to a widget later must be `#[serde(default)]`: a stored
//! layout without it would otherwise fail to parse and blank the dashboard
//! for every member who opens it.

use crate::TelemetryLatest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use uuid::Uuid;

/// Widgets one layout may hold.
pub const MAX_DASHBOARD_WIDGETS: usize = 24;
/// Bytes one layout may serialize to, checked on the raw request body.
pub const MAX_DASHBOARD_LAYOUT_BYTES: usize = 32 * 1024;
/// Dashboards one owner (a person, or an organization) may hold.
pub const MAX_DASHBOARDS_PER_OWNER: i64 = 50;
/// Characters in a dashboard name or a widget title.
pub const MAX_DASHBOARD_NAME_CHARS: usize = 80;
/// Characters in a text widget.
pub const MAX_WIDGET_TEXT_CHARS: usize = 2_000;
/// Telemetry keys one widget may name.
pub const MAX_WIDGET_KEYS: usize = 8;
/// Characters in a widget id.
pub const MAX_WIDGET_ID_CHARS: usize = 32;
/// The layout version this crate writes and accepts.
pub const DASHBOARD_LAYOUT_VERSION: u32 = 1;

/// How much of the grid a widget spans: one column, two, or the full row.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WidgetSize {
  #[default]
  Small,
  Half,
  Full,
}

impl WidgetSize {
  pub const ALL: [WidgetSize; 3] = [WidgetSize::Small, WidgetSize::Half, WidgetSize::Full];

  pub fn label(self) -> &'static str {
    match self {
      WidgetSize::Small => "Small",
      WidgetSize::Half => "Half",
      WidgetSize::Full => "Full width",
    }
  }
}

/// What a time series covers: one pigeon (a series per key) or one flock
/// (one key, a series per pigeon).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidgetScope {
  Pigeon(String),
  Flock(Uuid),
}

/// A time series widget's window. Spelled as fancier's saved-graph range so
/// a saved graph maps onto a widget one to one.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WidgetRange {
  Last1h,
  Last6h,
  #[default]
  Last24h,
  Last7d,
  Last30d,
}

/// A time series widget's chart. Spelled as fancier's chart kind.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WidgetChart {
  #[default]
  Line,
  Step,
  Area,
  Bar,
  Scatter,
}

/// One widget: its place in the grid plus what it shows.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Widget {
  pub id: String,
  #[serde(default)]
  pub size: WidgetSize,
  #[serde(default)]
  pub title: String,
  #[serde(flatten)]
  pub body: WidgetBody,
}

/// What a widget shows, tagged by `kind`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WidgetBody {
  /// One or more keys over a range.
  TimeSeries {
    scope: WidgetScope,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    range: WidgetRange,
    #[serde(default)]
    chart: WidgetChart,
  },
  /// One key's newest value and its age.
  Latest {
    pigeon: String,
    key: String,
    #[serde(default)]
    unit: String,
  },
  /// One value against a declared range, coloured by the threshold alerts on
  /// that key that apply to the pigeon, its own or its flock's.
  Threshold {
    pigeon: String,
    key: String,
    #[serde(default)]
    unit: String,
    min: f64,
    max: f64,
  },
  /// Every pigeon in a flock with its connection state, chosen keys, and
  /// whether it has applied its config.
  StatusTable {
    flock: Uuid,
    #[serde(default)]
    keys: Vec<String>,
  },
  /// One top-level key of a pigeon's target and reported config.
  ShadowKey { pigeon: String, key: String },
  /// A flock's alerts and which are firing.
  Alerts { flock: Uuid },
  /// A plain-text note. Rendered as text, never as markup: an org dashboard
  /// is one member's words shown in another member's session.
  Text {
    #[serde(default)]
    text: String,
  },
  /// A kind this build does not know. Read so a page still renders; never
  /// saved, so a stale tab cannot write over a widget it cannot read.
  #[serde(other)]
  Unknown,
}

/// The stored page: an ordered list of widgets.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DashboardLayout {
  #[serde(default = "layout_version")]
  pub v: u32,
  #[serde(default)]
  pub widgets: Vec<Widget>,
}

fn layout_version() -> u32 {
  DASHBOARD_LAYOUT_VERSION
}

impl Default for DashboardLayout {
  fn default() -> Self {
    Self {
      v: DASHBOARD_LAYOUT_VERSION,
      widgets: Vec::new(),
    }
  }
}

/// Why a layout or a name was refused. `message` is the 400 body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardRejection {
  Name,
  Version,
  TooManyWidgets,
  WidgetId,
  DuplicateWidgetId,
  Title,
  UnknownKind,
  Keys,
  Key,
  Pigeon,
  Range,
  Text,
}

impl DashboardRejection {
  pub fn message(self) -> &'static str {
    match self {
      Self::Name => "Bad Request: a dashboard name is 1 to 80 characters",
      Self::Version => "Bad Request: unsupported layout version",
      Self::TooManyWidgets => "Bad Request: a dashboard holds at most 24 widgets",
      Self::WidgetId => "Bad Request: a widget id is 1 to 32 of [A-Za-z0-9_-]",
      Self::DuplicateWidgetId => "Bad Request: two widgets share an id",
      Self::Title => "Bad Request: a widget title is at most 80 characters",
      Self::UnknownKind => {
        "Bad Request: this dashboard holds a widget this page does not know; reload the page"
      }
      Self::Keys => "Bad Request: a widget names 1 to 8 telemetry keys (exactly 1 at flock scope)",
      Self::Key => "Bad Request: a telemetry or config key is 1 to 128 bytes",
      Self::Pigeon => "Bad Request: invalid pigeon id",
      Self::Range => "Bad Request: a threshold tile needs finite min below max",
      Self::Text => "Bad Request: a text widget is at most 2000 characters",
    }
  }
}

/// A dashboard name, trimmed, or the reason it was refused.
pub fn normalize_dashboard_name(name: &str) -> Result<String, DashboardRejection> {
  let trimmed = name.trim();
  let chars = trimmed.chars().count();
  if chars == 0 || chars > MAX_DASHBOARD_NAME_CHARS {
    return Err(DashboardRejection::Name);
  }
  Ok(trimmed.to_string())
}

fn valid_widget_id(id: &str) -> bool {
  !id.is_empty()
    && id.len() <= MAX_WIDGET_ID_CHARS
    && id
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// Durable Object ids are 64 hex characters; anything else could not name a
/// pigeon and would only ever render as "unavailable".
fn valid_pigeon_id(id: &str) -> bool {
  id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_key(key: &str) -> bool {
  !key.is_empty() && key.len() <= crate::MAX_TELEMETRY_KEY_BYTES
}

fn check_keys(keys: &[String], exactly_one: bool) -> Result<(), DashboardRejection> {
  let count_ok = if exactly_one {
    keys.len() == 1
  } else {
    !keys.is_empty() && keys.len() <= MAX_WIDGET_KEYS
  };
  if !count_ok {
    return Err(DashboardRejection::Keys);
  }
  if keys.iter().all(|k| valid_key(k)) {
    Ok(())
  } else {
    Err(DashboardRejection::Key)
  }
}

fn check_pigeon_key(pigeon: &str, key: &str) -> Result<(), DashboardRejection> {
  if !valid_pigeon_id(pigeon) {
    return Err(DashboardRejection::Pigeon);
  }
  if !valid_key(key) {
    return Err(DashboardRejection::Key);
  }
  Ok(())
}

/// Checks a layout before it is stored. The byte cap is the caller's, since
/// only the caller holds the raw body.
pub fn validate_layout(layout: &DashboardLayout) -> Result<(), DashboardRejection> {
  if layout.v != DASHBOARD_LAYOUT_VERSION {
    return Err(DashboardRejection::Version);
  }
  if layout.widgets.len() > MAX_DASHBOARD_WIDGETS {
    return Err(DashboardRejection::TooManyWidgets);
  }
  let mut ids = HashSet::with_capacity(layout.widgets.len());
  for widget in &layout.widgets {
    if !valid_widget_id(&widget.id) {
      return Err(DashboardRejection::WidgetId);
    }
    if !ids.insert(widget.id.as_str()) {
      return Err(DashboardRejection::DuplicateWidgetId);
    }
    if widget.title.chars().count() > MAX_DASHBOARD_NAME_CHARS {
      return Err(DashboardRejection::Title);
    }
    match &widget.body {
      WidgetBody::TimeSeries { scope, keys, .. } => match scope {
        WidgetScope::Pigeon(pigeon) => {
          if !valid_pigeon_id(pigeon) {
            return Err(DashboardRejection::Pigeon);
          }
          check_keys(keys, false)?;
        }
        WidgetScope::Flock(_) => check_keys(keys, true)?,
      },
      WidgetBody::Latest { pigeon, key, .. } | WidgetBody::ShadowKey { pigeon, key } => {
        check_pigeon_key(pigeon, key)?;
      }
      WidgetBody::Threshold {
        pigeon,
        key,
        min,
        max,
        ..
      } => {
        check_pigeon_key(pigeon, key)?;
        if !(min.is_finite() && max.is_finite() && min < max) {
          return Err(DashboardRejection::Range);
        }
      }
      WidgetBody::StatusTable { keys, .. } => {
        if keys.len() > MAX_WIDGET_KEYS {
          return Err(DashboardRejection::Keys);
        }
        if !keys.iter().all(|k| valid_key(k)) {
          return Err(DashboardRejection::Key);
        }
      }
      WidgetBody::Alerts { .. } => {}
      WidgetBody::Text { text } => {
        if text.chars().count() > MAX_WIDGET_TEXT_CHARS {
          return Err(DashboardRejection::Text);
        }
      }
      WidgetBody::Unknown => return Err(DashboardRejection::UnknownKind),
    }
  }
  Ok(())
}

/// DB model for one `dashboards` row; `layout` is read as `::text`.
#[derive(Deserialize, Debug)]
pub struct DashboardRow {
  pub id: Uuid,
  pub user_id: Option<Uuid>,
  pub org_id: Option<Uuid>,
  pub created_by: Option<Uuid>,
  pub name: String,
  pub layout: String,
  pub revision: i32,
  pub created_at: OffsetDateTime,
  pub updated_at: OffsetDateTime,
}

impl From<DashboardRow> for Dashboard {
  fn from(row: DashboardRow) -> Self {
    // A layout this build cannot parse (written by a later version, or by
    // hand) reads as an empty page flagged `unreadable`, never as an empty
    // page that a save would write over the stored widgets.
    let parsed = serde_json::from_str(&row.layout);
    Self {
      id: row.id,
      user_id: row.user_id,
      org_id: row.org_id,
      created_by: row.created_by,
      name: row.name,
      unreadable: parsed.is_err(),
      layout: parsed.unwrap_or_default(),
      revision: row.revision,
      created_at: row.created_at,
      updated_at: row.updated_at,
      can_edit: false,
    }
  }
}

/// One dashboard with its layout, as `GET /dashboards/:dashboard_id` and
/// every write return it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Dashboard {
  pub id: Uuid,
  /// Set for a personal dashboard; exactly one of this and `org_id` is.
  pub user_id: Option<Uuid>,
  pub org_id: Option<Uuid>,
  /// Who made it. Provenance only, and `None` once that account is erased.
  pub created_by: Option<Uuid>,
  pub name: String,
  pub layout: DashboardLayout,
  /// Bumped by every save; an update must send the revision it loaded.
  pub revision: i32,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
  /// Whether the caller may save, rename or delete it. Computed per request,
  /// and `false` while `unreadable`.
  #[serde(default)]
  pub can_edit: bool,
  /// The stored layout could not be parsed, so `layout` is empty and nothing
  /// may be saved over it or copied from it.
  #[serde(default)]
  pub unreadable: bool,
}

/// One row of `GET /dashboards`: everything but the layout.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DashboardSummary {
  pub id: Uuid,
  pub user_id: Option<Uuid>,
  pub org_id: Option<Uuid>,
  pub name: String,
  pub widget_count: i32,
  pub revision: i32,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
  #[serde(default)]
  pub can_edit: bool,
}

/// `POST /dashboards`. `org_id` makes it an organization's dashboard.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DashboardCreateRequest {
  pub name: String,
  #[serde(default)]
  pub org_id: Option<Uuid>,
  /// Starts the page with widgets already on it; empty when absent.
  #[serde(default)]
  pub layout: Option<DashboardLayout>,
}

/// `PUT /dashboards/:dashboard_id`: replaces name and layout, but only if
/// `revision` is still the stored one.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DashboardUpdateRequest {
  pub name: String,
  pub layout: DashboardLayout,
  pub revision: i32,
}

/// Pigeon ids one `POST /pigeons/batch/telemetry` may name: the same
/// Workers subrequest bound as `POST /pigeons/batch`.
pub const MAX_TELEMETRY_BATCH_PIGEONS: usize = 48;

/// `POST /pigeons/batch/telemetry`. An empty `keys` returns every key.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonTelemetryBatchRequest {
  pub ids: Vec<String>,
  #[serde(default)]
  pub keys: Vec<String>,
}

/// One pigeon's latest readings in a batch answer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonLatestTelemetry {
  pub pigeon_id: String,
  pub telemetry: Vec<TelemetryLatest>,
}

/// Pigeons one `GET /flocks/:flock_id/shadow/versions` answer carries.
pub const MAX_FLOCK_SHADOW_ROWS: i64 = 1_000;

/// One pigeon's shadow versions, plus the named config keys when asked.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PigeonShadowSummary {
  pub pigeon_id: String,
  pub target_version: i32,
  pub current_version: i32,
  /// Unix seconds, as `PigeonShadow::updated_at`; `None` with no shadow row.
  pub updated_at: Option<i64>,
  /// The requested keys present in `target_config`.
  #[serde(default)]
  pub target: serde_json::Map<String, serde_json::Value>,
  /// The requested keys present in `current_config`.
  #[serde(default)]
  pub current: serde_json::Map<String, serde_json::Value>,
}

impl PigeonShadowSummary {
  /// The device has not yet reported the newest target.
  pub fn behind(&self) -> bool {
    self.current_version < self.target_version
  }
}

/// `GET /flocks/:flock_id/shadow/versions`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FlockShadowVersions {
  pub pigeons: Vec<PigeonShadowSummary>,
  /// More pigeons exist than [`MAX_FLOCK_SHADOW_ROWS`]; the first by id are
  /// here.
  pub truncated: bool,
}

#[cfg(test)]
mod tests {
  use super::*;

  const PIGEON: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

  fn widget(id: &str, body: WidgetBody) -> Widget {
    Widget {
      id: id.to_string(),
      size: WidgetSize::Small,
      title: String::new(),
      body,
    }
  }

  fn layout(widgets: Vec<Widget>) -> DashboardLayout {
    DashboardLayout {
      v: DASHBOARD_LAYOUT_VERSION,
      widgets,
    }
  }

  fn text(id: &str) -> Widget {
    widget(
      id,
      WidgetBody::Text {
        text: "Depot 3".into(),
      },
    )
  }

  #[test]
  fn the_design_example_round_trips() {
    let json = r#"{
      "v": 1,
      "widgets": [
        { "id": "w1", "kind": "time_series", "size": "full", "title": "Brightness",
          "scope": { "flock": "2f1a3b4c-5d6e-7f80-9012-3456789abcde" }, "keys": ["brightness"],
          "range": "Last24h", "chart": "Line" },
        { "id": "w2", "kind": "latest", "size": "small",
          "pigeon": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
          "key": "temp_c", "unit": "C" },
        { "id": "w3", "kind": "status_table", "size": "full",
          "flock": "2f1a3b4c-5d6e-7f80-9012-3456789abcde", "keys": ["layout"] },
        { "id": "w4", "kind": "text", "size": "half", "text": "Depot 3. Call Sam at ext 412." }
      ]
    }"#;
    let parsed: DashboardLayout = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.widgets.len(), 4);
    assert_eq!(parsed.widgets[0].size, WidgetSize::Full);
    assert!(matches!(
      parsed.widgets[0].body,
      WidgetBody::TimeSeries {
        scope: WidgetScope::Flock(_),
        ..
      }
    ));
    validate_layout(&parsed).unwrap();
    let again: DashboardLayout =
      serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
    assert_eq!(again, parsed);
  }

  #[test]
  fn a_kind_this_build_does_not_know_still_reads_but_never_saves() {
    let json = r#"{"v":1,"widgets":[{"id":"w1","kind":"map","zoom":3},
      {"id":"w2","kind":"text","text":"hi"}]}"#;
    let parsed: DashboardLayout = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.widgets[0].body, WidgetBody::Unknown);
    assert_eq!(
      validate_layout(&parsed),
      Err(DashboardRejection::UnknownKind)
    );
  }

  #[test]
  fn fields_added_later_default_rather_than_failing_the_page() {
    let json = r#"{"widgets":[{"id":"w1","kind":"text"}]}"#;
    let parsed: DashboardLayout = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.v, DASHBOARD_LAYOUT_VERSION);
    assert_eq!(parsed.widgets[0].size, WidgetSize::Small);
    assert_eq!(parsed.widgets[0].title, "");
  }

  #[test]
  fn widget_count_and_ids_are_capped_and_unique() {
    let many: Vec<Widget> = (0..=MAX_DASHBOARD_WIDGETS)
      .map(|i| text(&i.to_string()))
      .collect();
    assert_eq!(
      validate_layout(&layout(many)),
      Err(DashboardRejection::TooManyWidgets)
    );
    assert_eq!(
      validate_layout(&layout(vec![text("a"), text("a")])),
      Err(DashboardRejection::DuplicateWidgetId)
    );
    assert_eq!(
      validate_layout(&layout(vec![text("a b")])),
      Err(DashboardRejection::WidgetId)
    );
    let full: Vec<Widget> = (0..MAX_DASHBOARD_WIDGETS)
      .map(|i| text(&i.to_string()))
      .collect();
    assert_eq!(validate_layout(&layout(full)), Ok(()));
  }

  #[test]
  fn a_flock_time_series_takes_exactly_one_key() {
    let flock = |keys: Vec<&str>| {
      layout(vec![widget(
        "w",
        WidgetBody::TimeSeries {
          scope: WidgetScope::Flock(Uuid::nil()),
          keys: keys.into_iter().map(String::from).collect(),
          range: WidgetRange::Last24h,
          chart: WidgetChart::Line,
        },
      )])
    };
    assert_eq!(validate_layout(&flock(vec!["a"])), Ok(()));
    assert_eq!(
      validate_layout(&flock(vec!["a", "b"])),
      Err(DashboardRejection::Keys)
    );
    assert_eq!(
      validate_layout(&flock(vec![])),
      Err(DashboardRejection::Keys)
    );
  }

  #[test]
  fn pigeon_widgets_need_a_real_pigeon_id_and_key() {
    let latest = |pigeon: &str, key: &str| {
      layout(vec![widget(
        "w",
        WidgetBody::Latest {
          pigeon: pigeon.into(),
          key: key.into(),
          unit: String::new(),
        },
      )])
    };
    assert_eq!(validate_layout(&latest(PIGEON, "temp_c")), Ok(()));
    assert_eq!(
      validate_layout(&latest("../orgs", "temp_c")),
      Err(DashboardRejection::Pigeon)
    );
    assert_eq!(
      validate_layout(&latest(PIGEON, "")),
      Err(DashboardRejection::Key)
    );
  }

  #[test]
  fn a_threshold_tile_needs_an_ordered_finite_range() {
    let tile = |min: f64, max: f64| {
      layout(vec![widget(
        "w",
        WidgetBody::Threshold {
          pigeon: PIGEON.into(),
          key: "battery_pct".into(),
          unit: "%".into(),
          min,
          max,
        },
      )])
    };
    assert_eq!(validate_layout(&tile(0.0, 100.0)), Ok(()));
    assert_eq!(
      validate_layout(&tile(5.0, 5.0)),
      Err(DashboardRejection::Range)
    );
    assert_eq!(
      validate_layout(&tile(f64::NAN, 1.0)),
      Err(DashboardRejection::Range)
    );
  }

  #[test]
  fn long_text_and_titles_are_refused() {
    let mut long = text("w");
    long.body = WidgetBody::Text {
      text: "x".repeat(MAX_WIDGET_TEXT_CHARS + 1),
    };
    assert_eq!(
      validate_layout(&layout(vec![long])),
      Err(DashboardRejection::Text)
    );
    let mut titled = text("w");
    titled.title = "t".repeat(MAX_DASHBOARD_NAME_CHARS + 1);
    assert_eq!(
      validate_layout(&layout(vec![titled])),
      Err(DashboardRejection::Title)
    );
  }

  #[test]
  fn names_are_trimmed_and_bounded() {
    assert_eq!(normalize_dashboard_name("  Depot 3 ").unwrap(), "Depot 3");
    assert_eq!(
      normalize_dashboard_name("   "),
      Err(DashboardRejection::Name)
    );
    assert!(normalize_dashboard_name(&"é".repeat(MAX_DASHBOARD_NAME_CHARS)).is_ok());
    assert!(normalize_dashboard_name(&"é".repeat(MAX_DASHBOARD_NAME_CHARS + 1)).is_err());
  }

  #[test]
  fn an_unreadable_stored_layout_reads_as_an_empty_page_flagged_unreadable() {
    let row = DashboardRow {
      id: Uuid::nil(),
      user_id: Some(Uuid::nil()),
      org_id: None,
      created_by: None,
      name: "n".into(),
      layout: "{\"v\":1,\"widgets\":7}".into(),
      revision: 3,
      created_at: OffsetDateTime::UNIX_EPOCH,
      updated_at: OffsetDateTime::UNIX_EPOCH,
    };
    let dashboard = Dashboard::from(row);
    assert!(dashboard.layout.widgets.is_empty());
    assert_eq!(dashboard.revision, 3);
    assert!(dashboard.unreadable);
    assert!(!dashboard.can_edit);
  }

  #[test]
  fn behind_means_the_device_has_not_reported_the_newest_target() {
    let summary = |target, current| PigeonShadowSummary {
      pigeon_id: PIGEON.into(),
      target_version: target,
      current_version: current,
      updated_at: None,
      target: Default::default(),
      current: Default::default(),
    };
    assert!(summary(3, 2).behind());
    assert!(!summary(3, 3).behind());
  }
}
