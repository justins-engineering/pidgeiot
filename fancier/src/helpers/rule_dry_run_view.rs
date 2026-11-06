//! A dry run in the rule's words: what each version would have mailed, how its runs went, the
//! window replayed and the CPU check, as the rule page writes them. Every sentence comes from the
//! summary dovecote recorded; nothing here runs rule code.

use crate::components::group_thousands;
use capsules::{
  MAX_DRY_RUN_MAILS, RuleAlertKind, RuleCaseMail, RuleCaseOutcome, RuleCpuCheck, RuleCpuResult,
  RuleDryMail, RuleMailChange, RuleReplayWindow, RuleReplayed,
};

/// `n` with thousands grouped.
fn count(n: u32) -> String {
  group_thousands(&n.to_string())
}

/// `n` and the noun, singular for one.
fn counted(n: u32, one: &str, many: &str) -> String {
  [count(n).as_str(), " ", if n == 1 { one } else { many }].concat()
}

/// A span as the window line says it: days or hours to one decimal, else whole minutes.
fn span(secs: i64) -> String {
  let tenths = |unit: i64| {
    let t = (secs * 10 + unit / 2) / unit;
    let mut s = (t / 10).to_string();
    if t % 10 != 0 {
      s.push('.');
      s.push_str(&(t % 10).to_string());
    }
    s
  };
  if secs >= 86_400 {
    let n = tenths(86_400);
    let unit = if n == "1" { " day" } else { " days" };
    [n.as_str(), unit].concat()
  } else if secs >= 3_600 {
    let n = tenths(3_600);
    let unit = if n == "1" { " hour" } else { " hours" };
    [n.as_str(), unit].concat()
  } else if secs >= 60 {
    let minutes = u32::try_from(secs / 60).unwrap_or(u32::MAX);
    counted(minutes, "minute", "minutes")
  } else {
    "under a minute".to_string()
  }
}

/// The window actually replayed, such as "1.7 days of telemetry: 4,000 readings from 3 pigeons".
pub fn window_text(window: &RuleReplayWindow) -> String {
  let (Some(from), Some(to)) = (window.from, window.to) else {
    return "Nothing to replay: this flock reported no telemetry history in the last 7 days."
      .to_string();
  };
  let mut text = span((to - from).whole_seconds());
  text.push_str(" of telemetry: ");
  text.push_str(&counted(window.readings, "reading", "readings"));
  text.push_str(" from ");
  text.push_str(&counted(window.pigeons, "pigeon", "pigeons"));
  text.push_str(", replayed as ");
  text.push_str(&counted(window.batches, "batch", "batches"));
  if window.cut {
    text.push_str(". Older readings in the last 7 days were left out");
  }
  text.push('.');
  text
}

/// What a version would have mailed, such as "Would have mailed 3 times".
pub fn mails_text(replayed: &RuleReplayed) -> String {
  let mut text = match replayed.mails_total {
    0 => return "Would have sent no mail.".to_string(),
    1 => "Would have mailed once".to_string(),
    n => ["Would have mailed ", &count(n), " times"].concat(),
  };
  // Only the listed mails say whether they were held back.
  let held = replayed.mails.iter().filter(|m| !m.sent).count();
  if held > 0 {
    let n = |len: usize| u32::try_from(len).unwrap_or(u32::MAX);
    text.push_str(", ");
    text.push_str(&count(n(held)));
    if n(replayed.mails.len()) < replayed.mails_total {
      text.push_str(" of the first ");
      text.push_str(&count(n(replayed.mails.len())));
    } else {
      text.push_str(" of them");
    }
    text.push_str(" held back by the mail caps");
  }
  text.push('.');
  text
}

/// How a version's runs went, such as "40 runs: 38 fine, 2 threw".
pub fn runs_text(replayed: &RuleReplayed) -> String {
  if replayed.runs == 0 {
    return "No runs.".to_string();
  }
  let mut text = counted(replayed.runs, "run", "runs");
  text.push_str(": ");
  text.push_str(&count(replayed.ok));
  text.push_str(" fine");
  for (n, what) in [
    (replayed.threw, " threw"),
    (replayed.timeout, " ran out of time"),
    (
      replayed.bad_output,
      " returned a result the platform refuses",
    ),
  ] {
    if n > 0 {
      text.push_str(", ");
      text.push_str(&count(n));
      text.push_str(what);
    }
  }
  text.push('.');
  text
}

/// The candidate's mails against the deployed version's.
pub fn change_text(change: &RuleMailChange, deployed: Option<i32>) -> String {
  let Some(deployed) = deployed else {
    return "No other version is deployed, so every mail would be new.".to_string();
  };
  let version = ["v", &deployed.to_string()].concat();
  if change.new.is_empty() && change.gone.is_empty() {
    return ["The same mails as ", &version, "."].concat();
  }
  let mut text = listed(change.new.len());
  if let Some(first) = text.get_mut(..1) {
    first.make_ascii_uppercase();
  }
  text.push_str(if change.new.len() == 1 {
    " new mail and "
  } else {
    " new mails and "
  });
  text.push_str(&listed(change.gone.len()));
  text.push_str(" gone against ");
  text.push_str(&version);
  text.push_str("; ");
  text.push_str(&count(change.unchanged));
  text.push_str(" the same.");
  text
}

/// A mail list's length, "at least" when dovecote's cap may have cut it.
fn listed(len: usize) -> String {
  let n = count(u32::try_from(len).unwrap_or(u32::MAX));
  if len >= MAX_DRY_RUN_MAILS {
    ["at least ", &n].concat()
  } else {
    n
  }
}

/// What a mail's kind does, as the timeline says it.
pub fn kind_text(kind: RuleAlertKind) -> &'static str {
  match kind {
    RuleAlertKind::Fire => "fires",
    RuleAlertKind::Clear => "clears",
    RuleAlertKind::Notify => "notifies",
  }
}

/// One timeline line after its time: the pigeon, the rule's own subject, its key and kind.
pub fn mail_text(mail: &RuleDryMail) -> String {
  let pigeon = if mail.pigeon_name.is_empty() {
    mail.pigeon_id.get(..8).unwrap_or(&mail.pigeon_id)
  } else {
    &mail.pigeon_name
  };
  let mut text = [
    pigeon,
    ": \u{201c}",
    &mail.subject,
    "\u{201d} (",
    &mail.key,
    " ",
  ]
  .concat();
  text.push_str(kind_text(mail.kind));
  text.push(')');
  if !mail.sent {
    text.push_str(", held back by the mail caps");
  }
  text
}

fn mails_list(mails: &[RuleCaseMail]) -> String {
  if mails.is_empty() {
    return "no mail".to_string();
  }
  let parts: Vec<String> = mails
    .iter()
    .map(|m| [m.key.as_str(), " ", kind_text(m.kind)].concat())
    .collect();
  parts.join(", ")
}

/// A synthetic case's outcome in a line.
pub fn case_text(case: &RuleCaseOutcome) -> String {
  if case.passed {
    return ["Passed: ", &mails_list(&case.got), "."].concat();
  }
  let mut text = [
    "Failed: expected ",
    &mails_list(&case.expected),
    ", got ",
    &mails_list(&case.got),
  ]
  .concat();
  if let Some(error) = &case.error {
    text.push_str(". First error: ");
    text.push_str(error);
  }
  text.push('.');
  text
}

/// Failed runs of a replay.
fn failed(replayed: &RuleReplayed) -> u32 {
  replayed.threw + replayed.timeout + replayed.bad_output
}

/// The seeded replay against the fresh one: whether the state the live rule stores breaks this
/// version, which "Reset state on deploy" answers.
pub fn seeded_text(candidate: &RuleReplayed, seeded: &RuleReplayed) -> (String, bool) {
  let extra = failed(seeded).saturating_sub(failed(candidate));
  if extra == 0 {
    return (
      "From the state the live rule stores, it runs as it does from empty state.".to_string(),
      false,
    );
  }
  let text = [
    "From the state the live rule stores, ",
    &counted(extra, "more run fails", "more runs fail"),
    " than from empty state. Reset state on deploy starts it from empty state.",
  ]
  .concat();
  (text, true)
}

/// The CPU check in a sentence, and whether the page shows it as a warning. It never stops a
/// deploy.
pub fn cpu_text(cpu: &RuleCpuCheck) -> (String, bool) {
  let warns = cpu.warns();
  let busiest = counted(cpu.batches, "busiest batch", "busiest batches");
  let text = match (cpu.candidate, cpu.bundle) {
    (RuleCpuResult::DidNotLoad, _) | (_, RuleCpuResult::DidNotLoad) => {
      let mut text =
        "The flock's rules with this version did not load under the live CPU limit".to_string();
      if let Some(error) = &cpu.load_error {
        text.push_str(": ");
        text.push_str(error.trim_end_matches('.'));
      }
      text.push('.');
      text
    }
    (RuleCpuResult::Over, _) => [
      "Under the live CPU limit this version was stopped on one of its ",
      &busiest,
      ". Live, such a batch fails, and repeated failures pause the rule.",
    ]
    .concat(),
    (RuleCpuResult::Pass, RuleCpuResult::Over) => [
      "This version passed alone, but the flock's rules together were stopped on one of the ",
      &busiest,
      ": live, they share one CPU limit.",
    ]
    .concat(),
    (RuleCpuResult::Pass, _) => [
      "No batch was stopped under the live CPU limit over the ",
      &busiest,
      ". A fresh script gets about 2 s of grace, so this is no promise.",
    ]
    .concat(),
    (RuleCpuResult::NotRun, _) => {
      "The CPU check did not run: nothing to replay, or the platform was busy.".to_string()
    }
  };
  (text, warns)
}

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::RuleSeverity;
  use time::OffsetDateTime;

  fn at(secs: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(secs).ok()
  }

  fn mail(sent: bool) -> RuleDryMail {
    RuleDryMail {
      at: at(1_800_000_000).unwrap(),
      pigeon_id: "abcdef0123456789".into(),
      pigeon_name: String::new(),
      key: "too-hot".into(),
      kind: RuleAlertKind::Fire,
      severity: RuleSeverity::Warning,
      subject: "Running hot".into(),
      sent,
    }
  }

  #[test]
  fn the_window_says_what_was_actually_replayed() {
    let window = RuleReplayWindow {
      from: at(1_800_000_000),
      to: at(1_800_000_000 + 146_880),
      readings: 4_000,
      batches: 70,
      pigeons: 3,
      cut: true,
    };
    assert_eq!(
      window_text(&window),
      "1.7 days of telemetry: 4,000 readings from 3 pigeons, replayed as 70 batches. Older \
       readings in the last 7 days were left out."
    );
    let short = RuleReplayWindow {
      from: at(0),
      to: at(3_600),
      readings: 1,
      batches: 1,
      pigeons: 1,
      cut: false,
    };
    assert_eq!(
      window_text(&short),
      "1 hour of telemetry: 1 reading from 1 pigeon, replayed as 1 batch."
    );
    assert!(window_text(&RuleReplayWindow::default()).starts_with("Nothing to replay"));
    assert_eq!(span(59), "under a minute");
    assert_eq!(span(150), "2 minutes");
    assert_eq!(span(86_400 * 7), "7 days");
  }

  #[test]
  fn mails_are_counted_and_capped_ones_said_so() {
    let mut replayed = RuleReplayed::default();
    assert_eq!(mails_text(&replayed), "Would have sent no mail.");
    replayed.mails = vec![mail(true)];
    replayed.mails_total = 1;
    assert_eq!(mails_text(&replayed), "Would have mailed once.");
    replayed.mails = vec![mail(true), mail(false), mail(false)];
    replayed.mails_total = 3;
    assert_eq!(
      mails_text(&replayed),
      "Would have mailed 3 times, 2 of them held back by the mail caps."
    );
    replayed.mails_total = 1_200;
    assert_eq!(
      mails_text(&replayed),
      "Would have mailed 1,200 times, 2 of the first 3 held back by the mail caps."
    );
  }

  #[test]
  fn runs_name_only_the_outcomes_that_happened() {
    let replayed = RuleReplayed {
      runs: 40,
      ok: 37,
      threw: 2,
      bad_output: 1,
      ..RuleReplayed::default()
    };
    assert_eq!(
      runs_text(&replayed),
      "40 runs: 37 fine, 2 threw, 1 returned a result the platform refuses."
    );
    assert_eq!(runs_text(&RuleReplayed::default()), "No runs.");
  }

  #[test]
  fn the_change_is_against_the_deployed_version_by_number() {
    let change = RuleMailChange {
      new: vec![mail(true)],
      gone: vec![],
      unchanged: 4,
    };
    assert_eq!(
      change_text(&change, Some(2)),
      "1 new mail and 0 gone against v2; 4 the same."
    );
    let same = RuleMailChange {
      unchanged: 4,
      ..RuleMailChange::default()
    };
    assert_eq!(change_text(&same, Some(2)), "The same mails as v2.");
    assert!(change_text(&change, None).starts_with("No other version is deployed"));
    let capped = RuleMailChange {
      new: vec![mail(true); MAX_DRY_RUN_MAILS],
      gone: vec![mail(true); 2],
      unchanged: 0,
    };
    assert_eq!(
      change_text(&capped, Some(2)),
      "At least 100 new mails and 2 gone against v2; 0 the same."
    );
  }

  #[test]
  fn a_mail_line_is_the_rules_own_subject() {
    assert_eq!(
      mail_text(&mail(true)),
      "abcdef01: \u{201c}Running hot\u{201d} (too-hot fires)"
    );
    let mut named = mail(false);
    named.pigeon_name = "Lobby 2".into();
    named.kind = RuleAlertKind::Clear;
    assert_eq!(
      mail_text(&named),
      "Lobby 2: \u{201c}Running hot\u{201d} (too-hot clears), held back by the mail caps"
    );
  }

  #[test]
  fn a_case_says_what_it_expected_and_got() {
    let fire = RuleCaseMail {
      key: "too-hot".into(),
      kind: RuleAlertKind::Fire,
    };
    let pass = RuleCaseOutcome {
      name: "just over".into(),
      passed: true,
      expected: vec![fire.clone()],
      got: vec![fire.clone()],
      error: None,
    };
    assert_eq!(case_text(&pass), "Passed: too-hot fires.");
    let fail = RuleCaseOutcome {
      passed: false,
      got: vec![],
      error: Some("TypeError: x".into()),
      ..pass
    };
    assert_eq!(
      case_text(&fail),
      "Failed: expected too-hot fires, got no mail. First error: TypeError: x."
    );
  }

  #[test]
  fn the_cpu_check_warns_but_a_pass_promises_nothing() {
    let check = |candidate, bundle| RuleCpuCheck {
      candidate,
      bundle,
      batches: 10,
      load_error: None,
    };
    use RuleCpuResult::*;
    let (text, warn) = cpu_text(&check(Pass, Pass));
    assert!(!warn && text.ends_with("so this is no promise."));
    assert!(text.contains("10 busiest batches"));
    let (text, warn) = cpu_text(&check(Over, Pass));
    assert!(warn && text.contains("this version was stopped"));
    let (text, warn) = cpu_text(&check(Pass, Over));
    assert!(warn && text.contains("rules together"));
    let (text, warn) = cpu_text(&RuleCpuCheck {
      load_error: Some("Script startup exceeded CPU time limit.".into()),
      ..check(DidNotLoad, DidNotLoad)
    });
    assert!(warn && text.ends_with("exceeded CPU time limit."), "{text}");
    let (text, warn) = cpu_text(&check(NotRun, NotRun));
    assert!(!warn && text.starts_with("The CPU check did not run"));
  }

  #[test]
  fn live_state_that_breaks_the_version_points_at_the_reset() {
    let fresh = RuleReplayed {
      threw: 1,
      ..RuleReplayed::default()
    };
    let seeded = RuleReplayed {
      threw: 4,
      ..RuleReplayed::default()
    };
    let (text, warn) = seeded_text(&fresh, &seeded);
    assert!(warn && text.contains("3 more runs fail"));
    assert!(!seeded_text(&fresh, &fresh).1);
  }
}
