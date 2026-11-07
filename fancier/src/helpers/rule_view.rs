//! The pure parts of the rule pages: the status badges a rule shows, the editor's Tab key, the
//! runs view's hourly bars and failure groups, and the labels a version carries.

use capsules::{MAX_RULE_RUNS_PER_PAGE, RULE_CONTRACT, Rule, RuleOrigin, RuleRun, RuleVersion};
use time::OffsetDateTime;

/// One status badge: its text and the daisyUI badge class that colours it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
  /// What the badge says.
  pub label: String,
  /// Its daisyUI colour class, such as `badge-error`.
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
  /// Runs that answered `ok`.
  pub ok: u32,
  /// Runs the harness ran out of budget before reaching.
  pub skipped: u32,
  /// Every other status.
  pub failed: u32,
}

/// Failed runs that share a status and error text.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureGroup {
  /// The run status the group shares.
  pub status: String,
  /// The error text it shares, the rule's own.
  pub error: Option<String>,
  /// How many recorded runs it holds.
  pub count: usize,
  /// When the newest of them ran.
  pub last_at: OffsetDateTime,
}

/// A version number as the pages show it: `v12`.
pub fn version_label(version: i32) -> String {
  let number = version.to_string();
  let mut label = String::with_capacity(1 + number.len());
  label.push('v');
  label.push_str(&number);
  label
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
      let version = version_label(version);
      let mut label = String::with_capacity(8 + version.len());
      label.push_str("Running ");
      label.push_str(&version);
      badges.push(Badge::new(label, "badge-success"));
    }
    Some(_) => {}
  }
  if let Some(pending) = rule.pending_version {
    let pending = version_label(pending);
    let mut label = String::with_capacity(25 + pending.len());
    label.push_str("Deploy of ");
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

/// What a deploy's answer means: the rule only says a version is live once `deployed_version`
/// names it and no upload of it is pending. Anything else is an unfinished deploy.
pub fn deploy_outcome(rule: &Rule, version: i32) -> Result<String, String> {
  if rule.deployed_version == Some(version) && rule.pending_version != Some(version) {
    let label = version_label(version);
    let mut note = String::with_capacity(46 + label.len());
    note.push_str("Deployed ");
    note.push_str(&label);
    note.push_str(". Rules start within about a minute.");
    Ok(note)
  } else {
    Err(unfinished_deploy(rule, version))
  }
}

/// The notice for a deploy of `version` that did not finish. The old bundle stays live, so a rule
/// that never deployed is still not running.
pub fn unfinished_deploy(rule: &Rule, version: i32) -> String {
  let label = version_label(version);
  let tail = match rule.deployed_version {
    None => " did not finish. The rule is not running yet.",
    Some(_) => " did not finish. The rule stays as it was.",
  };
  let mut note = String::with_capacity(14 + label.len() + tail.len());
  note.push_str("The deploy of ");
  note.push_str(&label);
  note.push_str(tail);
  note
}

/// Whether a run status is a failure. `skipped` is not: the harness ran out of its budget before
/// reaching the rule, which says nothing about the rule's code.
pub fn is_failure(status: &str) -> bool {
  !matches!(status, "ok" | "skipped")
}

/// Replaces the selection `start..end` with two spaces, the editor's Tab key, and returns the new
/// text with the caret after them. Offsets are the textarea's UTF-16 units, not bytes; one that
/// falls past the end or inside a surrogate pair moves back to a character boundary.
#[cfg(any(feature = "web", test))]
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
#[cfg(any(feature = "web", test))]
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

/// How many hours of bars the runs can fill, at most `hours`. The runs route answers only the
/// newest [`MAX_RULE_RUNS_PER_PAGE`], so a full page says nothing about the hours before its oldest
/// run; the bars start there rather than draw those hours as quiet.
pub fn bar_hours(runs: &[RuleRun], now: OffsetDateTime, hours: usize) -> usize {
  let oldest = runs.iter().map(|r| r.ran_at.unix_timestamp()).min();
  match oldest {
    Some(oldest) if runs.len() >= MAX_RULE_RUNS_PER_PAGE as usize => {
      let top = now.unix_timestamp() - now.unix_timestamp().rem_euclid(3600);
      let first = oldest - oldest.rem_euclid(3600);
      let span = ((top - first) / 3600 + 1).max(1) as usize;
      span.min(hours)
    }
    _ => hours,
  }
}

/// The left end of the bars' axis.
pub fn hours_ago(hours: usize) -> String {
  if hours <= 1 {
    return "this hour".to_string();
  }
  let count = hours.to_string();
  let mut label = String::with_capacity(10 + count.len());
  label.push_str(&count);
  label.push_str(" hours ago");
  label
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
    RuleOrigin::Builder => "Rule builder",
  }
}

/// The origin a code save records: a person's change to an assistant's draft, or to an edit of
/// one, stays marked as assistant work, and code converted from a builder version is by hand.
pub fn save_origin(base: Option<&RuleVersion>) -> RuleOrigin {
  match base.map(|v| v.origin) {
    Some(RuleOrigin::Assistant | RuleOrigin::AssistantEdited) => RuleOrigin::AssistantEdited,
    _ => RuleOrigin::Hand,
  }
}

/// Whether the flock page shows its Rules card: only once the rules list has answered. A refusal
/// is what a flock outside the platform's rules allowlist gets, and a failed read shows nothing
/// rather than a card that may lead nowhere.
pub fn rules_card_shown(list: Option<&Result<Vec<Rule>, String>>) -> bool {
  matches!(list, Some(Ok(_)))
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
      builder_model: None,
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
  fn a_full_page_of_runs_starts_the_bars_at_its_oldest_run() {
    let page = |n: usize, oldest: OffsetDateTime| {
      let mut runs = vec![run("ok", None, NOW); n - 1];
      runs.push(run("ok", None, oldest));
      runs
    };
    let full = MAX_RULE_RUNS_PER_PAGE as usize;
    // Short of a full page, the list holds every recorded run of the last day.
    assert_eq!(
      bar_hours(&page(full - 1, datetime!(2026-11-03 10:15:00 UTC)), NOW, 24),
      24
    );
    // A full page from 10:15 covers the hours 10, 11 and 12 and says nothing earlier.
    assert_eq!(
      bar_hours(&page(full, datetime!(2026-11-03 10:15:00 UTC)), NOW, 24),
      3
    );
    assert_eq!(bar_hours(&page(full, NOW), NOW, 24), 1);
    // A full page older than the window still fills it.
    assert_eq!(
      bar_hours(&page(full, datetime!(2026-11-01 08:00:00 UTC)), NOW, 24),
      24
    );
  }

  #[test]
  fn hours_ago_names_the_start_of_the_bars() {
    assert_eq!(hours_ago(24), "24 hours ago");
    assert_eq!(hours_ago(1), "this hour");
    assert_eq!(hours_ago(3), "3 hours ago");
  }

  #[test]
  fn a_deploy_is_reported_only_once_it_went_live() {
    let mut live = rule();
    live.enabled = true;
    live.deployed_version = Some(4);
    assert_eq!(
      deploy_outcome(&live, 4),
      Ok("Deployed v4. Rules start within about a minute.".to_string())
    );
    // Answered, but the upload did not finish: the first deploy leaves nothing running.
    let mut first = rule();
    first.pending_version = Some(4);
    assert_eq!(
      deploy_outcome(&first, 4),
      Err("The deploy of v4 did not finish. The rule is not running yet.".to_string())
    );
    // A later one leaves the old version as it was.
    let mut later = live.clone();
    later.deployed_version = Some(3);
    later.pending_version = Some(4);
    assert_eq!(
      deploy_outcome(&later, 4),
      Err("The deploy of v4 did not finish. The rule stays as it was.".to_string())
    );
    later.pending_version = None;
    assert!(deploy_outcome(&later, 4).is_err());
  }

  #[test]
  fn versions_read_as_v_and_their_number() {
    assert_eq!(version_label(12), "v12");
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
    assert_eq!(
      save_origin(Some(&version(RuleOrigin::Builder, None))),
      RuleOrigin::Hand,
      "converting a builder version to code is a hand version"
    );
    assert_eq!(origin_label(RuleOrigin::Builder), "Rule builder");
  }

  #[test]
  fn the_rules_card_waits_for_the_list_and_hides_on_a_refusal() {
    assert!(!rules_card_shown(None), "nothing before the answer");
    let refused = Err("Forbidden: rules are not enabled for this organization".to_string());
    assert!(!rules_card_shown(Some(&refused)));
    assert!(
      rules_card_shown(Some(&Ok(vec![]))),
      "an empty list still opens rules"
    );
    assert!(rules_card_shown(Some(&Ok(vec![rule()]))));
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
