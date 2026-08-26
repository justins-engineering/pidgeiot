//! Organizations API client -- see `docs/api.md`'s
//! "Organizations" section for the wire surface and permission matrix.
//!
//! The org list lives in `LocalSession.orgs`, same convention as
//! `api::flocks`: `list` fills it once at sign-in, and every mutation
//! that changes which orgs the caller belongs to (`create`, `rename`,
//! `delete`, `leave`) writes its own response straight back into it. A
//! mutation's response is the only read that is guaranteed to reflect the
//! write -- dovecote's reads go through Hyperdrive, whose query cache
//! keeps answering an identical SELECT with the pre-write rows for up to a
//! minute -- so nothing here refetches the list to confirm a change.
//!
//! The per-org detail (`detail`, `business_details`) is view-local: the
//! org page fetches it on mount and patches its own copy from each
//! mutation's response for the same reason.

use crate::api::{fetch_json, fetch_json_any_status};
use capsules::{
  Flock, FlockTransferRequest, OrgRole, Organization, OrganizationBusinessDetails,
  OrganizationBusinessDetailsRequest, OrganizationCreateRequest, OrganizationDetail,
  OrganizationInviteAcceptRequest, OrganizationInviteCreateRequest, OrganizationInviteCreated,
  OrganizationMember, OrganizationMemberRoleUpdateRequest, OrganizationMembership,
  OrganizationRenameRequest,
};
use dioxus::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

pub(crate) fn to_body<T: serde::Serialize>(value: &T) -> Option<JsValue> {
  let body = serde_json::to_string(value).ok()?;
  serde_wasm_bindgen::to_value(&body).ok()
}

pub(crate) async fn parse<T: serde::de::DeserializeOwned>(
  response: web_sys::Response,
) -> Option<T> {
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value::<T>(json).ok()
}

/// Reads a non-2xx response's plain-text body as the user-facing error
/// (dovecote's error convention: text, not JSON).
pub(crate) async fn error_text(response: &web_sys::Response) -> String {
  match response.text().ok() {
    Some(promise) => JsFuture::from(promise)
      .await
      .ok()
      .and_then(|v| v.as_string())
      .unwrap_or_else(|| format!("Request failed with status {}", response.status())),
    None => format!("Request failed with status {}", response.status()),
  }
}

/// Fills `LocalSession.orgs`. Called once at sign-in (see `App`); the
/// mutations below keep the cache current from then on.
pub async fn list() -> Option<()> {
  let response = fetch_json("GET", "/orgs", None).await?;
  let memberships: Vec<OrganizationMembership> = parse(response).await?;
  let orgs_map: HashMap<Uuid, OrganizationMembership> = memberships
    .into_iter()
    .map(|membership| (membership.organization.id, membership))
    .collect();

  let mut orgs = consume_context::<crate::LocalSession>().orgs;
  *orgs.write() = orgs_map;
  Some(())
}

/// Records a membership the caller just gained in `LocalSession.orgs`.
fn cache_membership(membership: OrganizationMembership) {
  let mut orgs = consume_context::<crate::LocalSession>().orgs;
  orgs.write().insert(membership.organization.id, membership);
}

/// Drops an org the caller no longer belongs to from `LocalSession.orgs`.
fn uncache_org(org_id: Uuid) {
  let mut orgs = consume_context::<crate::LocalSession>().orgs;
  orgs.write().remove(&org_id);
}

/// Creates an org, optionally with the business details it will be
/// invoiced under. The caller is always the founding owner (dovecote
/// never creates an org without one), which is what lets the new
/// membership be cached from the response alone.
///
/// `Err` carries the server's own message rather than collapsing to
/// `None`, unlike most of this module: a creation can now be refused for a
/// reason the person can act on -- a malformed VAT ID, or one VIES
/// definitively rejects -- and "Failed to create organization" would throw
/// away the only sentence that tells them what to fix.
pub async fn create(request: &OrganizationCreateRequest) -> Result<Organization, String> {
  let Some(body) = to_body(request) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status("POST", "/orgs", Some(&body)).await else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  let Some(organization) = parse::<Organization>(response).await else {
    return Err("Failed to parse response".to_string());
  };
  cache_membership(OrganizationMembership {
    organization: organization.clone(),
    role: OrgRole::Owner,
  });
  Ok(organization)
}

pub async fn detail(org_id: Uuid) -> Option<OrganizationDetail> {
  let response = fetch_json("GET", &format!("/orgs/{org_id}"), None).await?;
  parse(response).await
}

pub async fn rename(org_id: Uuid, name: &str) -> Option<Organization> {
  let body = to_body(&OrganizationRenameRequest {
    name: name.to_string(),
  })?;
  let response = fetch_json("PUT", &format!("/orgs/{org_id}"), Some(&body)).await?;
  let organization: Organization = parse(response).await?;
  let mut orgs = consume_context::<crate::LocalSession>().orgs;
  if let Some(membership) = orgs.write().get_mut(&org_id) {
    membership.organization = organization.clone();
  }
  Some(organization)
}

/// The org's tax identity. Member-visible, like the billing overview: a
/// VAT number is public information, and a member who spots a typo in it
/// should be able to say so.
pub async fn business_details(org_id: Uuid) -> Option<OrganizationBusinessDetails> {
  let response = fetch_json("GET", &format!("/orgs/{org_id}/business-details"), None).await?;
  parse(response).await
}

/// Replaces the org's business details wholesale. `Err` carries the
/// server's own message, which is the only place a shape complaint ("that
/// is not the shape of a DE VAT number") or a VIES refusal is phrased --
/// the form shows it verbatim rather than inventing its own wording for a
/// verdict it did not reach.
pub async fn set_business_details(
  org_id: Uuid,
  request: &OrganizationBusinessDetailsRequest,
) -> Result<OrganizationBusinessDetails, String> {
  let Some(body) = to_body(request) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status(
    "PUT",
    &format!("/orgs/{org_id}/business-details"),
    Some(&body),
  )
  .await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    parse(response)
      .await
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
}

/// `Err` carries the server's own message (e.g. the 409 "still owns
/// flocks" refusal) so the UI can show why deletion was blocked.
pub async fn delete(org_id: Uuid) -> Result<(), String> {
  let Some(response) = fetch_json_any_status("DELETE", &format!("/orgs/{org_id}"), None).await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    uncache_org(org_id);
    Ok(())
  } else {
    Err(error_text(&response).await)
  }
}

/// `Err` carries the server message (last-owner protection, etc).
pub async fn change_role(
  org_id: Uuid,
  user_id: Uuid,
  role: OrgRole,
) -> Result<OrganizationMember, String> {
  let Some(body) = to_body(&OrganizationMemberRoleUpdateRequest { role }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status(
    "PUT",
    &format!("/orgs/{org_id}/members/{user_id}"),
    Some(&body),
  )
  .await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    parse(response)
      .await
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
}

/// `Err` carries the server message (last-owner protection, etc).
pub async fn remove_member(org_id: Uuid, user_id: Uuid) -> Result<(), String> {
  let Some(response) =
    fetch_json_any_status("DELETE", &format!("/orgs/{org_id}/members/{user_id}"), None).await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    Ok(())
  } else {
    Err(error_text(&response).await)
  }
}

/// The caller removing themselves -- the same route as `remove_member`,
/// but the org also leaves `LocalSession.orgs`, since the caller no
/// longer belongs to it.
pub async fn leave(org_id: Uuid, user_id: Uuid) -> Result<(), String> {
  remove_member(org_id, user_id).await?;
  uncache_org(org_id);
  Ok(())
}

/// `Err` carries the server message. The one worth reading is the seat
/// cap's `403`, which names the tier, the seats it includes and the count
/// already spent -- a generic failure would leave the inviter with no way
/// to tell a full organization from a broken request.
pub async fn create_invite(
  org_id: Uuid,
  email: &str,
  role: OrgRole,
) -> Result<OrganizationInviteCreated, String> {
  let Some(body) = to_body(&OrganizationInviteCreateRequest {
    email: email.to_string(),
    role,
  }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) =
    fetch_json_any_status("POST", &format!("/orgs/{org_id}/invites"), Some(&body)).await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    parse(response)
      .await
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
}

pub async fn revoke_invite(org_id: Uuid, invite_id: Uuid) -> Option<()> {
  fetch_json(
    "DELETE",
    &format!("/orgs/{org_id}/invites/{invite_id}"),
    None,
  )
  .await?;
  Some(())
}

/// Token-alone acceptance (`POST /invites/accept`) -- `Err` carries the
/// server's own message (invalid/expired/used token, already a member) for
/// the invite view to render verbatim. The response is the caller's new
/// membership in the shape `GET /orgs` lists it, which is what lets it go
/// straight into `LocalSession.orgs`.
pub async fn accept_invite(token: &str) -> Result<OrganizationMembership, String> {
  let Some(body) = to_body(&OrganizationInviteAcceptRequest {
    token: token.to_string(),
  }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status("POST", "/invites/accept", Some(&body)).await else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  let Some(membership) = parse::<OrganizationMembership>(response).await else {
    return Err("Failed to parse response".to_string());
  };
  cache_membership(membership.clone());
  Ok(membership)
}

/// Transfers a personal flock into an org (see `docs/api.md`). On success
/// the updated `Flock` (now carrying `org_id`) is written back into
/// `LocalSession.flocks`, same cache convention as `api::flocks`.
pub async fn transfer_flock(flock_id: Uuid, org_id: Uuid) -> Result<Flock, String> {
  let Some(body) = to_body(&FlockTransferRequest { org_id }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) =
    fetch_json_any_status("POST", &format!("/flocks/{flock_id}/transfer"), Some(&body)).await
  else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  let Some(flock) = parse::<Flock>(response).await else {
    return Err("Failed to parse response".to_string());
  };
  let mut flock_list = consume_context::<crate::LocalSession>().flocks;
  flock_list.write().insert(flock.id, flock.clone());
  Ok(flock)
}
