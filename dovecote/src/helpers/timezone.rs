//! The organization timezone: checking a name against a real timezone
//! database before it is stored.
//!
//! The database lives in this crate and only in this crate. `capsules`
//! formats the emails but stays free of one, and `fancier` never compiles
//! one at all: a browser already ships a copy, so a second one in the wasm
//! bundle would be paid for on every page load.

use capsules::{Clock, LocalTime, LocalZone};
use time::OffsetDateTime;
use time_tz::{Offset, TimeZone, timezones};

/// The name to store for a zone somebody asked for, or `None` when the
/// database has never heard of it.
///
/// A name the database knows is kept exactly as it was asked for, because
/// that spelling is also the one the dashboard's zone list offers and
/// stored text that differs from it would fail to match the stored value
/// back to an option. A name that differs only in case is repaired
/// (`america/new_york` is stored as `America/New_York`) rather than
/// refused for a shift key -- the database itself matches exactly, so an
/// unrepaired spelling would resolve to nothing at send time.
pub fn canonical_timezone(name: &str) -> Option<String> {
  let name = name.trim();
  if name.is_empty() {
    return None;
  }
  if timezones::get_by_name(name).is_some() {
    return Some(name.to_string());
  }
  // Only reached for a name the exact lookup missed, so the scan costs
  // nothing on the common path -- and it runs on a write, never a read.
  timezones::iter()
    .find(|tz| tz.name().eq_ignore_ascii_case(name))
    .map(|tz| tz.name().to_string())
}

/// One organization's zone, looked up once so every timestamp in a
/// message costs a lookup in it rather than a search for it.
pub struct OrgTimeZone(&'static time_tz::Tz);

/// The zone a message about this organization is written in, or `None`
/// when the stored name resolves to nothing. A stored name only fails to
/// resolve if the database dropped a zone between the write and the send,
/// which is rare and must not cost anyone their alert: the caller falls
/// back to UTC, which is what the message said before zones existed.
pub fn org_timezone(name: &str) -> Option<OrgTimeZone> {
  timezones::get_by_name(name.trim()).map(OrgTimeZone)
}

/// Pairs with `org_timezone`: the zone has to be held by the caller,
/// because the clock only borrows it.
pub fn clock_for(zone: Option<&OrgTimeZone>) -> Clock<'_> {
  match zone {
    Some(zone) => Clock::zoned(zone),
    None => Clock::utc(),
  }
}

impl LocalZone for OrgTimeZone {
  fn local_time(&self, at: OffsetDateTime) -> Option<LocalTime> {
    let offset = self.0.get_offset_utc(&at);
    Some(LocalTime {
      offset: offset.to_utc(),
      abbreviation: offset.name().to_string(),
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::macros::datetime;

  fn stamp(zone_name: &str, at: time::OffsetDateTime) -> String {
    let zone = org_timezone(zone_name);
    let local = zone
      .as_ref()
      .and_then(|zone| zone.local_time(at))
      .expect("zone resolves for this instant");
    let there = at.to_offset(local.offset);
    format!(
      "{:02}:{:02} {}",
      there.hour(),
      there.minute(),
      local.abbreviation
    )
  }

  #[test]
  fn a_real_zone_comes_back_as_itself() {
    assert_eq!(
      canonical_timezone("America/New_York").as_deref(),
      Some("America/New_York")
    );
    assert_eq!(canonical_timezone("UTC").as_deref(), Some("UTC"));
    assert_eq!(
      canonical_timezone("Australia/Sydney").as_deref(),
      Some("Australia/Sydney")
    );
  }

  #[test]
  fn a_zone_that_does_not_exist_is_refused() {
    assert_eq!(canonical_timezone("America/Atlantis"), None);
    assert_eq!(canonical_timezone("EDT"), None);
    assert_eq!(canonical_timezone("UTC+2"), None);
    assert_eq!(canonical_timezone(""), None);
    assert_eq!(canonical_timezone("   "), None);
  }

  #[test]
  fn case_and_surrounding_space_do_not_decide_the_answer() {
    assert_eq!(
      canonical_timezone("america/new_york").as_deref(),
      Some("America/New_York")
    );
    assert_eq!(
      canonical_timezone("  Europe/Dublin  ").as_deref(),
      Some("Europe/Dublin")
    );
    assert_eq!(canonical_timezone("utc").as_deref(), Some("UTC"));
  }

  #[test]
  fn an_alias_the_database_knows_is_accepted_as_written() {
    assert_eq!(
      canonical_timezone("US/Eastern").as_deref(),
      Some("US/Eastern")
    );
  }

  #[test]
  fn a_zone_that_keeps_summer_time_moves_with_it() {
    let summer = datetime!(2026-08-26 19:10:00 UTC);
    let winter = datetime!(2026-01-15 19:10:00 UTC);
    assert_eq!(stamp("America/New_York", summer), "15:10 EDT");
    assert_eq!(stamp("America/New_York", winter), "14:10 EST");
  }

  #[test]
  fn a_southern_zone_keeps_its_own_summer() {
    let january = datetime!(2026-01-15 02:10:00 UTC);
    let july = datetime!(2026-07-15 02:10:00 UTC);
    assert_eq!(stamp("Australia/Sydney", january), "13:10 AEDT");
    assert_eq!(stamp("Australia/Sydney", july), "12:10 AEST");
  }

  #[test]
  fn a_zone_that_never_changes_reads_the_same_all_year() {
    let summer = datetime!(2026-08-26 19:10:00 UTC);
    let winter = datetime!(2026-01-15 19:10:00 UTC);
    assert_eq!(stamp("Asia/Tokyo", summer), "04:10 JST");
    assert_eq!(stamp("Asia/Tokyo", winter), "04:10 JST");
  }

  #[test]
  fn a_message_with_no_resolvable_zone_is_written_in_utc() {
    assert!(org_timezone("America/Atlantis").is_none());
    let clock = clock_for(None);
    // The public surface of a UTC clock is what the emails render; the
    // formatting itself is capsules' own test.
    assert_eq!(format!("{clock:?}"), "Clock(UTC)");
  }

  /// Both halves at once: a zone this database resolved, spelled into a
  /// real message. Each half is checked on its own above, but an offset
  /// applied the wrong way round would satisfy both and still send an
  /// invitation naming the wrong hour.
  #[test]
  fn a_resolved_zone_spells_a_real_message_in_local_time() {
    let zone = org_timezone("America/New_York").expect("known zone");
    let sent_at = datetime!(2026-08-26 14:05:00 UTC);
    let message = capsules::format_invite_email(
      &capsules::InviteEmail {
        inviter_name: Some("Ana Ruiz"),
        inviter_email: Some("ana@example.com"),
        org_name: "Acme Sensors",
        role: capsules::OrgRole::Admin,
        invite_url: "https://pidgeiot.com/invite?token=sample",
        expires_at: sent_at + time::Duration::days(7),
        sent_at,
      },
      clock_for(Some(&zone)),
    );
    assert!(
      message
        .text
        .contains("2 Sep 2026, 10:05 EDT (14:05 UTC), 7 days from now"),
      "the invitation must name the organization's own hour"
    );
  }

  #[test]
  fn a_resolvable_zone_produces_a_zoned_clock() {
    let zone = org_timezone("Europe/Dublin").expect("known zone");
    let clock = clock_for(Some(&zone));
    assert_eq!(format!("{clock:?}"), "Clock(zoned)");
  }
}
