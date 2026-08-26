//! The zone list behind the organization's timezone control.
//!
//! The names come from the browser (`Intl.supportedValuesOf("timeZone")`),
//! never from a table compiled into this bundle: every browser that can
//! run this dashboard already carries the timezone database, and a second
//! copy would be downloaded by every visitor to fill one select.
//!
//! Reading them is a browser call, so it happens after hydration; these
//! two functions are the part that can be reasoned about without one.

use capsules::DEFAULT_TIMEZONE;

/// The options to offer, given what the browser knows and what the
/// organization currently has stored.
///
/// The stored zone is always in the list even when this browser has never
/// heard of it: an option that is missing cannot be shown as selected, so
/// leaving it out would render the control as though the organization were
/// set to something else. Duplicates are dropped and the order is the
/// alphabetical one people scan a zone list in.
pub fn zone_options(supported: &[String], stored: &str) -> Vec<String> {
  let mut names: Vec<String> = supported
    .iter()
    .map(|name| name.trim())
    .filter(|name| !name.is_empty())
    .map(str::to_string)
    .collect();
  let stored = stored.trim();
  if !stored.is_empty() && !names.iter().any(|name| name == stored) {
    names.push(stored.to_string());
  }
  names.sort_unstable();
  names.dedup();
  names
}

/// The zone worth offering as a one-click suggestion: this browser's own,
/// while the organization is still on the default.
///
/// Once somebody has chosen a zone deliberately, a suggestion would be
/// second-guessing them from whichever machine happens to be open, so
/// there is none.
pub fn suggested_zone(stored: &str, browser: Option<&str>) -> Option<String> {
  if stored.trim() != DEFAULT_TIMEZONE {
    return None;
  }
  let browser = browser?.trim();
  if browser.is_empty() || browser == DEFAULT_TIMEZONE {
    return None;
  }
  Some(browser.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
  }

  #[test]
  fn the_browsers_list_comes_back_sorted_and_deduplicated() {
    let supported = names(&["Europe/Dublin", "America/New_York", "Europe/Dublin", "UTC"]);
    assert_eq!(
      zone_options(&supported, "UTC"),
      names(&["America/New_York", "Europe/Dublin", "UTC"])
    );
  }

  #[test]
  fn a_stored_zone_the_browser_does_not_list_is_added_anyway() {
    let supported = names(&["Europe/Dublin", "UTC"]);
    assert_eq!(
      zone_options(&supported, "US/Eastern"),
      names(&["Europe/Dublin", "US/Eastern", "UTC"])
    );
  }

  #[test]
  fn an_empty_browser_list_still_offers_what_is_stored() {
    assert_eq!(
      zone_options(&[], "America/New_York"),
      names(&["America/New_York"])
    );
    assert!(zone_options(&[], "  ").is_empty());
  }

  #[test]
  fn blank_entries_never_reach_the_select() {
    let supported = names(&["", "  ", "UTC"]);
    assert_eq!(zone_options(&supported, "UTC"), names(&["UTC"]));
  }

  #[test]
  fn the_browsers_own_zone_is_suggested_while_the_org_is_on_the_default() {
    assert_eq!(
      suggested_zone("UTC", Some("America/New_York")).as_deref(),
      Some("America/New_York")
    );
  }

  #[test]
  fn nothing_is_suggested_once_a_zone_has_been_chosen() {
    assert_eq!(
      suggested_zone("Europe/Dublin", Some("America/New_York")),
      None
    );
  }

  #[test]
  fn nothing_is_suggested_when_the_browser_agrees_with_the_default() {
    assert_eq!(suggested_zone("UTC", Some("UTC")), None);
    assert_eq!(suggested_zone("UTC", Some("   ")), None);
    assert_eq!(suggested_zone("UTC", None), None);
  }
}
