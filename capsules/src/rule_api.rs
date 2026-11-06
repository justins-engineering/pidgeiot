//! The dashboard's rule routes: the bodies they accept, the models they answer with, and the
//! checks fancier and dovecote both apply to a rule's name and source.

use crate::RuleDryRun;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Longest rule name, in bytes. The name rides in the footer of every mail the rule sends.
pub const MAX_RULE_NAME_BYTES: usize = 120;

/// Largest rule source, in bytes; the `rule_versions.source` CHECK holds the same number.
pub const MAX_RULE_SOURCE_BYTES: usize = 64 * 1024;

/// Most rules one flock runs at once. One flock's rules share one dispatch and its CPU, so this
/// is a ceiling on that bundle, not a price.
pub const MAX_ENABLED_RULES_PER_FLOCK: i64 = 20;

/// Most versions kept per rule. A save past it drops the oldest that is neither deployed nor
/// mid-deploy.
pub const MAX_RULE_VERSIONS: i64 = 50;

/// Most run records one read returns, newest first.
pub const MAX_RULE_RUNS_PER_PAGE: i64 = 200;

/// Most latest values one read returns, one per pigeon and key.
pub const MAX_RULE_VALUES_RETURNED: i64 = 1000;

/// The one module a rule may import: the platform's helpers, shipped in every rules script.
pub const RULE_HELPERS_IMPORT: &str = "./pidgeiot.js";

/// Who wrote a version.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleOrigin {
  /// A person, in the editor or by uploading a file.
  #[default]
  Hand,
  /// The assistant, unchanged.
  Assistant,
  /// A person's edit of an assistant draft.
  AssistantEdited,
}

impl RuleOrigin {
  /// The `rule_versions.origin` value.
  pub fn as_str(self) -> &'static str {
    match self {
      RuleOrigin::Hand => "hand",
      RuleOrigin::Assistant => "assistant",
      RuleOrigin::AssistantEdited => "assistant_edited",
    }
  }

  /// Reads a stored `origin`; anything unknown reads as `Hand`.
  pub fn from_stored(value: &str) -> Self {
    match value {
      "assistant" => RuleOrigin::Assistant,
      "assistant_edited" => RuleOrigin::AssistantEdited,
      _ => RuleOrigin::Hand,
    }
  }
}

/// Why a rule name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleNameRejection {
  /// Blank once trimmed.
  Empty,
  /// Over [`MAX_RULE_NAME_BYTES`].
  TooLong,
  /// Holds a control character, which a mail header or footer must not carry.
  ControlCharacter,
}

impl RuleNameRejection {
  /// The route's 400 text.
  pub fn message(self) -> &'static str {
    match self {
      RuleNameRejection::Empty => "Bad Request: 'name' cannot be empty",
      RuleNameRejection::TooLong => "Bad Request: 'name' is longer than 120 bytes",
      RuleNameRejection::ControlCharacter => "Bad Request: 'name' holds a control character",
    }
  }
}

/// Trims a rule name and checks it: not empty, at most [`MAX_RULE_NAME_BYTES`], and no control
/// character, since it becomes part of a mail.
pub fn normalize_rule_name(name: &str) -> Result<String, RuleNameRejection> {
  let name = name.trim();
  if name.is_empty() {
    return Err(RuleNameRejection::Empty);
  }
  if name.len() > MAX_RULE_NAME_BYTES {
    return Err(RuleNameRejection::TooLong);
  }
  if name.chars().any(char::is_control) {
    return Err(RuleNameRejection::ControlCharacter);
  }
  Ok(name.to_string())
}

/// Why a rule's source was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSourceRejection {
  /// Blank.
  Empty,
  /// Over [`MAX_RULE_SOURCE_BYTES`].
  TooLarge,
  /// A static import of anything but [`RULE_HELPERS_IMPORT`].
  Import,
}

impl RuleSourceRejection {
  /// The route's error text; `TooLarge` answers 413, the others 400.
  pub fn message(self) -> &'static str {
    match self {
      RuleSourceRejection::Empty => "Bad Request: 'source' cannot be empty",
      RuleSourceRejection::TooLarge => "Payload Too Large: a rule's source is at most 64 KiB",
      RuleSourceRejection::Import => "Bad Request: a rule may import only ./pidgeiot.js",
    }
  }
}

/// Checks a rule's source before it is stored: not blank, at most [`MAX_RULE_SOURCE_BYTES`], and
/// no static import but [`RULE_HELPERS_IMPORT`] ([`first_refused_import`]).
pub fn check_rule_source(source: &str) -> Result<(), RuleSourceRejection> {
  if source.trim().is_empty() {
    return Err(RuleSourceRejection::Empty);
  }
  if source.len() > MAX_RULE_SOURCE_BYTES {
    return Err(RuleSourceRejection::TooLarge);
  }
  match first_refused_import(source) {
    Some(_) => Err(RuleSourceRejection::Import),
    None => Ok(()),
  }
}

/// The first static import specifier in `source` other than [`RULE_HELPERS_IMPORT`]: a string
/// literal following `from` or `import`, outside comments, strings and templates.
///
/// A screen, not a parser. It keeps a rule from naming the harness or a sibling module by
/// accident; it reads no regular expression literal and leaves `import()` alone, because the
/// boundary between tenants is the flock's own script, not this check.
pub fn first_refused_import(source: &str) -> Option<&str> {
  let bytes = source.as_bytes();
  // Every byte of a multi-byte character is >= 0x80, so treating those as identifier bytes keeps
  // each slice below on a character boundary.
  let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80;
  let mut after = "";
  let mut i = 0;
  while i < bytes.len() {
    let b = bytes[i];
    if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
      i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
    } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
      i = source[i + 2..]
        .find("*/")
        .map_or(bytes.len(), |n| i + n + 4);
    } else if matches!(b, b'"' | b'\'' | b'`') {
      let start = i + 1;
      let mut end = start;
      while end < bytes.len() && bytes[end] != b {
        end += if bytes[end] == b'\\' { 2 } else { 1 };
      }
      let end = end.min(bytes.len());
      if b != b'`'
        && matches!(after, "from" | "import")
        && &source[start..end] != RULE_HELPERS_IMPORT
      {
        return Some(&source[start..end]);
      }
      after = "";
      i = end + 1;
    } else if ident(b) {
      let start = i;
      while i < bytes.len() && ident(bytes[i]) {
        i += 1;
      }
      after = &source[start..i];
    } else {
      if !b.is_ascii_whitespace() {
        after = "";
      }
      i += 1;
    }
  }
  None
}

/// Body of `POST /flocks/:flock_id/rules`. A new rule is disabled and has no version.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleCreateRequest {
  pub name: String,
  /// Who the rule mails, each address tied to the account as an alert's are; empty means the
  /// flock's owner address.
  #[serde(default)]
  pub recipients: Vec<String>,
}

/// Body of `PUT /flocks/:flock_id/rules/:rule_id`; an omitted field keeps its value.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleUpdateRequest {
  pub name: Option<String>,
  pub recipients: Option<Vec<String>>,
  /// `true` needs a deployed version, no platform block and room in the flock.
  pub enabled: Option<bool>,
}

/// Body of `POST /flocks/:flock_id/rules/:rule_id/versions`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleVersionCreateRequest {
  /// The module, at most [`MAX_RULE_SOURCE_BYTES`].
  pub source: String,
  /// `Hand` or `AssistantEdited`; `Assistant` is the assistant's own and is refused here.
  #[serde(default)]
  pub origin: RuleOrigin,
  /// The version this one was edited from, which must exist.
  #[serde(default)]
  pub based_on: Option<i32>,
}

/// Body of `POST .../versions/:version/deploy`; may be omitted.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuleDeployRequest {
  /// Delete what the rule has stored, so the new version starts from empty documents.
  #[serde(default)]
  pub reset_state: bool,
}

/// DB model for one `rules` row. `recipients` is read as `::text`.
#[derive(Deserialize, Debug, Clone)]
pub struct RuleRow {
  pub id: Uuid,
  pub flock_id: Uuid,
  pub created_by: Uuid,
  pub name: String,
  pub recipients: String,
  pub enabled: bool,
  pub deployed_version: Option<i32>,
  pub pending_version: Option<i32>,
  pub paused_reason: Option<String>,
  pub blocked_at: Option<OffsetDateTime>,
  pub fail_count: i32,
  pub watch_until: Option<OffsetDateTime>,
  pub created_at: OffsetDateTime,
  pub updated_at: OffsetDateTime,
}

impl From<RuleRow> for Rule {
  fn from(row: RuleRow) -> Self {
    Self {
      id: row.id,
      flock_id: row.flock_id,
      created_by: row.created_by,
      name: row.name,
      recipients: serde_json::from_str(&row.recipients).unwrap_or_default(),
      enabled: row.enabled,
      deployed_version: row.deployed_version,
      pending_version: row.pending_version,
      paused_reason: row.paused_reason,
      blocked: row.blocked_at.is_some(),
      fail_count: row.fail_count,
      watch_until: row.watch_until,
      created_at: row.created_at,
      updated_at: row.updated_at,
    }
  }
}

/// One rule as the routes answer it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rule {
  pub id: Uuid,
  pub flock_id: Uuid,
  pub created_by: Uuid,
  pub name: String,
  pub recipients: Vec<String>,
  /// Runs on the flock's telemetry. Only a deployed rule can be enabled.
  pub enabled: bool,
  /// The version the flock's script runs; `None` until the first deploy.
  pub deployed_version: Option<i32>,
  /// Set while a deploy of that version is uploading; still set afterwards means it did not
  /// finish.
  pub pending_version: Option<i32>,
  /// `owner` when a person disabled it, `auto:<status>` when repeated failures did.
  pub paused_reason: Option<String>,
  /// Blocked by the platform; only the platform can lift it.
  pub blocked: bool,
  /// Consecutive failed runs.
  pub fail_count: i32,
  /// Every run is recorded until then.
  #[serde(with = "time::serde::rfc3339::option")]
  pub watch_until: Option<OffsetDateTime>,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
}

/// DB model for one `rule_versions` row. `test_summary` is read as `::text`, and `source` only
/// where a route returns it.
#[derive(Deserialize, Debug, Clone)]
pub struct RuleVersionRow {
  pub rule_id: Uuid,
  pub version: i32,
  pub sha256: String,
  pub contract: i32,
  pub origin: String,
  pub based_on: Option<i32>,
  pub repair_depth: i16,
  pub prompt: Option<String>,
  pub tested_sha256: Option<String>,
  pub tested_contract: Option<i32>,
  pub tested_at: Option<OffsetDateTime>,
  pub test_summary: Option<String>,
  pub created_by: Uuid,
  pub created_at: OffsetDateTime,
  pub source: Option<String>,
}

impl From<RuleVersionRow> for RuleVersion {
  fn from(row: RuleVersionRow) -> Self {
    // A test record counts only for the bytes it ran.
    let tested = row.tested_sha256.as_deref() == Some(row.sha256.as_str());
    Self {
      rule_id: row.rule_id,
      version: row.version,
      sha256: row.sha256,
      contract: row.contract,
      origin: RuleOrigin::from_stored(&row.origin),
      based_on: row.based_on,
      repair_depth: row.repair_depth,
      prompt: row.prompt,
      tested_contract: row.tested_contract.filter(|_| tested),
      tested_at: row.tested_at.filter(|_| tested),
      // A summary from before the dry run's shape existed reads as none; the record stands.
      test_summary: row
        .test_summary
        .filter(|_| tested)
        .and_then(|s| serde_json::from_str(&s).ok()),
      created_by: row.created_by,
      created_at: row.created_at,
      source: row.source,
    }
  }
}

/// One version of a rule. Lists leave out `source` and `test_summary`; the single read and the
/// save carry them.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleVersion {
  pub rule_id: Uuid,
  /// 1 for the first save, one more for each after.
  pub version: i32,
  /// Lowercase hex sha256 of `source`.
  pub sha256: String,
  /// The contract the version was written against.
  pub contract: i32,
  pub origin: RuleOrigin,
  pub based_on: Option<i32>,
  /// 0 for a draft, 1 or 2 for an assistant repair of one.
  pub repair_depth: i16,
  /// The person's words, on assistant versions only.
  pub prompt: Option<String>,
  /// The contract its dry run ran under; `None` until tested.
  pub tested_contract: Option<i32>,
  #[serde(with = "time::serde::rfc3339::option")]
  pub tested_at: Option<OffsetDateTime>,
  /// What the dry run of these bytes found.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub test_summary: Option<RuleDryRun>,
  pub created_by: Uuid,
  #[serde(with = "time::serde::rfc3339")]
  pub created_at: OffsetDateTime,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub source: Option<String>,
}

/// `GET /flocks/:flock_id/rules/:rule_id`: the rule and its versions, newest first.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleDetail {
  pub rule: Rule,
  pub versions: Vec<RuleVersion>,
}

/// DB model for one `rule_runs` row. `logs` is read as `::text`.
#[derive(Deserialize, Debug, Clone)]
pub struct RuleRunRow {
  pub rule_id: Uuid,
  pub version: i32,
  pub pigeon_id: String,
  pub batch_id: String,
  pub status: String,
  pub readings: i32,
  pub alerts_sent: i32,
  pub alerts_suppressed: i32,
  pub values_written: i32,
  pub state_changed: bool,
  pub wall_ms: Option<i32>,
  pub error: Option<String>,
  pub logs: Option<String>,
  pub ran_at: OffsetDateTime,
}

impl From<RuleRunRow> for RuleRun {
  fn from(row: RuleRunRow) -> Self {
    Self {
      rule_id: row.rule_id,
      version: row.version,
      pigeon_id: row.pigeon_id,
      batch_id: row.batch_id,
      status: row.status,
      readings: row.readings,
      alerts_sent: row.alerts_sent,
      alerts_suppressed: row.alerts_suppressed,
      values_written: row.values_written,
      state_changed: row.state_changed,
      wall_ms: row.wall_ms,
      error: row.error,
      logs: row
        .logs
        .and_then(|l| serde_json::from_str(&l).ok())
        .unwrap_or_default(),
      ran_at: row.ran_at,
    }
  }
}

/// One recorded run of a rule over one pigeon's batch. A failed run is recorded, but a skipped
/// one only while the rule is watched and a version mismatch only once it has lasted ten minutes,
/// longer than a deploy takes; a quiet successful run only while watched.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleRun {
  pub rule_id: Uuid,
  pub version: i32,
  pub pigeon_id: String,
  pub batch_id: String,
  /// `ok`, `threw`, `timeout`, `skipped`, `bad_output`, `oversize`, `version_mismatch` or
  /// `dispatch_failed`.
  pub status: String,
  pub readings: i32,
  pub alerts_sent: i32,
  pub alerts_suppressed: i32,
  pub values_written: i32,
  pub state_changed: bool,
  pub wall_ms: Option<i32>,
  /// The rule's own error text, which may hold anything the code put in it.
  pub error: Option<String>,
  /// The rule's captured `console.log` lines.
  pub logs: Vec<String>,
  #[serde(with = "time::serde::rfc3339")]
  pub ran_at: OffsetDateTime,
}

/// DB model for one `rule_values` row.
#[derive(Deserialize, Debug, Clone)]
pub struct RuleValueRow {
  pub pigeon_id: String,
  pub key: String,
  pub value: String,
  pub value_num: Option<f64>,
  pub at: OffsetDateTime,
}

impl From<RuleValueRow> for RuleValue {
  fn from(row: RuleValueRow) -> Self {
    Self {
      pigeon_id: row.pigeon_id,
      key: row.key,
      value: row.value,
      value_num: row.value_num,
      at: row.at,
    }
  }
}

/// A rule's latest derived value for one pigeon and key.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RuleValue {
  pub pigeon_id: String,
  pub key: String,
  pub value: String,
  /// `value` as a number, when it parses as one.
  pub value_num: Option<f64>,
  /// The reading time the rule gave it, clamped into its batch.
  #[serde(with = "time::serde::rfc3339")]
  pub at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_name_is_trimmed_and_bounded() {
    assert_eq!(
      normalize_rule_name("  Reboot loop "),
      Ok("Reboot loop".into())
    );
    assert_eq!(normalize_rule_name(" \t "), Err(RuleNameRejection::Empty));
    let longest = "é".repeat(MAX_RULE_NAME_BYTES / 2);
    assert_eq!(normalize_rule_name(&longest), Ok(longest.clone()));
    let mut over = longest;
    over.push('x');
    assert_eq!(normalize_rule_name(&over), Err(RuleNameRejection::TooLong));
    for bad in ["a\nb", "a\rb", "a\u{7f}b", "a\u{85}b", "a\tb"] {
      assert_eq!(
        normalize_rule_name(bad),
        Err(RuleNameRejection::ControlCharacter)
      );
    }
  }

  #[test]
  fn source_is_capped_in_bytes_and_never_blank() {
    let mut source = String::from("export default () => ({});\n//");
    while source.len() < MAX_RULE_SOURCE_BYTES {
      source.push('x');
    }
    assert_eq!(check_rule_source(&source), Ok(()));
    source.push('x');
    assert_eq!(
      check_rule_source(&source),
      Err(RuleSourceRejection::TooLarge)
    );
    assert_eq!(check_rule_source(" \n\t"), Err(RuleSourceRejection::Empty));
  }

  #[test]
  fn the_helpers_are_the_only_static_import() {
    for ok in [
      "import { num, held } from \"./pidgeiot.js\";\nexport default () => ({});",
      "import * as p from './pidgeiot.js';",
      "import './pidgeiot.js';",
      "export { num } from \"./pidgeiot.js\";",
    ] {
      assert_eq!(first_refused_import(ok), None, "{ok}");
      assert_eq!(check_rule_source(ok), Ok(()), "{ok}");
    }
    for (bad, spec) in [
      ("import h from \"./main.js\";", "./main.js"),
      ("import \"./rules.js\";", "./rules.js"),
      ("export * from './r-0123.js';", "./r-0123.js"),
      ("import {a}\nfrom\n'./prelude.js'", "./prelude.js"),
      ("import x from /* note */ \"./main.js\"", "./main.js"),
      (
        "import { num } from \"./pidgeiot.js\";\nimport m from 'node:fs';",
        "node:fs",
      ),
      ("import x from \"./pidgeiot.js?x\"", "./pidgeiot.js?x"),
    ] {
      assert_eq!(first_refused_import(bad), Some(spec), "{bad}");
      assert_eq!(
        check_rule_source(bad),
        Err(RuleSourceRejection::Import),
        "{bad}"
      );
    }
  }

  #[test]
  fn the_import_screen_skips_what_is_not_code() {
    for quiet in [
      "// import x from \"./main.js\"\nexport default () => ({});",
      "/* import x from './main.js' */",
      "const s = 'import x from \"./main.js\"';",
      "const t = `import x from './main.js'`;",
      "const u = \"it's \\\" from \"; const v = 1;",
      "const w = Array.from(\"abc\");",
      "const m = await import(\"./main.js\");",
      "const é = 'from'; const from = 1;",
    ] {
      assert_eq!(first_refused_import(quiet), None, "{quiet}");
    }
  }

  #[test]
  fn an_unterminated_literal_ends_the_screen_without_panicking() {
    for odd in ["import x from \"./main.js", "/* open", "`open", "from 'é"] {
      let _ = first_refused_import(odd);
    }
    assert_eq!(
      first_refused_import("import x from \"./main.js"),
      Some("./main.js")
    );
  }

  #[test]
  fn origin_round_trips_through_its_stored_text() {
    for origin in [
      RuleOrigin::Hand,
      RuleOrigin::Assistant,
      RuleOrigin::AssistantEdited,
    ] {
      assert_eq!(RuleOrigin::from_stored(origin.as_str()), origin);
      let json = serde_json::to_string(&origin).expect("origin serializes");
      assert_eq!(json.trim_matches('"'), origin.as_str());
    }
  }

  #[test]
  fn a_test_record_counts_only_for_the_bytes_it_ran() {
    let now = OffsetDateTime::UNIX_EPOCH;
    let summary = serde_json::to_string(&crate::RuleDryRun {
      window: Default::default(),
      candidate: Default::default(),
      deployed: None,
      seeded: None,
      mails: Default::default(),
      cases: vec![],
      cpu: Default::default(),
    })
    .expect("summary serializes");
    let row = |tested: Option<&str>, summary: &str| RuleVersionRow {
      rule_id: Uuid::nil(),
      version: 3,
      sha256: "aa".into(),
      contract: 1,
      origin: "assistant_edited".into(),
      based_on: Some(2),
      repair_depth: 0,
      prompt: None,
      tested_sha256: tested.map(str::to_string),
      tested_contract: Some(1),
      tested_at: Some(now),
      test_summary: Some(summary.into()),
      created_by: Uuid::nil(),
      created_at: now,
      source: None,
    };
    let tested = RuleVersion::from(row(Some("aa"), &summary));
    assert_eq!(tested.tested_contract, Some(1));
    assert!(tested.test_summary.is_some());
    assert_eq!(tested.origin, RuleOrigin::AssistantEdited);
    let stale = RuleVersion::from(row(Some("bb"), &summary));
    assert_eq!(stale.tested_contract, None);
    assert_eq!(stale.tested_at, None);
    assert_eq!(stale.test_summary, None);
    let json = serde_json::to_value(&stale).expect("version serializes");
    assert!(json.get("source").is_none() && json.get("test_summary").is_none());
    let seeded_by_hand = RuleVersion::from(row(Some("aa"), "{\"cases\":[]}"));
    assert_eq!(seeded_by_hand.tested_contract, Some(1), "the record stands");
    assert_eq!(seeded_by_hand.test_summary, None);
  }

  #[test]
  fn refusal_texts_name_their_caps() {
    assert!(
      RuleNameRejection::TooLong
        .message()
        .contains(&MAX_RULE_NAME_BYTES.to_string())
    );
    assert!(
      RuleSourceRejection::TooLarge
        .message()
        .contains(&(MAX_RULE_SOURCE_BYTES / 1024).to_string())
    );
  }

  #[test]
  fn a_value_row_reads_as_its_rfc3339_model() {
    let at = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("valid time");
    let value = RuleValue::from(RuleValueRow {
      pigeon_id: "p1".into(),
      key: "dwell_s".into(),
      value: "42".into(),
      value_num: Some(42.0),
      at,
    });
    let json = serde_json::to_value(&value).expect("value serializes");
    assert_eq!(json["pigeon_id"], "p1");
    assert_eq!(json["value_num"], 42.0);
    assert_eq!(json["at"], "2027-01-15T08:00:00Z");
  }

  #[test]
  fn a_rule_row_hides_why_it_was_blocked() {
    let now = OffsetDateTime::UNIX_EPOCH;
    let rule = Rule::from(RuleRow {
      id: Uuid::nil(),
      flock_id: Uuid::nil(),
      created_by: Uuid::nil(),
      name: "r".into(),
      recipients: "[\"ops@example.com\"]".into(),
      enabled: false,
      deployed_version: Some(2),
      pending_version: None,
      paused_reason: Some("auto:threw".into()),
      blocked_at: Some(now),
      fail_count: 20,
      watch_until: None,
      created_at: now,
      updated_at: now,
    });
    assert!(rule.blocked);
    assert_eq!(rule.recipients, ["ops@example.com"]);
    let json = serde_json::to_value(&rule).expect("rule serializes");
    assert!(json.get("blocked_at").is_none() && json.get("blocked_reason").is_none());
  }
}
