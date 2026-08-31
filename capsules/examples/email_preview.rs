//! Renders the transactional email templates with sample data so they can
//! be opened in a browser or screenshotted before a change ships:
//!
//! ```sh
//! cargo run -p capsules --example email_preview -- /some/output/dir
//! ```
//!
//! Writes one `.html` and one `.txt` per message into that directory.

use capsules::{
  AlertChannel, AlertCondition, AlertDefinition, AlertEmail, AlertObservation, AlertScope,
  AlertSeverity, Clock, Comparator, ConnectionStateKind, EmailMessage, InviteEmail, LocalTime,
  LocalZone, OrgRole, format_alert_email, format_invite_email,
};
use std::path::Path;
use time::macros::datetime;
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

/// A stand-in for an organization's zone, so the preview shows what a
/// stamp looks like away from UTC. dovecote resolves the real thing
/// against a timezone database; this crate carries none by design.
struct SampleZone;

impl LocalZone for SampleZone {
  fn local_time(&self, _at: OffsetDateTime) -> Option<LocalTime> {
    Some(LocalTime {
      offset: UtcOffset::from_hms(-4, 0, 0).ok()?,
      abbreviation: "EDT".to_string(),
    })
  }
}

fn main() {
  let out = std::env::args()
    .nth(1)
    .expect("usage: email_preview <output dir>");
  let out = Path::new(&out);
  std::fs::create_dir_all(out).expect("output dir");

  let sent_at = datetime!(2026-08-26 14:05:09 UTC);
  let zone = SampleZone;
  let clock = Clock::zoned(&zone);

  let invite = InviteEmail {
    inviter_name: Some("Ana Ruiz"),
    inviter_email: Some("ana.ruiz@acmesensors.example"),
    org_name: "Acme Sensors",
    role: OrgRole::Admin,
    invite_url: "https://pidgeiot.com/invite?token=Q3VyaW91cyBiaXJkcyBmbHkgZmFyLCB0aGUgZW5kLQ",
    expires_at: sent_at + time::Duration::days(7),
    sent_at,
  };
  write(out, "invite", &format_invite_email(&invite, clock));

  let flock_id = "8dc58300-70e6-4484-99f3-18ff7487b6fd";
  let pigeon_id = "59d0c929f9124b0e";
  let pigeon_url = format!("https://pidgeiot.com/flocks/{flock_id}/pigeons/{pigeon_id}");
  let manage_url = format!("{pigeon_url}#pigeonAlerts");

  let high_temp = definition(
    "Greenhouse over temperature",
    AlertCondition::Threshold {
      key: "temp_c".to_string(),
      comparator: Comparator::Gt,
      value: 30.0,
    },
    AlertSeverity::Critical,
  );
  let observed = AlertObservation::Value { observed: 34.2 };
  let firing = AlertEmail {
    definition: &high_temp,
    fired: true,
    pigeon_id,
    pigeon_name: Some("Greenhouse north"),
    flock_name: Some("Springfield growers"),
    observation: Some(&observed),
    at: sent_at,
    pigeon_url: &pigeon_url,
    manage_url: &manage_url,
  };
  write(out, "alert_firing", &format_alert_email(&firing, clock));

  let recovered = AlertObservation::Value { observed: 27.9 };
  let resolved = AlertEmail {
    fired: false,
    observation: Some(&recovered),
    at: sent_at + time::Duration::minutes(42),
    ..firing.clone()
  };
  write(out, "alert_resolved", &format_alert_email(&resolved, clock));

  let mut noted = high_temp.clone();
  noted.notes = Some(
    "Vents & shades are on the same breaker: check that first.\n\
     Runbook: https://pidgeiot.com/documentation"
      .to_string(),
  );
  let with_notes = AlertEmail {
    definition: &noted,
    ..firing.clone()
  };
  write(out, "alert_notes", &format_alert_email(&with_notes, clock));

  let gone_quiet = definition(
    "Pump controller silent",
    AlertCondition::DeviceState {
      state: ConnectionStateKind::Offline,
      min_duration_secs: Some(600),
    },
    AlertSeverity::Warning,
  );
  let silence = AlertObservation::Silence {
    last_seen: Some(sent_at - time::Duration::minutes(24) - time::Duration::seconds(7)),
  };
  let offline = AlertEmail {
    definition: &gone_quiet,
    fired: true,
    pigeon_id: "b41e0c7d2a9f8813",
    pigeon_name: Some("Pump house"),
    flock_name: Some("Springfield growers"),
    observation: Some(&silence),
    at: sent_at,
    pigeon_url: "https://pidgeiot.com/flocks/8dc58300-70e6-4484-99f3-18ff7487b6fd/pigeons/b41e0c7d2a9f8813",
    manage_url: "https://pidgeiot.com/flocks/8dc58300-70e6-4484-99f3-18ff7487b6fd/pigeons#flockAlerts",
  };
  write(out, "alert_offline", &format_alert_email(&offline, clock));
}

fn definition(name: &str, condition: AlertCondition, severity: AlertSeverity) -> AlertDefinition {
  AlertDefinition {
    id: Uuid::nil(),
    user_id: Uuid::nil(),
    scope: AlertScope::Pigeon("59d0c929f9124b0e".to_string()),
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

fn write(out: &Path, stem: &str, message: &EmailMessage) {
  std::fs::write(out.join(format!("{stem}.html")), &message.html).expect("write html");
  std::fs::write(
    out.join(format!("{stem}.txt")),
    format!("Subject: {}\n\n{}", message.subject, message.text),
  )
  .expect("write text");
  println!("{stem}: {}", message.subject);
}
