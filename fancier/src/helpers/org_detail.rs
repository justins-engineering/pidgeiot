//! What the org page does to its own copy of `OrganizationDetail` after
//! each save. The page fetches the detail once on mount and then applies
//! every mutation's response here instead of refetching: dovecote's reads
//! go through Hyperdrive, whose query cache keeps answering an identical
//! SELECT with the pre-write rows for up to a minute, so a refetch right
//! after a save would show the state from before it. The response body is
//! the only read guaranteed to reflect the write.
//!
//! Each function keeps the list order `GET /orgs/:id` uses (members and
//! invites both oldest-first), so a patched page looks the same as a
//! freshly loaded one.

use capsules::{Organization, OrganizationDetail, OrganizationInvite, OrganizationMember};
use uuid::Uuid;

/// `PUT /orgs/:id` answered with the renamed org.
pub fn rename(detail: &mut OrganizationDetail, organization: Organization) {
  detail.organization = organization;
}

/// `PUT /orgs/:id/members/:user_id` answered with the member's new row.
/// When the row is the caller's own, `caller_role` follows it, since that
/// is what gates the manager-only sections of the page.
pub fn set_member_role(
  detail: &mut OrganizationDetail,
  member: OrganizationMember,
  caller_user_id: Option<Uuid>,
) {
  if caller_user_id == Some(member.user_id) {
    detail.caller_role = member.role;
  }
  if let Some(existing) = detail
    .members
    .iter_mut()
    .find(|m| m.user_id == member.user_id)
  {
    *existing = member;
  }
}

/// `DELETE /orgs/:id/members/:user_id` succeeded.
pub fn remove_member(detail: &mut OrganizationDetail, user_id: Uuid) {
  detail.members.retain(|m| m.user_id != user_id);
}

/// `POST /orgs/:id/invites` answered with the pending invite. A repeat of
/// an id already listed replaces it rather than listing it twice.
pub fn add_invite(detail: &mut OrganizationDetail, invite: OrganizationInvite) {
  if let Some(existing) = detail.invites.iter_mut().find(|i| i.id == invite.id) {
    *existing = invite;
  } else {
    detail.invites.push(invite);
  }
}

/// `DELETE /orgs/:id/invites/:invite_id` succeeded.
pub fn remove_invite(detail: &mut OrganizationDetail, invite_id: Uuid) {
  detail.invites.retain(|i| i.id != invite_id);
}

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::OrgRole;
  use time::OffsetDateTime;

  fn org(name: &str) -> Organization {
    Organization {
      id: Uuid::from_u128(1),
      name: name.to_string(),
      timezone: capsules::DEFAULT_TIMEZONE.to_string(),
      created_at: OffsetDateTime::UNIX_EPOCH,
      updated_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  fn member(user: u128, role: OrgRole) -> OrganizationMember {
    OrganizationMember {
      org_id: Uuid::from_u128(1),
      user_id: Uuid::from_u128(user),
      role,
      email: Some(format!("user{user}@example.com")),
      invited_by: None,
      created_at: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(user as i64),
    }
  }

  fn invite(id: u128, email: &str) -> OrganizationInvite {
    OrganizationInvite {
      id: Uuid::from_u128(id),
      org_id: Uuid::from_u128(1),
      email: email.to_string(),
      role: OrgRole::Member,
      expires_at: OffsetDateTime::UNIX_EPOCH,
      created_by: Uuid::from_u128(10),
      created_at: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(id as i64),
    }
  }

  fn detail() -> OrganizationDetail {
    OrganizationDetail {
      organization: org("Before"),
      caller_role: OrgRole::Owner,
      members: vec![
        member(10, OrgRole::Owner),
        member(11, OrgRole::Admin),
        member(12, OrgRole::Member),
      ],
      invites: vec![invite(20, "a@example.com"), invite(21, "b@example.com")],
    }
  }

  #[test]
  fn rename_replaces_the_org_row_only() {
    let mut d = detail();
    let mut renamed = org("After");
    renamed.updated_at = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1);
    rename(&mut d, renamed.clone());
    assert_eq!(d.organization, renamed);
    assert_eq!(d.members.len(), 3);
    assert_eq!(d.invites.len(), 2);
  }

  #[test]
  fn role_change_replaces_the_row_in_place() {
    let mut d = detail();
    set_member_role(
      &mut d,
      member(12, OrgRole::Admin),
      Some(Uuid::from_u128(10)),
    );
    let ids: Vec<u128> = d.members.iter().map(|m| m.user_id.as_u128()).collect();
    assert_eq!(ids, vec![10, 11, 12], "order must survive the patch");
    assert_eq!(d.members[2].role, OrgRole::Admin);
    assert_eq!(d.caller_role, OrgRole::Owner, "somebody else's row");
  }

  #[test]
  fn own_role_change_moves_caller_role() {
    let mut d = detail();
    set_member_role(
      &mut d,
      member(10, OrgRole::Member),
      Some(Uuid::from_u128(10)),
    );
    assert_eq!(d.caller_role, OrgRole::Member);
    assert_eq!(d.members[0].role, OrgRole::Member);
  }

  #[test]
  fn role_change_for_an_unknown_member_changes_nothing() {
    let mut d = detail();
    let before = d.clone();
    set_member_role(&mut d, member(99, OrgRole::Admin), None);
    assert_eq!(d, before);
  }

  #[test]
  fn removing_a_member_drops_exactly_that_row() {
    let mut d = detail();
    remove_member(&mut d, Uuid::from_u128(11));
    let ids: Vec<u128> = d.members.iter().map(|m| m.user_id.as_u128()).collect();
    assert_eq!(ids, vec![10, 12]);
  }

  #[test]
  fn a_new_invite_lands_last() {
    let mut d = detail();
    add_invite(&mut d, invite(22, "c@example.com"));
    let ids: Vec<u128> = d.invites.iter().map(|i| i.id.as_u128()).collect();
    assert_eq!(ids, vec![20, 21, 22]);
  }

  #[test]
  fn a_repeated_invite_id_replaces_instead_of_duplicating() {
    let mut d = detail();
    add_invite(&mut d, invite(21, "renamed@example.com"));
    assert_eq!(d.invites.len(), 2);
    assert_eq!(d.invites[1].email, "renamed@example.com");
  }

  #[test]
  fn revoking_an_invite_drops_exactly_that_row() {
    let mut d = detail();
    remove_invite(&mut d, Uuid::from_u128(20));
    let ids: Vec<u128> = d.invites.iter().map(|i| i.id.as_u128()).collect();
    assert_eq!(ids, vec![21]);
  }

  #[test]
  fn revoking_an_unknown_invite_changes_nothing() {
    let mut d = detail();
    remove_invite(&mut d, Uuid::from_u128(99));
    assert_eq!(d.invites.len(), 2);
  }
}
