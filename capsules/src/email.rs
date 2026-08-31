//! The two customer-facing transactional emails dovecote sends on a
//! user's behalf: organization invitations and alert notifications.
//!
//! The formatting lives here rather than next to the senders for the same
//! reason `feedback` does: it is pure string logic, and this crate's tests
//! run on a host target. Both messages are assembled into one `Document`
//! and rendered twice, as HTML and as plain text, so the two parts say the
//! same thing by construction rather than by discipline.
//!
//! The HTML is the conservative kind that survives Gmail, Outlook and Apple
//! Mail: a single 600px table column, every style inline, a system font
//! stack, no images. Colors are the dashboard's own DaisyUI theme, so a
//! message looks like it came from the same product as the page it links
//! to. Every background is painted explicitly and the dark palette is
//! applied through a `prefers-color-scheme` override; a client that strips
//! the style block still gets readable dark text on a light card instead of
//! white text on whatever it decided the background should be.

use crate::{
  AlertCondition, AlertDefinition, AlertSeverity, Comparator, ConnectionStateKind, OrgRole,
};
use time::{OffsetDateTime, UtcOffset};

/// One rendered message: the subject line and both body parts. The `text`
/// part is not a fallback afterthought; it is the same document rendered
/// for a reader who turned HTML off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMessage {
  pub subject: String,
  pub text: String,
  pub html: String,
}

/// One instant as an organization sees it: how far its zone stood from UTC
/// at that moment, and what that period is called there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTime {
  pub offset: UtcOffset,
  /// The zone's own abbreviation for that period ("EDT"). Empty is
  /// allowed and renders as the offset instead.
  pub abbreviation: String,
}

/// Resolves an instant into one organization's local wall clock.
///
/// A trait rather than a zone name because the timezone database that
/// answers it is a server dependency: this crate is compiled into the
/// dashboard's wasm bundle too, and a second copy of the database would be
/// downloaded by every visitor to spell a handful of email timestamps.
pub trait LocalZone {
  /// `None` when the zone cannot answer for this instant, which reads the
  /// same as having no zone at all: the stamp falls back to UTC rather
  /// than the message failing to send.
  fn local_time(&self, at: OffsetDateTime) -> Option<LocalTime>;
}

/// The clock a message is written against: an organization's own zone when
/// one is known, UTC otherwise.
#[derive(Clone, Copy)]
pub struct Clock<'a> {
  zone: Option<&'a dyn LocalZone>,
}

impl<'a> Clock<'a> {
  /// For a message with no organization behind it, and for one whose
  /// stored zone could not be resolved.
  pub fn utc() -> Self {
    Clock { zone: None }
  }

  pub fn zoned(zone: &'a dyn LocalZone) -> Self {
    Clock { zone: Some(zone) }
  }

  /// One instant in local time with UTC beside it, so a reader who works
  /// in the zone and one who reads the logs in UTC both get their own
  /// answer without doing arithmetic: "26 Aug 2026, 15:10 EDT (19:10
  /// UTC)". The date is repeated inside the parentheses only when the two
  /// zones disagree about which day it is.
  fn stamp(&self, t: OffsetDateTime, precision: Precision) -> String {
    let utc = t.to_offset(UtcOffset::UTC);
    let local = self.zone.and_then(|zone| zone.local_time(t));
    let Some(local) = local else {
      return format!("{} UTC", wall_clock(utc, precision));
    };
    let label = zone_label(&local);
    // An organization on UTC would otherwise be told the same time twice.
    if local.offset == UtcOffset::UTC && label == "UTC" {
      return format!("{} UTC", wall_clock(utc, precision));
    }
    let there = t.to_offset(local.offset);
    let beside = if there.date() == utc.date() {
      clock_time(utc, precision)
    } else {
      wall_clock(utc, precision)
    };
    format!("{} {label} ({beside} UTC)", wall_clock(there, precision))
  }

  fn minutes(&self, t: OffsetDateTime) -> String {
    self.stamp(t, Precision::Minutes)
  }

  fn seconds(&self, t: OffsetDateTime) -> String {
    self.stamp(t, Precision::Seconds)
  }

  /// The year a moment falls in locally, for the copyright line.
  fn year(&self, t: OffsetDateTime) -> i32 {
    match self.zone.and_then(|zone| zone.local_time(t)) {
      Some(local) => t.to_offset(local.offset).year(),
      None => t.to_offset(UtcOffset::UTC).year(),
    }
  }
}

impl std::fmt::Debug for Clock<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self.zone {
      Some(_) => f.write_str("Clock(zoned)"),
      None => f.write_str("Clock(UTC)"),
    }
  }
}

/// What the evaluator saw when an alert changed state, so the message can
/// name the observed value next to the configured one. Which variant
/// applies follows from the condition kind; `Silence` is what the
/// scheduled sweep knows about a pigeon that stopped reporting.
#[derive(Debug, Clone, PartialEq)]
pub enum AlertObservation {
  Value { observed: f64 },
  Change { previous: f64, observed: f64 },
  Silence { last_seen: Option<OffsetDateTime> },
}

/// Everything the invitation email says. The inviter is named from the
/// inviting account's identity traits: "Ana Ruiz (ana@example.com)" when
/// both are present, whichever one exists otherwise, and the organization
/// itself when neither does.
#[derive(Debug, Clone, PartialEq)]
pub struct InviteEmail<'a> {
  pub inviter_name: Option<&'a str>,
  pub inviter_email: Option<&'a str>,
  pub org_name: &'a str,
  pub role: OrgRole,
  pub invite_url: &'a str,
  pub expires_at: OffsetDateTime,
  pub sent_at: OffsetDateTime,
}

/// Everything the alert notification says. `pigeon_url` opens the pigeon
/// in the dashboard; `manage_url` lands on the alerts section the
/// definition is edited from, which differs for pigeon- and flock-scoped
/// alerts.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertEmail<'a> {
  pub definition: &'a AlertDefinition,
  pub fired: bool,
  pub pigeon_id: &'a str,
  pub pigeon_name: Option<&'a str>,
  pub flock_name: Option<&'a str>,
  pub observation: Option<&'a AlertObservation>,
  pub at: OffsetDateTime,
  pub pigeon_url: &'a str,
  pub manage_url: &'a str,
}

const SITE_URL: &str = "https://pidgeiot.com";
const SENDER: &str = "Justin's Engineering Services, LLC";

pub fn format_invite_email(invite: &InviteEmail<'_>, clock: Clock<'_>) -> EmailMessage {
  let org = inline(invite.org_name);
  let inviter_name = invite.inviter_name.map(inline).filter(|s| !s.is_empty());
  let inviter_email = invite.inviter_email.map(inline).filter(|s| !s.is_empty());
  let inviter = match (inviter_name, inviter_email) {
    (Some(name), Some(email)) => Some(format!("{name} ({email})")),
    (Some(name), None) => Some(name),
    (None, Some(email)) => Some(email),
    (None, None) => None,
  };
  let role = invite.role.as_str();
  let expires = clock.minutes(invite.expires_at);

  let who = match &inviter {
    Some(inviter) => inviter.clone(),
    None => format!("A member of {org}"),
  };
  let ask_again = match &inviter {
    Some(inviter) => format!("ask {inviter} to send a new one"),
    None => "ask the person who invited you to send a new one".to_string(),
  };

  let doc = Document {
    subject: format!("[PidgeIoT] Invitation to join {org}"),
    preheader: format!(
      "{who} invited you to join {org} as {} {role}.",
      article(role)
    ),
    chip: Some(Chip {
      label: "Invitation",
      tone: Tone::Brand,
    }),
    title: format!("Join {org} on PidgeIoT"),
    lead: vec![
      format!(
        "{who} has invited you to join the organization {org} as {} {role}.",
        article(role)
      ),
      format!(
        "As {} {role} you can {}.",
        article(role),
        role_powers(&invite.role)
      ),
    ],
    facts: vec![
      Fact::new("Organization", org.clone()),
      Fact::new(
        "Invited by",
        inviter
          .clone()
          .unwrap_or_else(|| "not recorded".to_string()),
      ),
      Fact::new("Your role", capitalize(role)),
      Fact::new(
        "Link expires",
        // Not a parenthetical: a local stamp already carries one, and two
        // in a row read as a stutter.
        format!(
          "{expires}, {} from now",
          humanize_secs((invite.expires_at - invite.sent_at).whole_seconds())
        ),
      ),
    ],
    action: Some(Action {
      label: "Accept invitation",
      url: invite.invite_url.to_string(),
    }),
    notes: vec![
      "You will be asked to sign in, or to create a PidgeIoT account first. The link then adds \
       you to the organization."
        .to_string(),
      format!(
        "The link is single-use and stops working on {expires}. If it has already expired, \
         {ask_again}."
      ),
      "If you weren't expecting this invitation, you can ignore this email: nothing changes \
       unless the link is used. It works for whoever signs in with it, so please don't forward it."
        .to_string(),
    ],
    reason: format!("this address was invited to join {org} on PidgeIoT"),
    sent_at: invite.sent_at,
    clock,
  };

  doc.render()
}

pub fn format_alert_email(alert: &AlertEmail<'_>, clock: Clock<'_>) -> EmailMessage {
  let def = alert.definition;
  let name = inline(&def.name);
  let pigeon = alert
    .pigeon_name
    .map(inline)
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| inline(alert.pigeon_id));
  let flock = alert.flock_name.map(inline).filter(|s| !s.is_empty());
  let location = match &flock {
    Some(flock) => format!("{pigeon} in flock {flock}"),
    None => pigeon.clone(),
  };
  let when = clock.seconds(alert.at);
  let state = if alert.fired { "firing" } else { "resolved" };

  let chip = match (alert.fired, &def.severity) {
    (true, AlertSeverity::Critical) => Chip {
      label: "Critical alert firing",
      tone: Tone::Critical,
    },
    (true, AlertSeverity::Warning) => Chip {
      label: "Alert firing",
      tone: Tone::Warning,
    },
    (false, _) => Chip {
      label: "Resolved",
      tone: Tone::Resolved,
    },
  };

  let mut facts = vec![
    Fact::new("Alert", name.clone()),
    Fact::new("Severity", capitalize(def.severity.as_str())),
    Fact::new("State", capitalize(state)),
    Fact::new(
      "Pigeon",
      if alert.pigeon_name.is_some() {
        format!("{pigeon} ({})", inline(alert.pigeon_id))
      } else {
        pigeon.clone()
      },
    ),
  ];
  if let Some(flock) = &flock {
    facts.push(Fact::new("Flock", flock.clone()));
  }
  facts.extend(condition_facts(&def.condition));
  facts.push(Fact::new(
    "Observed",
    observation_text(
      &def.condition,
      alert.observation,
      alert.at,
      alert.fired,
      clock,
    ),
  ));
  facts.push(Fact::new("When", when.clone()));

  let mut lead = vec![if alert.fired {
    format!("{location} has met the condition of the alert {name}.")
  } else {
    format!("{location} no longer meets the condition of the alert {name}.")
  }];
  // The operator's own words sit above the facts table, where "check the
  // cabinet breaker first" is read before the numbers rather than after.
  lead.extend(note_paragraphs(alert.definition.notes.as_deref()));

  let kind = match (alert.fired, &def.severity) {
    (true, AlertSeverity::Critical) => "Critical alert",
    _ => "Alert",
  };

  let doc = Document {
    subject: format!(
      "[PidgeIoT] {kind} {state}: {} on {pigeon}",
      subject_metric(&def.condition)
    ),
    preheader: format!("{name} is {state} on {location} as of {when}."),
    chip: Some(chip),
    title: if alert.fired {
      format!("{name} is firing")
    } else {
      format!("{name} has resolved")
    },
    lead,
    facts,
    action: Some(Action {
      label: "View pigeon in dashboard",
      url: alert.pigeon_url.to_string(),
    }),
    notes: vec![format!(
      "To change, pause or delete this alert, open the Alerts section of the dashboard: {}",
      alert.manage_url
    )],
    reason: "an alert configured on PidgeIoT names this address as its recipient".to_string(),
    sent_at: alert.at,
    clock,
  };

  doc.render()
}

/// One paragraph per line the operator typed. `inline` per line rather
/// than over the whole note, so a runbook written as a short list keeps its
/// breaks instead of collapsing into a wall of text.
fn note_paragraphs(notes: Option<&str>) -> Vec<String> {
  notes
    .unwrap_or_default()
    .lines()
    .map(inline)
    .filter(|line| !line.is_empty())
    .collect()
}

// --- Content vocabulary ---

/// What the role lets the invitee do, phrased for someone who has not seen
/// the permission matrix. Mirrors `docs/api.md`'s organization table.
fn role_powers(role: &OrgRole) -> &'static str {
  match role {
    OrgRole::Owner => {
      "manage everything in the organization: members and their roles, billing, flocks, \
       devices and alerts"
    }
    OrgRole::Admin => {
      "manage the organization's flocks, devices and alerts, invite and remove members, and \
       manage billing"
    }
    OrgRole::Member => "view the organization's flocks, devices, telemetry and alerts",
  }
}

fn article(word: &str) -> &'static str {
  match word.chars().next().map(|c| c.to_ascii_lowercase()) {
    Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
    _ => "a",
  }
}

/// The short noun the subject line keys on, so an inbox sorted by subject
/// groups one metric's alerts together.
fn subject_metric(condition: &AlertCondition) -> String {
  match condition {
    AlertCondition::Threshold { key, .. } | AlertCondition::RateOfChange { key, .. } => inline(key),
    AlertCondition::DeviceState { state, .. } => match state {
      ConnectionStateKind::Offline => "device offline".to_string(),
      ConnectionStateKind::Stale => "device stale".to_string(),
    },
    AlertCondition::MissingReport { .. } => "missing reports".to_string(),
  }
}

fn comparator_phrase(comparator: &Comparator) -> &'static str {
  match comparator {
    Comparator::Gt => "greater than",
    Comparator::Gte => "at least",
    Comparator::Lt => "less than",
    Comparator::Lte => "at most",
    Comparator::Eq => "equal to",
  }
}

fn condition_facts(condition: &AlertCondition) -> Vec<Fact> {
  match condition {
    AlertCondition::Threshold {
      key,
      comparator,
      value,
    } => vec![
      Fact::new("Metric", inline(key)),
      Fact::new(
        "Threshold",
        format!("{} {}", comparator_phrase(comparator), fmt_num(*value)),
      ),
    ],
    AlertCondition::RateOfChange {
      key,
      max_delta,
      window_secs,
    } => {
      let window = window_secs
        .map(|w| format!(" within {}", humanize_secs(w)))
        .unwrap_or_default();
      vec![
        Fact::new("Metric", inline(key)),
        Fact::new(
          "Threshold",
          format!("changes by more than {}{window}", fmt_num(*max_delta)),
        ),
      ]
    }
    AlertCondition::DeviceState {
      state,
      min_duration_secs,
    } => {
      let held = min_duration_secs
        .map(|s| format!(" for at least {}", humanize_secs(s)))
        .unwrap_or_default();
      let state = match state {
        ConnectionStateKind::Offline => "offline",
        ConnectionStateKind::Stale => "stale",
      };
      vec![Fact::new("Condition", format!("device {state}{held}"))]
    }
    AlertCondition::MissingReport { max_silence_secs } => vec![Fact::new(
      "Condition",
      format!("no report for {}", humanize_secs(*max_silence_secs)),
    )],
  }
}

fn observation_text(
  condition: &AlertCondition,
  observation: Option<&AlertObservation>,
  at: OffsetDateTime,
  fired: bool,
  clock: Clock<'_>,
) -> String {
  match (condition, observation) {
    (AlertCondition::Threshold { .. }, Some(AlertObservation::Value { observed })) => {
      fmt_num(*observed)
    }
    (
      AlertCondition::RateOfChange { .. },
      Some(AlertObservation::Change { previous, observed }),
    ) => {
      let delta = observed - previous;
      let sign = if delta >= 0.0 { "+" } else { "" };
      format!(
        "{} to {} ({sign}{})",
        fmt_num(*previous),
        fmt_num(*observed),
        fmt_num(delta)
      )
    }
    (
      AlertCondition::DeviceState { .. } | AlertCondition::MissingReport { .. },
      Some(AlertObservation::Silence { last_seen }),
    ) => match last_seen {
      None => "never reported".to_string(),
      Some(seen) => {
        let age = humanize_secs((at - *seen).whole_seconds().max(0));
        if fired {
          format!("silent for {age}, last report {}", clock.seconds(*seen))
        } else {
          format!("reporting again, last report {}", clock.seconds(*seen))
        }
      }
    },
    _ => "not captured".to_string(),
  }
}

// --- Formatting helpers ---

/// Single-line form of a user-supplied name: control characters and runs
/// of whitespace collapse to one space so a name can never break the
/// subject line, a fact row, or the plain-text layout.
fn inline(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut pending_space = false;
  for c in s.trim().chars() {
    if c.is_whitespace() || c.is_control() {
      pending_space = true;
      continue;
    }
    if pending_space && !out.is_empty() {
      out.push(' ');
    }
    pending_space = false;
    out.push(c);
  }
  out
}

fn capitalize(s: &str) -> String {
  let mut chars = s.chars();
  match chars.next() {
    Some(first) => first.to_uppercase().chain(chars).collect(),
    None => String::new(),
  }
}

/// Shortest decimal form, `30` for a whole number and `29.75` otherwise,
/// rounded to six places so a computed difference such as `1001.5 -
/// 1013.2` prints as `-11.7` rather than its binary residue.
fn fmt_num(value: f64) -> String {
  let rounded = (value * 1e6).round() / 1e6;
  format!("{rounded}")
}

fn month_abbr(month: time::Month) -> &'static str {
  match month {
    time::Month::January => "Jan",
    time::Month::February => "Feb",
    time::Month::March => "Mar",
    time::Month::April => "Apr",
    time::Month::May => "May",
    time::Month::June => "Jun",
    time::Month::July => "Jul",
    time::Month::August => "Aug",
    time::Month::September => "Sep",
    time::Month::October => "Oct",
    time::Month::November => "Nov",
    time::Month::December => "Dec",
  }
}

/// How much of the wall clock a stamp shows. An alert names the second it
/// was observed; an expiry a week out does not pretend to that precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Precision {
  Minutes,
  Seconds,
}

/// "26 Aug 2026, 15:10:09".
fn wall_clock(t: OffsetDateTime, precision: Precision) -> String {
  format!(
    "{} {} {}, {}",
    t.day(),
    month_abbr(t.month()),
    t.year(),
    clock_time(t, precision)
  )
}

/// "15:10:09".
fn clock_time(t: OffsetDateTime, precision: Precision) -> String {
  match precision {
    Precision::Minutes => format!("{:02}:{:02}", t.hour(), t.minute()),
    Precision::Seconds => format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()),
  }
}

/// What to call the zone next to a local time: its own abbreviation when
/// it has one, and the offset when it does not (a handful of zones carry
/// no name in the database, only a numeric designation).
fn zone_label(local: &LocalTime) -> String {
  let abbreviation = local.abbreviation.trim();
  if !abbreviation.is_empty() {
    return abbreviation.to_string();
  }
  let (hours, minutes, _) = local.offset.as_hms();
  let sign = if local.offset.whole_seconds() < 0 {
    '-'
  } else {
    '+'
  };
  format!(
    "UTC{sign}{:02}:{:02}",
    hours.unsigned_abs(),
    minutes.unsigned_abs()
  )
}

/// Largest two units of a duration, so "1 hour 30 minutes" rather than
/// "5400 seconds" or "1.5 hours".
fn humanize_secs(secs: i64) -> String {
  const UNITS: [(&str, i64); 4] = [
    ("day", 86_400),
    ("hour", 3_600),
    ("minute", 60),
    ("second", 1),
  ];
  let mut remaining = secs.max(0);
  let mut parts = Vec::with_capacity(2);
  for (unit, size) in UNITS {
    if parts.len() == 2 {
      break;
    }
    let count = remaining / size;
    if count > 0 || (parts.is_empty() && size == 1) {
      let plural = if count == 1 { "" } else { "s" };
      parts.push(format!("{count} {unit}{plural}"));
      remaining -= count * size;
    } else if !parts.is_empty() {
      break;
    }
  }
  parts.join(" ")
}

pub fn html_escape(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&#39;"),
      _ => out.push(c),
    }
  }
  out
}

// --- The shared layout ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
  Brand,
  Warning,
  Critical,
  Resolved,
}

impl Tone {
  /// DaisyUI's `badge-accent`/`badge-warning`/`badge-error`/`badge-success`
  /// pairs from the dashboard theme. Each pair is legible on both the light
  /// and the dark card, which is why the chip needs no dark override.
  fn colors(self) -> (&'static str, &'static str) {
    match self {
      Tone::Brand => ("#ffd6a7", "#7c2808"),
      Tone::Warning => ("#fcb700", "#793205"),
      Tone::Critical => ("#ff627d", "#4d0218"),
      Tone::Resolved => ("#00d390", "#004c39"),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Chip {
  label: &'static str,
  tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fact {
  label: &'static str,
  value: String,
}

impl Fact {
  fn new(label: &'static str, value: String) -> Self {
    Self { label, value }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Action {
  label: &'static str,
  url: String,
}

/// The one shape both messages share. Rendering it twice is what keeps the
/// HTML and text parts in agreement.
#[derive(Debug, Clone)]
struct Document<'a> {
  subject: String,
  preheader: String,
  chip: Option<Chip>,
  title: String,
  lead: Vec<String>,
  facts: Vec<Fact>,
  action: Option<Action>,
  notes: Vec<String>,
  reason: String,
  sent_at: OffsetDateTime,
  clock: Clock<'a>,
}

const FONT: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";

/// One theme's colors as sRGB hex, converted from the dashboard's DaisyUI
/// theme so a message matches the page it links to. Every text/background
/// pair the layout paints from these is held to WCAG AA by the unit tests.
#[derive(Debug, Clone, Copy)]
struct Palette {
  canvas: &'static str,
  card: &'static str,
  rule: &'static str,
  ink: &'static str,
  muted: &'static str,
  link: &'static str,
  wordmark: &'static str,
  button: &'static str,
  button_text: &'static str,
}

const LIGHT: Palette = Palette {
  canvas: "#f5f5f4",
  card: "#ffffff",
  rule: "#e6e4e3",
  ink: "#1b1816",
  muted: "#6b6461",
  link: "#00776f",
  wordmark: "#00776f",
  button: "#9605f7",
  button_text: "#f8f3fd",
};

const DARK: Palette = Palette {
  canvas: "#1b1816",
  card: "#272322",
  rule: "#3a3432",
  ink: "#ecf9ff",
  muted: "#b9c4cc",
  link: "#00d3bb",
  wordmark: "#c079ff",
  button: "#00d3bb",
  button_text: "#002d2c",
};

impl Palette {
  /// The `prefers-color-scheme: dark` overrides, keyed by the classes the
  /// inline styles also carry. `!important` because an inline style would
  /// otherwise win over a stylesheet rule.
  fn dark_overrides(&self) -> String {
    let Palette {
      canvas,
      card,
      rule,
      ink,
      muted,
      link,
      wordmark,
      button,
      button_text,
    } = *self;
    format!(
      "@media (prefers-color-scheme:dark){{\
       .canvas{{background-color:{canvas}!important;}}\
       .card{{background-color:{card}!important;border-color:{rule}!important;}}\
       .ink{{color:{ink}!important;}}\
       .muted{{color:{muted}!important;}}\
       .rule{{border-top-color:{rule}!important;}}\
       .wordmark{{color:{wordmark}!important;}}\
       .btn{{background-color:{button}!important;}}\
       .btn a{{color:{button_text}!important;}}\
       .link{{color:{link}!important;}}\
       }}"
    )
  }
}

impl Document<'_> {
  fn render(&self) -> EmailMessage {
    EmailMessage {
      subject: self.subject.clone(),
      text: self.render_text(),
      html: self.render_html(),
    }
  }

  fn render_text(&self) -> String {
    let mut out = String::new();
    out.push_str("PidgeIoT\n\n");
    if let Some(chip) = &self.chip {
      out.push_str(&chip.label.to_uppercase());
      out.push('\n');
    }
    out.push_str(&self.title);
    out.push('\n');
    out.push_str(&"=".repeat(self.title.chars().count()));
    out.push_str("\n\n");
    for paragraph in &self.lead {
      out.push_str(paragraph);
      out.push_str("\n\n");
    }
    for fact in &self.facts {
      out.push_str(fact.label);
      out.push_str(": ");
      out.push_str(&fact.value);
      out.push('\n');
    }
    if let Some(action) = &self.action {
      out.push('\n');
      out.push_str(action.label);
      out.push_str(":\n");
      out.push_str(&action.url);
      out.push('\n');
    }
    for paragraph in &self.notes {
      out.push('\n');
      out.push_str(paragraph);
      out.push('\n');
    }
    out.push_str("\n-- \n");
    out.push_str(&format!("Sent by PidgeIoT, a product of {SENDER}.\n"));
    out.push_str(&format!(
      "You received this email because {}.\n",
      self.reason
    ));
    out.push_str(&format!(
      "{SITE_URL}\n(c) {} {SENDER}\n",
      self.clock.year(self.sent_at)
    ));
    out
  }

  fn render_html(&self) -> String {
    let Palette {
      canvas,
      card,
      rule,
      ink,
      muted,
      link,
      wordmark,
      button,
      button_text,
    } = LIGHT;
    let dark_overrides = DARK.dark_overrides();
    let mut body = String::new();

    if let Some(chip) = &self.chip {
      let (bg, fg) = chip.tone.colors();
      body.push_str(&format!(
        "<p style=\"margin:0 0 14px 0;\"><span style=\"display:inline-block;padding:4px 10px;\
         border-radius:999px;background-color:{bg};color:{fg};font-family:{FONT};\
         font-size:12px;font-weight:700;letter-spacing:0.04em;\">{}</span></p>",
        html_escape(&chip.label.to_uppercase())
      ));
    }

    body.push_str(&format!(
      "<h1 class=\"ink\" style=\"margin:0 0 16px 0;font-family:{FONT};font-size:24px;\
       line-height:32px;font-weight:700;color:{ink};\">{}</h1>",
      html_escape(&self.title)
    ));

    for paragraph in &self.lead {
      body.push_str(&paragraph_html(paragraph));
    }

    if !self.facts.is_empty() {
      body.push_str(
        "<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" \
         border=\"0\" style=\"margin:20px 0 4px 0;border-collapse:collapse;\">",
      );
      for fact in &self.facts {
        body.push_str(&format!(
          "<tr><td class=\"muted rule\" width=\"150\" valign=\"top\" style=\"padding:8px 12px 8px 0;\
           border-top:1px solid {rule};font-family:{FONT};font-size:13px;line-height:20px;\
           color:{muted};\">{}</td><td class=\"ink rule\" valign=\"top\" style=\"padding:8px 0;\
           border-top:1px solid {rule};font-family:{FONT};font-size:15px;line-height:20px;\
           color:{ink};word-break:break-word;\">{}</td></tr>",
          html_escape(fact.label),
          html_escape(&fact.value)
        ));
      }
      body.push_str("</table>");
    }

    if let Some(action) = &self.action {
      let url = html_escape(&action.url);
      body.push_str(&format!(
        "<table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" \
         style=\"margin:24px 0 12px 0;\"><tr><td class=\"btn\" align=\"center\" \
         style=\"background-color:{button};border-radius:8px;\"><a href=\"{url}\" \
         style=\"display:inline-block;padding:12px 24px;font-family:{FONT};font-size:15px;\
         line-height:20px;font-weight:600;color:{button_text};text-decoration:none;\
         border-radius:8px;\">{}</a></td></tr></table>",
        html_escape(action.label)
      ));
      body.push_str(&format!(
        "<p class=\"muted\" style=\"margin:0 0 20px 0;font-family:{FONT};font-size:13px;\
         line-height:20px;color:{muted};\">If the button does not work, copy this link into \
         your browser:<br><a class=\"link\" href=\"{url}\" style=\"color:{link};\
         word-break:break-all;\">{url}</a></p>"
      ));
    }

    for paragraph in &self.notes {
      body.push_str(&paragraph_html(paragraph));
    }

    let year = self.clock.year(self.sent_at);
    let footer = format!(
      "Sent by PidgeIoT, a product of {}. You received this email because {}.<br>\
       <a class=\"link\" href=\"{SITE_URL}\" style=\"color:{link};text-decoration:none;\">\
       pidgeiot.com</a> &middot; &copy; {year} {}",
      html_escape(SENDER),
      html_escape(&self.reason),
      html_escape(SENDER)
    );

    format!(
      "<!DOCTYPE html>\
       <html lang=\"en\" xmlns=\"http://www.w3.org/1999/xhtml\" \
       xmlns:o=\"urn:schemas-microsoft-com:office:office\">\
       <head>\
       <meta charset=\"utf-8\">\
       <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
       <meta name=\"color-scheme\" content=\"light dark\">\
       <meta name=\"supported-color-schemes\" content=\"light dark\">\
       <meta name=\"x-apple-disable-message-reformatting\">\
       <title>{title}</title>\
       <!--[if mso]><xml><o:OfficeDocumentSettings><o:PixelsPerInch>96</o:PixelsPerInch>\
       </o:OfficeDocumentSettings></xml><![endif]-->\
       <style>\
       :root{{color-scheme:light dark;supported-color-schemes:light dark;}}\
       body{{margin:0;padding:0;}}\
       {dark_overrides}\
       </style>\
       </head>\
       <body class=\"canvas\" style=\"margin:0;padding:0;background-color:{canvas};\">\
       <div style=\"display:none;max-height:0;overflow:hidden;mso-hide:all;font-size:1px;\
       line-height:1px;color:{canvas};\">{preheader}</div>\
       <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" \
       border=\"0\" class=\"canvas\" style=\"background-color:{canvas};\">\
       <tr><td align=\"center\" style=\"padding:24px 12px;\">\
       <table role=\"presentation\" width=\"600\" cellpadding=\"0\" cellspacing=\"0\" \
       border=\"0\" style=\"width:600px;max-width:100%;\">\
       <tr><td class=\"wordmark\" style=\"padding:0 8px 12px 8px;font-family:{FONT};\
       font-size:20px;line-height:28px;font-weight:700;color:{wordmark};\">PidgeIoT</td></tr>\
       <tr><td class=\"card\" style=\"background-color:{card};border:1px solid {rule};\
       border-radius:8px;padding:32px 32px 24px 32px;\">{body}</td></tr>\
       <tr><td class=\"muted\" style=\"padding:20px 8px 0 8px;font-family:{FONT};\
       font-size:12px;line-height:18px;color:{muted};\">{footer}</td></tr>\
       </table>\
       </td></tr></table>\
       </body></html>",
      title = html_escape(&self.title),
      preheader = html_escape(&self.preheader),
    )
  }
}

/// A body paragraph. A bare `https://` URL inside it becomes a link so
/// the plain "how to change this alert" note stays clickable in HTML.
fn paragraph_html(paragraph: &str) -> String {
  let Palette { ink, link, .. } = LIGHT;
  let mut inner = String::new();
  for (index, word) in paragraph.split(' ').enumerate() {
    if index > 0 {
      inner.push(' ');
    }
    if word.starts_with("https://") {
      let url = html_escape(word);
      inner.push_str(&format!(
        "<a class=\"link\" href=\"{url}\" style=\"color:{link};word-break:break-all;\">{url}</a>"
      ));
    } else {
      inner.push_str(&html_escape(word));
    }
  }
  format!(
    "<p class=\"ink\" style=\"margin:0 0 14px 0;font-family:{FONT};font-size:15px;\
     line-height:24px;color:{ink};\">{inner}</p>"
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{AlertChannel, AlertScope};
  use time::macros::datetime;
  use uuid::Uuid;

  fn invite<'a>(
    inviter_name: Option<&'a str>,
    inviter_email: Option<&'a str>,
    org_name: &'a str,
  ) -> InviteEmail<'a> {
    InviteEmail {
      inviter_name,
      inviter_email,
      org_name,
      role: OrgRole::Admin,
      invite_url: "https://pidgeiot.com/invite?token=Q3VyaW91cyBiaXJk-x_Y",
      expires_at: datetime!(2026-09-02 14:05:00 UTC),
      sent_at: datetime!(2026-08-26 14:05:00 UTC),
    }
  }

  fn definition(name: &str, condition: AlertCondition, severity: AlertSeverity) -> AlertDefinition {
    AlertDefinition {
      id: Uuid::nil(),
      user_id: Uuid::nil(),
      scope: AlertScope::Pigeon("59d0c929f912".to_string()),
      name: name.to_string(),
      condition,
      severity,
      channel: AlertChannel::default(),
      notes: None,
      enabled: true,
      created_at: datetime!(2026-08-01 00:00:00 UTC),
      updated_at: datetime!(2026-08-01 00:00:00 UTC),
    }
  }

  fn threshold(key: &str, value: f64) -> AlertCondition {
    AlertCondition::Threshold {
      key: key.to_string(),
      comparator: Comparator::Gt,
      value,
    }
  }

  fn alert<'a>(
    def: &'a AlertDefinition,
    fired: bool,
    observation: Option<&'a AlertObservation>,
  ) -> AlertEmail<'a> {
    AlertEmail {
      definition: def,
      fired,
      pigeon_id: "59d0c929f912",
      pigeon_name: Some("Greenhouse north"),
      flock_name: Some("Springfield growers"),
      observation,
      at: datetime!(2026-08-26 14:05:09 UTC),
      pigeon_url: "https://pidgeiot.com/flocks/8dc58300-70e6-4484-99f3-18ff7487b6fd/pigeons/59d0c929f912",
      manage_url: "https://pidgeiot.com/flocks/8dc58300-70e6-4484-99f3-18ff7487b6fd/pigeons/59d0c929f912#pigeonAlerts",
    }
  }

  fn every_message() -> Vec<EmailMessage> {
    let high = definition(
      "High temperature",
      threshold("temp", 30.0),
      AlertSeverity::Critical,
    );
    let jump = definition(
      "Pressure jump",
      AlertCondition::RateOfChange {
        key: "pressure".to_string(),
        max_delta: 5.0,
        window_secs: Some(300),
      },
      AlertSeverity::Warning,
    );
    let offline = definition(
      "Gone quiet",
      AlertCondition::DeviceState {
        state: ConnectionStateKind::Offline,
        min_duration_secs: Some(600),
      },
      AlertSeverity::Warning,
    );
    let missing = definition(
      "Heartbeat",
      AlertCondition::MissingReport {
        max_silence_secs: 900,
      },
      AlertSeverity::Warning,
    );
    let noted = {
      let mut noted = definition(
        "Freezer door",
        threshold("temp_c", 4.0),
        AlertSeverity::Critical,
      );
      noted.notes = Some(
        "Check the breaker & the door seal first\nRunbook: https://ops.example.com/freezer".into(),
      );
      noted
    };
    let value = AlertObservation::Value { observed: 34.2 };
    let change = AlertObservation::Change {
      previous: 1013.2,
      observed: 1001.5,
    };
    let silence = AlertObservation::Silence {
      last_seen: Some(datetime!(2026-08-26 13:41:02 UTC)),
    };
    let never = AlertObservation::Silence { last_seen: None };
    vec![
      format_invite_email(
        &invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors"),
        Clock::utc(),
      ),
      format_invite_email(&invite(None, None, "Acme Sensors"), Clock::utc()),
      format_alert_email(&alert(&high, true, Some(&value)), Clock::utc()),
      format_alert_email(&alert(&high, false, Some(&value)), Clock::utc()),
      format_alert_email(&alert(&high, true, None), Clock::utc()),
      format_alert_email(&alert(&jump, true, Some(&change)), Clock::utc()),
      format_alert_email(&alert(&offline, true, Some(&silence)), Clock::utc()),
      format_alert_email(&alert(&offline, false, Some(&silence)), Clock::utc()),
      format_alert_email(&alert(&missing, true, Some(&never)), Clock::utc()),
      format_alert_email(&alert(&noted, true, Some(&value)), Clock::utc()),
    ]
  }

  #[test]
  fn invite_subject_names_the_organization() {
    let message = format_invite_email(
      &invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors"),
      Clock::utc(),
    );
    assert_eq!(
      message.subject,
      "[PidgeIoT] Invitation to join Acme Sensors"
    );
  }

  #[test]
  fn invite_says_who_which_org_what_role_and_when_it_expires() {
    let message = format_invite_email(
      &invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors"),
      Clock::utc(),
    );
    for part in [&message.text, &message.html] {
      assert!(
        part.contains(
          "Ana Ruiz (ana@example.com) has invited you to join the organization Acme Sensors as an admin."
        ),
        "{part}"
      );
      assert!(part.contains("Invited by"));
      assert!(part.contains("Admin"));
      assert!(part.contains("2 Sep 2026, 14:05 UTC, 7 days from now"));
      assert!(part.contains("https://pidgeiot.com/invite?token=Q3VyaW91cyBiaXJk-x_Y"));
      assert!(part.contains("If you weren"));
      assert!(part.contains("ask Ana Ruiz (ana@example.com) to send a new one"));
    }
  }

  #[test]
  fn inviter_is_named_by_whatever_the_identity_carries() {
    let text = format_invite_email(
      &invite(None, Some("ana@example.com"), "Acme Sensors"),
      Clock::utc(),
    )
    .text;
    assert!(text.contains("ana@example.com has invited you"));
    assert!(text.contains("Invited by: ana@example.com\n"));

    let text = format_invite_email(
      &invite(Some("Ana Ruiz"), None, "Acme Sensors"),
      Clock::utc(),
    )
    .text;
    assert!(text.contains("Ana Ruiz has invited you"));
    assert!(text.contains("Invited by: Ana Ruiz\n"));

    let text = format_invite_email(
      &invite(Some("  "), Some("ana@example.com"), "Acme Sensors"),
      Clock::utc(),
    )
    .text;
    assert!(text.contains("Invited by: ana@example.com\n"));
  }

  #[test]
  fn only_a_critical_alert_says_so_in_the_subject() {
    let value = AlertObservation::Value { observed: 34.2 };
    let warning = definition(
      "High temperature",
      threshold("temp", 30.0),
      AlertSeverity::Warning,
    );
    assert_eq!(
      format_alert_email(&alert(&warning, true, Some(&value)), Clock::utc()).subject,
      "[PidgeIoT] Alert firing: temp on Greenhouse north"
    );
    let critical = definition(
      "High temperature",
      threshold("temp", 30.0),
      AlertSeverity::Critical,
    );
    assert_eq!(
      format_alert_email(&alert(&critical, false, Some(&value)), Clock::utc()).subject,
      "[PidgeIoT] Alert resolved: temp on Greenhouse north"
    );
  }

  fn channel(hex: &str, index: usize) -> f64 {
    let byte = u8::from_str_radix(&hex[1 + 2 * index..3 + 2 * index], 16).expect("hex color");
    let c = f64::from(byte) / 255.0;
    if c <= 0.04045 {
      c / 12.92
    } else {
      ((c + 0.055) / 1.055).powf(2.4)
    }
  }

  /// WCAG 2.x relative luminance and contrast ratio.
  fn contrast(a: &str, b: &str) -> f64 {
    let luminance =
      |hex: &str| 0.2126 * channel(hex, 0) + 0.7152 * channel(hex, 1) + 0.0722 * channel(hex, 2);
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
  }

  #[test]
  fn every_color_pair_meets_wcag_aa_in_both_themes() {
    // 4.5:1 for normal text, 3:1 for the large title, the button label and
    // the chip label. The chips are theme-invariant, so they are checked
    // once against their own background.
    for (theme, palette) in [("light", LIGHT), ("dark", DARK)] {
      let pairs = [
        ("body text on card", palette.ink, palette.card, 4.5),
        ("title on card (large)", palette.ink, palette.card, 3.0),
        (
          "muted labels and notes on card",
          palette.muted,
          palette.card,
          4.5,
        ),
        ("footer on canvas", palette.muted, palette.canvas, 4.5),
        ("link on card", palette.link, palette.card, 4.5),
        ("link on canvas", palette.link, palette.canvas, 4.5),
        ("wordmark on canvas", palette.wordmark, palette.canvas, 4.5),
        (
          "button text on button",
          palette.button_text,
          palette.button,
          3.0,
        ),
      ];
      for (what, fg, bg, minimum) in pairs {
        let ratio = contrast(fg, bg);
        println!("{theme:5} {what:32} {fg} on {bg}  {ratio:.2}:1 (needs {minimum})");
        assert!(ratio >= minimum, "{theme} {what}: {ratio:.2} < {minimum}");
      }
    }
    for tone in [Tone::Brand, Tone::Warning, Tone::Critical, Tone::Resolved] {
      let (bg, fg) = tone.colors();
      let ratio = contrast(fg, bg);
      println!("chip  {tone:?} {fg} on {bg}  {ratio:.2}:1 (needs 3)");
      assert!(ratio >= 3.0, "chip {tone:?}: {ratio:.2} < 3");
    }
  }

  #[test]
  fn invite_without_a_known_inviter_still_reads_naturally() {
    let message = format_invite_email(&invite(None, None, "Acme Sensors"), Clock::utc());
    assert!(
      message
        .text
        .contains("A member of Acme Sensors has invited you")
    );
    assert!(message.text.contains("Invited by: not recorded"));
    assert!(
      message
        .text
        .contains("ask the person who invited you to send a new one")
    );
  }

  #[test]
  fn invite_role_gets_the_right_article_and_powers() {
    let mut owner = invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors");
    owner.role = OrgRole::Owner;
    let message = format_invite_email(&owner, Clock::utc());
    assert!(message.text.contains("as an owner."));
    assert!(
      message
        .text
        .contains("As an owner you can manage everything")
    );

    let mut member = invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors");
    member.role = OrgRole::Member;
    let message = format_invite_email(&member, Clock::utc());
    assert!(message.text.contains("as a member."));
    assert!(message.text.contains("As a member you can view"));
  }

  #[test]
  fn alert_subjects_are_scannable() {
    let high = definition(
      "High temperature",
      threshold("temp", 30.0),
      AlertSeverity::Critical,
    );
    let value = AlertObservation::Value { observed: 34.2 };
    assert_eq!(
      format_alert_email(&alert(&high, true, Some(&value)), Clock::utc()).subject,
      "[PidgeIoT] Critical alert firing: temp on Greenhouse north"
    );
    assert_eq!(
      format_alert_email(&alert(&high, false, Some(&value)), Clock::utc()).subject,
      "[PidgeIoT] Alert resolved: temp on Greenhouse north"
    );

    let offline = definition(
      "Gone quiet",
      AlertCondition::DeviceState {
        state: ConnectionStateKind::Offline,
        min_duration_secs: None,
      },
      AlertSeverity::Warning,
    );
    let mut unnamed = alert(&offline, true, None);
    unnamed.pigeon_name = None;
    assert_eq!(
      format_alert_email(&unnamed, Clock::utc()).subject,
      "[PidgeIoT] Alert firing: device offline on 59d0c929f912"
    );
  }

  #[test]
  fn alert_names_pigeon_flock_metric_threshold_observed_time_and_state() {
    let high = definition(
      "High temperature",
      threshold("temp", 30.0),
      AlertSeverity::Critical,
    );
    let value = AlertObservation::Value { observed: 34.2 };
    let message = format_alert_email(&alert(&high, true, Some(&value)), Clock::utc());
    for part in [&message.text, &message.html] {
      assert!(part.contains("Greenhouse north (59d0c929f912)"), "{part}");
      assert!(part.contains("Springfield growers"));
      assert!(part.contains("Metric"));
      assert!(part.contains("temp"));
      assert!(part.contains("greater than 30"));
      assert!(part.contains("34.2"));
      assert!(part.contains("26 Aug 2026, 14:05:09 UTC"));
      assert!(part.contains("Firing"));
      assert!(part.contains("Critical"));
      assert!(part.contains("#pigeonAlerts"));
      assert!(part.contains("To change, pause or delete this alert"));
    }
    assert!(message.text.contains("CRITICAL ALERT FIRING"));
    assert!(message.text.contains("Observed: 34.2"));
    assert!(message.text.contains("When: 26 Aug 2026, 14:05:09 UTC"));

    let message = format_alert_email(&alert(&high, false, Some(&value)), Clock::utc());
    assert!(message.text.contains("RESOLVED"));
    assert!(message.text.contains("State: Resolved"));
    assert!(message.text.contains("no longer meets the condition"));
  }

  #[test]
  fn alert_describes_each_condition_kind_in_words() {
    let jump = definition(
      "Pressure jump",
      AlertCondition::RateOfChange {
        key: "pressure".to_string(),
        max_delta: 5.0,
        window_secs: Some(300),
      },
      AlertSeverity::Warning,
    );
    let change = AlertObservation::Change {
      previous: 1013.2,
      observed: 1001.5,
    };
    let text = format_alert_email(&alert(&jump, true, Some(&change)), Clock::utc()).text;
    assert!(text.contains("Threshold: changes by more than 5 within 5 minutes"));
    assert!(text.contains("Observed: 1013.2 to 1001.5 (-11.7"), "{text}");

    let offline = definition(
      "Gone quiet",
      AlertCondition::DeviceState {
        state: ConnectionStateKind::Offline,
        min_duration_secs: Some(600),
      },
      AlertSeverity::Warning,
    );
    let silence = AlertObservation::Silence {
      last_seen: Some(datetime!(2026-08-26 13:41:02 UTC)),
    };
    let text = format_alert_email(&alert(&offline, true, Some(&silence)), Clock::utc()).text;
    assert!(text.contains("Condition: device offline for at least 10 minutes"));
    assert!(text.contains(
      "Observed: silent for 24 minutes 7 seconds, last report 26 Aug 2026, 13:41:02 UTC"
    ));
    let text = format_alert_email(&alert(&offline, false, Some(&silence)), Clock::utc()).text;
    assert!(text.contains("Observed: reporting again, last report 26 Aug 2026, 13:41:02 UTC"));

    let missing = definition(
      "Heartbeat",
      AlertCondition::MissingReport {
        max_silence_secs: 900,
      },
      AlertSeverity::Warning,
    );
    let never = AlertObservation::Silence { last_seen: None };
    let text = format_alert_email(&alert(&missing, true, Some(&never)), Clock::utc()).text;
    assert!(text.contains("Condition: no report for 15 minutes"));
    assert!(text.contains("Observed: never reported"));

    let high = definition(
      "High temperature",
      threshold("temp", 30.0),
      AlertSeverity::Warning,
    );
    let text = format_alert_email(&alert(&high, true, None), Clock::utc()).text;
    assert!(text.contains("Observed: not captured"));
    assert!(text.contains("ALERT FIRING\n"));
  }

  #[test]
  fn every_dynamic_value_is_html_escaped() {
    let hostile = "<script>alert(\"x\")</script> & Co's";
    let message = format_invite_email(&invite(Some(hostile), Some(hostile), hostile), Clock::utc());
    assert!(!message.html.contains("<script>"));
    assert!(
      message
        .html
        .contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; Co&#39;s")
    );
    assert!(message.text.contains(hostile));

    let def = definition(hostile, threshold(hostile, 1.0), AlertSeverity::Warning);
    let mut email = alert(&def, true, None);
    email.pigeon_name = Some(hostile);
    email.flock_name = Some(hostile);
    email.pigeon_url = "https://pidgeiot.com/flocks/x/pigeons/y?a=1&b=2";
    let message = format_alert_email(&email, Clock::utc());
    assert!(!message.html.contains("<script>"));
    assert!(
      message
        .html
        .contains("href=\"https://pidgeiot.com/flocks/x/pigeons/y?a=1&amp;b=2\"")
    );
    assert!(
      message
        .text
        .contains("https://pidgeiot.com/flocks/x/pigeons/y?a=1&b=2")
    );
  }

  #[test]
  fn operator_notes_are_escaped_and_kept_line_per_line() {
    let mut def = definition("Freezer", threshold("temp_c", 1.0), AlertSeverity::Critical);
    def.notes = Some(
      "Check the breaker & the door seal <script>alert('xss')</script> first\n\
       Runbook: https://ops.example.com/f"
        .into(),
    );
    let message = format_alert_email(&alert(&def, true, None), Clock::utc());

    assert!(!message.html.contains("<script>"));
    assert!(message.html.contains(
      "Check the breaker &amp; the door seal &lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt; first"
    ));
    // The link keeps its own paragraph, and stays clickable.
    assert!(message.html.contains("href=\"https://ops.example.com/f\""));
    assert!(
      message
        .text
        .contains("Check the breaker & the door seal <script>alert('xss')</script> first\n")
    );
    assert!(
      message
        .text
        .contains("Runbook: https://ops.example.com/f\n")
    );
  }

  #[test]
  fn an_alert_without_notes_reads_exactly_as_it_did() {
    let def = definition("Freezer", threshold("temp_c", 1.0), AlertSeverity::Warning);
    let plain = format_alert_email(&alert(&def, true, None), Clock::utc());

    let mut blank = def.clone();
    blank.notes = Some("   \n  ".to_string());
    assert_eq!(
      format_alert_email(&alert(&blank, true, None), Clock::utc()),
      plain
    );
  }

  #[test]
  fn names_are_kept_to_one_line() {
    let message = format_invite_email(
      &invite(
        Some("Ana Ruiz"),
        Some("ana@example.com"),
        "Acme\r\n  Sensors\tGmbH",
      ),
      Clock::utc(),
    );
    assert_eq!(
      message.subject,
      "[PidgeIoT] Invitation to join Acme Sensors GmbH"
    );
    assert!(message.text.contains("Organization: Acme Sensors GmbH\n"));
  }

  #[test]
  fn the_plain_text_part_says_what_the_html_part_says() {
    // The text part never wraps, so every line of it is a unit the HTML
    // part must carry too: a fact splits into its label and value, a URL
    // appears escaped, and everything else appears escaped as a whole.
    for message in every_message() {
      for line in message.text.lines() {
        let line = line.trim_end_matches(':');
        if line.is_empty() || line.starts_with("--") || line.chars().all(|c| c == '=') {
          continue;
        }
        if let Some(rest) = line.strip_prefix("(c) ") {
          assert!(
            message
              .html
              .contains(&format!("&copy; {}", html_escape(rest)))
          );
          continue;
        }
        let units: Vec<&str> = match line.split_once(": ") {
          Some((label, value)) if !line.starts_with("https://") => vec![label, value],
          _ => vec![line],
        };
        for unit in units {
          let unit = unit.trim_end();
          for piece in unit
            .split(' ')
            .filter(|piece| piece.starts_with("https://"))
          {
            assert!(
              message
                .html
                .contains(&format!("href=\"{}\"", html_escape(piece))),
              "text link {piece:?} missing from html"
            );
          }
          if unit.contains("https://") {
            continue;
          }
          assert!(
            message.html.contains(&html_escape(unit)),
            "text line {unit:?} missing from html"
          );
        }
      }
    }
  }

  #[test]
  fn no_em_dashes_anywhere() {
    for message in every_message() {
      for part in [&message.subject, &message.text, &message.html] {
        assert!(!part.contains('\u{2014}'), "{part}");
      }
    }
  }

  #[test]
  fn html_carries_no_images_and_only_https_links() {
    for message in every_message() {
      assert!(!message.html.contains("<img"));
      assert!(!message.html.contains("src="));
      assert!(!message.html.contains("url("));
      assert!(!message.html.contains("background="));
      for href in message.html.split("href=\"").skip(1) {
        assert!(href.starts_with("https://"), "{href}");
      }
    }
  }

  #[test]
  fn html_paints_its_own_backgrounds_and_declares_both_color_schemes() {
    for message in every_message() {
      assert!(message.html.contains(
        "<body class=\"canvas\" style=\"margin:0;padding:0;background-color:#f5f5f4;\">"
      ));
      assert!(
        message
          .html
          .contains("<meta name=\"color-scheme\" content=\"light dark\">")
      );
      assert!(message.html.contains("@media (prefers-color-scheme:dark)"));
      assert!(message.html.contains("width=\"600\""));
      assert!(message.html.contains("role=\"presentation\""));
      // White is only ever a card background, never a text color: text
      // painted white would vanish the moment a client dropped the
      // background it sits on.
      for (index, _) in message.html.match_indices("color:#ffffff") {
        assert!(message.html[..index].ends_with("background-"));
      }
    }
  }

  #[test]
  fn footer_says_who_sent_it_and_why() {
    let message = format_invite_email(
      &invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors"),
      Clock::utc(),
    );
    assert!(
      message
        .text
        .contains("Sent by PidgeIoT, a product of Justin's Engineering Services, LLC.")
    );
    assert!(message.text.contains(
      "You received this email because this address was invited to join Acme Sensors on PidgeIoT."
    ));
    assert!(
      message
        .text
        .contains("(c) 2026 Justin's Engineering Services, LLC")
    );
    assert!(
      message
        .html
        .contains("&copy; 2026 Justin&#39;s Engineering Services, LLC")
    );
  }

  #[test]
  fn durations_read_as_their_two_largest_units() {
    assert_eq!(humanize_secs(0), "0 seconds");
    assert_eq!(humanize_secs(1), "1 second");
    assert_eq!(humanize_secs(59), "59 seconds");
    assert_eq!(humanize_secs(60), "1 minute");
    assert_eq!(humanize_secs(90), "1 minute 30 seconds");
    assert_eq!(humanize_secs(3_600), "1 hour");
    assert_eq!(humanize_secs(5_400), "1 hour 30 minutes");
    assert_eq!(humanize_secs(3_605), "1 hour");
    assert_eq!(humanize_secs(7 * 86_400), "7 days");
    assert_eq!(humanize_secs(90_000), "1 day 1 hour");
  }

  #[test]
  fn numbers_print_in_their_shortest_exact_form() {
    assert_eq!(fmt_num(30.0), "30");
    assert_eq!(fmt_num(29.75), "29.75");
    assert_eq!(fmt_num(-0.5), "-0.5");
  }

  #[test]
  fn escape_covers_the_five_html_metacharacters() {
    assert_eq!(html_escape("a<b>&\"c'"), "a&lt;b&gt;&amp;&quot;c&#39;");
    assert_eq!(html_escape("plain"), "plain");
  }
  // --- The organization's own clock ---

  /// US Eastern's rule, which is all a formatting test needs from a zone:
  /// the real database lives in dovecote (see its `helpers/timezone.rs`).
  struct Eastern;

  impl LocalZone for Eastern {
    fn local_time(&self, at: OffsetDateTime) -> Option<LocalTime> {
      let month = at.to_offset(UtcOffset::UTC).month() as u8;
      let (hours, abbreviation) = if (4..=10).contains(&month) {
        (-4, "EDT")
      } else {
        (-5, "EST")
      };
      Some(LocalTime {
        offset: UtcOffset::from_hms(hours, 0, 0).ok()?,
        abbreviation: abbreviation.to_string(),
      })
    }
  }

  /// An organization that chose UTC itself, as opposed to one with no zone.
  struct Utc;

  impl LocalZone for Utc {
    fn local_time(&self, _at: OffsetDateTime) -> Option<LocalTime> {
      Some(LocalTime {
        offset: UtcOffset::UTC,
        abbreviation: "UTC".to_string(),
      })
    }
  }

  /// A zone the database could not answer for.
  struct Unresolvable;

  impl LocalZone for Unresolvable {
    fn local_time(&self, _at: OffsetDateTime) -> Option<LocalTime> {
      None
    }
  }

  /// A zone with no abbreviation of its own, which the database gives as
  /// an offset rather than a name.
  struct Unnamed;

  impl LocalZone for Unnamed {
    fn local_time(&self, _at: OffsetDateTime) -> Option<LocalTime> {
      Some(LocalTime {
        offset: UtcOffset::from_hms(5, 45, 0).ok()?,
        abbreviation: String::new(),
      })
    }
  }

  fn alert_at(at: OffsetDateTime, clock: Clock<'_>) -> EmailMessage {
    let high = threshold("temp_c", 30.0);
    let definition = definition("Greenhouse too hot", high, AlertSeverity::Warning);
    let observation = AlertObservation::Value { observed: 34.2 };
    let mut email = alert(&definition, true, Some(&observation));
    email.at = at;
    format_alert_email(&email, clock)
  }

  #[test]
  fn a_summer_stamp_reads_local_first_and_utc_beside_it() {
    let zone = Eastern;
    let message = alert_at(datetime!(2026-08-26 19:10:09 UTC), Clock::zoned(&zone));
    for part in [&message.text, &message.html] {
      assert!(
        part.contains("26 Aug 2026, 15:10:09 EDT (19:10:09 UTC)"),
        "missing local stamp in {part}"
      );
    }
    assert!(message.text.contains("When: 26 Aug 2026, 15:10:09 EDT"));
  }

  #[test]
  fn the_same_zone_in_winter_names_its_winter_offset() {
    let zone = Eastern;
    let message = alert_at(datetime!(2026-01-15 19:10:09 UTC), Clock::zoned(&zone));
    assert!(
      message
        .text
        .contains("15 Jan 2026, 14:10:09 EST (19:10:09 UTC)"),
      "winter stamp missing"
    );
  }

  #[test]
  fn a_local_date_the_utc_date_disagrees_with_is_spelled_out() {
    let zone = Eastern;
    let message = alert_at(datetime!(2026-08-27 01:10:09 UTC), Clock::zoned(&zone));
    assert!(
      message
        .text
        .contains("26 Aug 2026, 21:10:09 EDT (27 Aug 2026, 01:10:09 UTC)"),
      "the parentheses must carry the date when the two zones disagree"
    );
  }

  #[test]
  fn an_organization_on_utc_is_not_told_the_time_twice() {
    let zone = Utc;
    let message = alert_at(datetime!(2026-08-26 19:10:09 UTC), Clock::zoned(&zone));
    assert!(message.text.contains("When: 26 Aug 2026, 19:10:09 UTC\n"));
    assert!(!message.text.contains("(19:10:09 UTC)"));
  }

  #[test]
  fn a_zone_that_cannot_answer_falls_back_to_utc() {
    let zone = Unresolvable;
    let zoned = alert_at(datetime!(2026-08-26 19:10:09 UTC), Clock::zoned(&zone));
    let plain = alert_at(datetime!(2026-08-26 19:10:09 UTC), Clock::utc());
    assert_eq!(zoned, plain);
    assert!(plain.text.contains("When: 26 Aug 2026, 19:10:09 UTC"));
  }

  #[test]
  fn a_zone_with_no_name_of_its_own_is_labelled_by_its_offset() {
    let zone = Unnamed;
    let message = alert_at(datetime!(2026-08-26 19:10:09 UTC), Clock::zoned(&zone));
    assert!(
      message
        .text
        .contains("27 Aug 2026, 00:55:09 UTC+05:45 (26 Aug 2026, 19:10:09 UTC)"),
      "offset label missing"
    );
  }

  #[test]
  fn every_time_the_invitation_names_is_local() {
    let zone = Eastern;
    let message = format_invite_email(
      &invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors"),
      Clock::zoned(&zone),
    );
    // The fixture expires a week after it was sent, and both the fact row
    // and the note about the link say when.
    assert_eq!(
      message
        .text
        .matches("2 Sep 2026, 10:05 EDT (14:05 UTC)")
        .count(),
      2,
      "the expiry is stated twice and both must be local"
    );
    assert!(!message.text.contains("2 Sep 2026, 14:05 UTC ("));
  }

  #[test]
  fn a_silence_observation_reports_its_last_report_locally() {
    let zone = Eastern;
    let offline = definition(
      "Gone quiet",
      AlertCondition::DeviceState {
        state: ConnectionStateKind::Offline,
        min_duration_secs: Some(600),
      },
      AlertSeverity::Warning,
    );
    let silence = AlertObservation::Silence {
      last_seen: Some(datetime!(2026-08-26 13:41:02 UTC)),
    };
    let message = format_alert_email(&alert(&offline, true, Some(&silence)), Clock::zoned(&zone));
    assert!(
      message
        .text
        .contains("last report 26 Aug 2026, 09:41:02 EDT (13:41:02 UTC)"),
      "the last report must be stamped like every other time"
    );
  }

  #[test]
  fn the_copyright_year_follows_the_organization_not_utc() {
    let zone = Eastern;
    let mut email = invite(Some("Ana Ruiz"), Some("ana@example.com"), "Acme Sensors");
    let sent_at = datetime!(2026-01-01 02:00:00 UTC);
    email.sent_at = sent_at;
    email.expires_at = sent_at + time::Duration::days(7);
    let message = format_invite_email(&email, Clock::zoned(&zone));
    assert!(
      message.text.contains("(c) 2025 "),
      "31 December locally is still 2025"
    );
  }
}
