//! Empty-state wording for the pigeon detail page's move control.
//!
//! Pure and synchronous so the case selection can be tested off a wasm
//! target, same rationale as `firmware_repush`.

use uuid::Uuid;

/// Said before the flocks cache has answered, so the control claims nothing
/// about an ownership rule it cannot yet know applies.
const NOT_LOADED: &str = "Your flock list has not loaded yet, so there are no destinations \
  to show.";

const PERSONAL: &str = "You have no other personal flock to move this pigeon to. A pigeon only \
  ever moves between flocks with the same owner.";

const ORG_LEAD: &str = "You have no other flock in ";
const ORG_MID: &str = " to move this pigeon to, and a pigeon only ever moves between flocks \
  with the same owner. \"Transfer to org\" on a personal flock's page moves that flock into ";
const ORG_TAIL: &str = ", which gives this pigeon a destination.";

/// Stands in when the org list cached at sign-in has no name for this org.
const UNNAMED_ORG: &str = "this organization";

/// Why the move control has no destination to offer.
///
/// `flock_org` is the current flock's owner as the flocks cache answers it:
/// the outer `Option` is whether the cache has an answer at all, the inner
/// one is the flock's `org_id`. `org_name` is that org's name when the org
/// list cached at sign-in carries it.
pub fn no_destination_message(flock_org: Option<Option<Uuid>>, org_name: Option<&str>) -> String {
  match flock_org {
    None => NOT_LOADED.to_string(),
    Some(None) => PERSONAL.to_string(),
    Some(Some(_)) => {
      let org = org_name
        .filter(|name| !name.is_empty())
        .unwrap_or(UNNAMED_ORG);
      let mut message =
        String::with_capacity(ORG_LEAD.len() + ORG_MID.len() + ORG_TAIL.len() + org.len() * 2);
      message.push_str(ORG_LEAD);
      message.push_str(org);
      message.push_str(ORG_MID);
      message.push_str(org);
      message.push_str(ORG_TAIL);
      message
    }
  }
}

#[cfg(test)]
mod tests {
  use super::no_destination_message;
  use uuid::Uuid;

  #[test]
  fn an_unloaded_cache_claims_no_ownership_rule() {
    let message = no_destination_message(None, Some("Acme Robotics"));
    assert!(message.contains("has not loaded"));
    assert!(
      !message.contains("owner"),
      "an unknown owner cannot be explained"
    );
  }

  #[test]
  fn a_personal_flock_names_the_personal_rule() {
    let message = no_destination_message(Some(None), Some("Acme Robotics"));
    assert!(message.contains("personal flock"));
    assert!(
      !message.contains("Acme Robotics"),
      "a personal flock has no org"
    );
    assert!(
      !message.contains("Transfer to org"),
      "the flock is already personal"
    );
  }

  #[test]
  fn an_org_flock_names_the_org_and_the_way_to_add_a_destination() {
    let message = no_destination_message(Some(Some(Uuid::nil())), Some("Acme Robotics"));
    assert!(message.contains("no other flock in Acme Robotics"));
    assert!(message.contains("into Acme Robotics"));
    assert!(message.contains("\"Transfer to org\" on a personal flock's page"));
  }

  #[test]
  fn an_org_without_a_cached_name_stays_readable() {
    for name in [None, Some("")] {
      let message = no_destination_message(Some(Some(Uuid::nil())), name);
      assert!(message.contains("no other flock in this organization"));
      assert!(message.contains("into this organization"));
    }
  }
}
