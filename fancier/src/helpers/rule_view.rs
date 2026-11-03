//! The pure parts of the rule pages: the status badges a rule shows, the editor's Tab key, the
//! runs view's hourly bars and failure groups, and the labels a version carries.

use capsules::{RULE_CONTRACT, Rule, RuleOrigin, RuleRun, RuleVersion};
use time::OffsetDateTime;

/// One status badge: its text and the daisyUI badge class that colours it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
  pub label: String,
  pub class: &'static str,
}

impl Badge {
  fn new(label: impl Into<String>, class: &'static str) -> Self {
    Self {
      label: label.into(),
      class,
    }
  }
}

/// One hour of a rule's recorded runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourBar {
  /// Unix seconds at the top of the hour.
  pub start: i64,
  pub ok: u32,
  pub skipped: u32,
  pub failed: u32,
}

/// Failed runs that share a status and error text.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureGroup {
  pub status: String,
  pub error: Option<String>,
  pub count: usize,
  pub last_at: OffsetDateTime,
}

/// The badges a rule shows, most important first: a platform block, a pause, what is running, an
/// unfinished deploy, a failure streak and a watch.
pub fn rule_badges(rule: &Rule, now: OffsetDateTime) -> Vec<Badge> {
  let mut badges = Vec::with_capacity(4);
  if rule.blocked {
    badges.push(Badge::new("Blocked by the platform", "badge-error"));
  } else if let Some(status) = rule
    .paused_reason
    .as_deref()
    .and_then(|r| r.strip_prefix("auto:"))
    .filter(|_| !rule.enabled)
  {
    let mut label = String::with_capacity(13 + status.len());
    label.push_str("Auto-paused: ");
    label.push_str(status);
    badges.push(Badge::new(label, "badge-warning"));
  } else if !rule.enabled && rule.deployed_version.is_some() {
    badges.push(Badge::new("Paused", "badge-ghost"));
  }
  match rule.deployed_version {
    None => badges.push(Badge::new("Not deployed", "badge-ghost")),
    Some(version) if rule.enabled && !rule.blocked => {
      let version = version.to_string();
      let mut label = String::with_capacity(9 + version.len());
      label.push_str("Running v");
      label.push_str(&version);
      badges.push(Badge::new(label, "badge-success"));
    }
    Some(_) => {}
  }
  if let Some(pending) = rule.pending_version {
    let pending = pending.to_string();
    let mut label = String::with_capacity(26 + pending.len());
    label.push_str("Deploy of v");
    label.push_str(&pending);
    label.push_str(" did not finish");
    badges.push(Badge::new(label, "badge-warning"));
  }
  if rule.fail_count == 1 {
    badges.push(Badge::new("1 failed run", "badge-error"));
  } else if rule.fail_count > 1 {
    let count = rule.fail_count.to_string();
    let mut label = String::with_capacity(22 + count.len());
    label.push_str(&count);
    label.push_str(" failed runs in a row");
    badges.push(Badge::new(label, "badge-error"));
  }
  if rule.watch_until.is_some_and(|until| until > now) {
    badges.push(Badge::new("Recording every run", "badge-info"));
  }
  badges
}

/// Whether a run status is a failure. `skipped` is not: the harness ran out of its budget before
/// reaching the rule, which says nothing about the rule's code.
pub fn is_failure(status: &str) -> bool {
  !matches!(status, "ok" | "skipped")
}

/// Replaces the selection `start..end` with two spaces, the editor's Tab key, and returns the new
/// text with the caret after them. Offsets are the textarea's UTF-16 units, not bytes; one that
/// falls past the end or inside a surrogate pair moves back to a character boundary.
pub fn insert_indent(text: &str, start: u32, end: u32) -> (String, u32) {
  let (start, end) = if start <= end {
    (start, end)
  } else {
    (end, start)
  };
  let (start_byte, start_units) = utf16_boundary(text, start);
  let (end_byte, _) = utf16_boundary(text, end);
  let mut out = String::with_capacity(text.len() + 2);
  out.push_str(&text[..start_byte]);
  out.push_str("  ");
  out.push_str(&text[end_byte..]);
  (out, start_units + 2)
}

/// The byte index and UTF-16 offset of the last character boundary at or before `units`.
fn utf16_boundary(text: &str, units: u32) -> (usize, u32) {
  let mut seen = 0u32;
  for (byte, ch) in text.char_indices() {
    let next = seen + ch.len_utf16() as u32;
    if next > units {
      return (byte, seen);
    }
    seen = next;
  }
  (text.len(), seen)
}

/// The runs in each of the last `hours` hours, oldest first, the current hour last. Only recorded
/// runs are counted, so a quiet hour of a rule nobody is watching reads as empty.
pub fn hourly_bars(runs: &[RuleRun], now: OffsetDateTime, hours: usize) -> Vec<HourBar> {
  let top = now.unix_timestamp() - now.unix_timestamp().rem_euclid(3600);
  let first = top - 3600 * (hours as i64 - 1);
  let mut bars: Vec<HourBar> = (0..hours as i64)
    .map(|i| HourBar {
      start: first + 3600 * i,
      ok: 0,
      skipped: 0,
      failed: 0,
    })
    .collect();
  for run in runs {
    let at = run.ran_at.unix_timestamp();
    if at < first || at >= top + 3600 {
      continue;
    }
    let bar = &mut bars[((at - first) / 3600) as usize];
    match run.status.as_str() {
      "ok" => bar.ok += 1,
      "skipped" => bar.skipped += 1,
      _ => bar.failed += 1,
    }
  }
  bars
}

/// Failed runs grouped by status and error text, the largest group first and, between equals, the
/// one that failed last.
pub fn failure_groups(runs: &[RuleRun]) -> Vec<FailureGroup> {
  let mut groups: Vec<FailureGroup> = Vec::new();
  for run in runs.iter().filter(|r| is_failure(&r.status)) {
    match groups
      .iter_mut()
      .find(|g| g.status == run.status && g.error == run.error)
    {
      Some(group) => {
        group.count += 1;
        group.last_at = group.last_at.max(run.ran_at);
      }
      None => groups.push(FailureGroup {
        status: run.status.clone(),
        error: run.error.clone(),
        count: 1,
        last_at: run.ran_at,
      }),
    }
  }
  groups.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_at.cmp(&a.last_at)));
  groups
}

/// Whether a version may deploy as it stands: a dry run of its bytes under the contract dovecote
/// runs today. A test under an older contract has to be repeated.
pub fn test_badge(version: &RuleVersion) -> Badge {
  match version.tested_contract {
    Some(contract) if contract == RULE_CONTRACT as i32 => Badge::new("Tested", "badge-success"),
    Some(_) => Badge::new("Re-test: contract changed", "badge-warning"),
    None => Badge::new("Not tested", "badge-ghost"),
  }
}

/// Who wrote a version, as the version list says it.
pub fn origin_label(origin: RuleOrigin) -> &'static str {
  match origin {
    RuleOrigin::Hand => "By hand",
    RuleOrigin::Assistant => "Assistant",
    RuleOrigin::AssistantEdited => "Assistant, edited",
  }
}

/// The origin a save records: a person's change to an assistant's draft, or to an edit of one,
/// stays marked as assistant work.
pub fn save_origin(base: Option<&RuleVersion>) -> RuleOrigin {
  match base.map(|v| v.origin) {
    Some(RuleOrigin::Assistant | RuleOrigin::AssistantEdited) => RuleOrigin::AssistantEdited,
    _ => RuleOrigin::Hand,
  }
}

/// The addresses typed into the recipients field, split on commas and whitespace. dovecote checks
/// each one; this only splits.
pub fn parse_recipients(text: &str) -> Vec<String> {
  text
    .split(|c: char| c == ',' || c.is_whitespace())
    .filter(|s| !s.is_empty())
    .map(String::from)
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::macros::datetime;
  use uuid::Uuid;

  const NOW: OffsetDateTime = datetime!(2026-11-03 12:30:00 UTC);

  fn rule() -> Rule {
    Rule {
      id: Uuid::nil(),
      flock_id: Uuid::nil(),
      created_by: Uuid::nil(),
      name: "Reboot loop".into(),
      recipients: vec![],
      enabled: false,
      deployed_version: None,
      pending_version: None,
      paused_reason: None,
      blocked: false,
      fail_count: 0,
      watch_until: None,
      created_at: NOW,
      updated_at: NOW,
    }
  }

  fn version(origin: RuleOrigin, tested_contract: Option<i32>) -> RuleVersion {
    RuleVersion {
      rule_id: Uuid::nil(),
      version: 3,
      sha256: "ab".into(),
      contract: 1,
      origin,
      based_on: None,
      repair_depth: 0,
      prompt: None,
      tested_contract,
      tested_at: tested_contract.map(|_| NOW),
      test_summary: None,
      created_by: Uuid::nil(),
      created_at: NOW,
      source: None,
    }
  }

  fn run(status: &str, error: Option<&str>, ran_at: OffsetDateTime) -> RuleRun {
    RuleRun {
      rule_id: Uuid::nil(),
      version: 1,
      pigeon_id: "p".into(),
      batch_id: "b".into(),
      status: status.into(),
      readings: 1,
      alerts_sent: 0,
      alerts_suppressed: 0,
      values_written: 0,
      state_changed: false,
      wall_ms: Some(3),
      error: error.map(String::from),
      logs: vec![],
      ran_at,
    }
  }

  fn labels(badges: &[Badge]) -> Vec<&str> {
    badges.iter().map(|b| b.label.as_str()).collect()
  }

  #[test]
  fn a_new_rule_says_it_is_not_deployed() {
    assert_eq!(
      rule_badges(&rule(), NOW),
      vec![Badge::new("Not deployed", "badge-ghost")]
    );
  }

  #[test]
  fn a_running_rule_names_its_version() {
    let mut r = rule();
    r.enabled = true;
    r.deployed_version = Some(4);
    assert_eq!(
      rule_badges(&r, NOW),
      vec![Badge::new("Running v4", "badge-success")]
    );
  }

  #[test]
  fn a_platform_block_outranks_every_other_state() {
    let mut r = rule();
    r.enabled = true;
    r.deployed_version = Some(4);
    r.blocked = true;
    let badges = rule_badges(&r, NOW);
    assert_eq!(
      badges[0],
      Badge::new("Blocked by the platform", "badge-error")
    );
    assert!(!labels(&badges).contains(&"Running v4"));
  }

  #[test]
  fn pauses_say_who_paused() {
    let mut r = rule();
    r.deployed_version = Some(2);
    r.paused_reason = Some("auto:timeout".into());
    assert_eq!(
      rule_badges(&r, NOW)[0],
      Badge::new("Auto-paused: timeout", "badge-warning")
    );
    r.paused_reason = Some("owner".into());
    assert_eq!(rule_badges(&r, NOW)[0], Badge::new("Paused", "badge-ghost"));
  }

  #[test]
  fn an_unfinished_deploy_failures_and_a_watch_each_show() {
    let mut r = rule();
    r.enabled = true;
    r.deployed_version = Some(2);
    r.pending_version = Some(3);
    r.fail_count = 1;
    r.watch_until = Some(NOW + time::Duration::minutes(5));
    assert_eq!(
      labels(&rule_badges(&r, NOW)),
      vec![
        "Running v2",
        "Deploy of v3 did not finish",
        "1 failed run",
        "Recording every run"
      ]
    );
    r.fail_count = 7;
    r.watch_until = Some(NOW - time::Duration::seconds(1));
    let badges = rule_badges(&r, NOW);
    assert!(labels(&badges).contains(&"7 failed runs in a row"));
    assert!(!labels(&badges).contains(&"Recording every run"));
  }

  #[test]
  fn ok_and_skipped_are_not_failures() {
    assert!(!is_failure("ok"));
    assert!(!is_failure("skipped"));
    for status in [
      "threw",
      "timeout",
      "cpu_limit",
      "bad_output",
      "oversize",
      "version_mismatch",
      "dispatch_failed",
    ] {
      assert!(is_failure(status), "{status}");
    }
  }

  #[test]
  fn tab_replaces_the_selection_with_two_spaces() {
    assert_eq!(insert_indent("ab", 1, 1), ("a  b".into(), 3));
    assert_eq!(insert_indent("abcd", 1, 3), ("a  d".into(), 3));
    // A reversed selection is the same range.
    assert_eq!(insert_indent("abcd", 3, 1), ("a  d".into(), 3));
  }

  #[test]
  fn tab_counts_the_browser_utf16_offsets() {
    // "é" is one UTF-16 unit and two bytes; the emoji is two units and four bytes.
    assert_eq!(insert_indent("é😀x", 3, 3), ("é😀  x".into(), 5));
    // An offset past the end, or inside a surrogate pair, lands on a character boundary.
    assert_eq!(insert_indent("é", 9, 9), ("é  ".into(), 3));
    assert_eq!(insert_indent("😀", 1, 1), ("  😀".into(), 2));
  }

  #[test]
  fn bars_cover_the_window_oldest_first() {
    let runs = vec![
      run("ok", None, datetime!(2026-11-03 12:05:00 UTC)),
      run("threw", Some("x"), datetime!(2026-11-03 12:10:00 UTC)),
      run("skipped", None, datetime!(2026-11-03 11:59:59 UTC)),
      run("ok", None, datetime!(2026-11-03 10:00:00 UTC)),
      // Outside a three-hour window.
      run("ok", None, datetime!(2026-11-03 09:59:59 UTC)),
    ];
    let bars = hourly_bars(&runs, NOW, 3);
    let hour = |h: u8| {
      datetime!(2026-11-03 00:00:00 UTC)
        .replace_hour(h)
        .unwrap()
        .unix_timestamp()
    };
    assert_eq!(
      bars,
      vec![
        HourBar {
          start: hour(10),
          ok: 1,
          skipped: 0,
          failed: 0
        },
        HourBar {
          start: hour(11),
          ok: 0,
          skipped: 1,
          failed: 0
        },
        HourBar {
          start: hour(12),
          ok: 1,
          skipped: 0,
          failed: 1
        },
      ]
    );
  }

  #[test]
  fn failures_group_by_status_and_text_biggest_first() {
    let t = |m: u8| {
      datetime!(2026-11-03 12:00:00 UTC)
        .replace_minute(m)
        .unwrap()
    };
    let runs = vec![
      run("threw", Some("TypeError: x"), t(1)),
      run("ok", None, t(2)),
      run("timeout", None, t(3)),
      run("threw", Some("TypeError: x"), t(4)),
      run("threw", Some("RangeError"), t(5)),
    ];
    let groups = failure_groups(&runs);
    assert_eq!(groups.len(), 3);
    assert_eq!(
      (groups[0].status.as_str(), groups[0].error.as_deref()),
      ("threw", Some("TypeError: x"))
    );
    assert_eq!((groups[0].count, groups[0].last_at), (2, t(4)));
    // Equal counts: the most recent first.
    assert_eq!(groups[1].error.as_deref(), Some("RangeError"));
    assert_eq!((groups[2].status.as_str(), groups[2].count), ("timeout", 1));
  }

  #[test]
  fn a_version_is_tested_only_under_the_current_contract() {
    let current = RULE_CONTRACT as i32;
    assert_eq!(
      test_badge(&version(RuleOrigin::Hand, Some(current))),
      Badge::new("Tested", "badge-success")
    );
    assert_eq!(
      test_badge(&version(RuleOrigin::Hand, Some(current - 1))),
      Badge::new("Re-test: contract changed", "badge-warning")
    );
    assert_eq!(
      test_badge(&version(RuleOrigin::Hand, None)),
      Badge::new("Not tested", "badge-ghost")
    );
  }

  #[test]
  fn an_edit_of_an_assistant_draft_is_marked_as_one() {
    assert_eq!(save_origin(None), RuleOrigin::Hand);
    assert_eq!(
      save_origin(Some(&version(RuleOrigin::Hand, None))),
      RuleOrigin::Hand
    );
    for origin in [RuleOrigin::Assistant, RuleOrigin::AssistantEdited] {
      assert_eq!(
        save_origin(Some(&version(origin, None))),
        RuleOrigin::AssistantEdited
      );
    }
    assert_eq!(
      origin_label(RuleOrigin::AssistantEdited),
      "Assistant, edited"
    );
  }

  #[test]
  fn recipients_split_on_commas_and_whitespace() {
    assert_eq!(
      parse_recipients(" a@x.io, b@y.io\nc@z.io ,, "),
      vec!["a@x.io", "b@y.io", "c@z.io"]
    );
    assert!(parse_recipients("  ").is_empty());
  }
}
