//! The organization timezone: checking a name against a real timezone
//! database before it is stored.
//!
//! The database lives in this crate and only in this crate. `capsules`
//! formats the emails but stays free of one, and `fancier` never compiles
//! one at all: a browser already ships a copy, so a second one in the wasm
//! bundle would be paid for on every page load.

use time_tz::{TimeZone, timezones};

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

#[cfg(test)]
mod tests {
  use super::*;

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
}
