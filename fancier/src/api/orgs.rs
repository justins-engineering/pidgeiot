//! Organizations API client -- see `docs/api.md`'s
//! "Organizations" section for the wire surface and permission matrix.
//!
//! Unlike flocks/pigeons, orgs are NOT cached in `LocalSession` -- the org
//! views are self-contained management pages that own their data via
//! `use_resource`, so plain returns keep the shared cache honest. The one
//! exception is `transfer_flock`, which writes the updated `Flock` back
//! into `LocalSession.flocks` (that cache IS the flock views' source of
//! truth).

use crate::api::{fetch_json, fetch_json_any_status};
use capsules::{
  Flock, FlockTransferRequest, OrgRole, Organization, OrganizationBusinessDetails,
  OrganizationBusinessDetailsRequest, OrganizationCreateRequest, OrganizationDetail,
  OrganizationInviteAcceptRequest, OrganizationInviteCreateRequest, OrganizationInviteCreated,
  OrganizationMember, OrganizationMemberRoleUpdateRequest, OrganizationMembership,
  OrganizationRenameRequest,
};
use dioxus::prelude::*;
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

pub async fn list() -> Option<Vec<OrganizationMembership>> {
  let response = fetch_json("GET", "/orgs", None).await?;
  parse(response).await
}

/// Creates an org, optionally with the business details it will be
/// invoiced under.
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
  if response.ok() {
    parse(response)
      .await
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
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
  parse(response).await
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

pub async fn create_invite(
  org_id: Uuid,
  email: &str,
  role: OrgRole,
) -> Option<OrganizationInviteCreated> {
  let body = to_body(&OrganizationInviteCreateRequest {
    email: email.to_string(),
    role,
  })?;
  let response = fetch_json("POST", &format!("/orgs/{org_id}/invites"), Some(&body)).await?;
  parse(response).await
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
/// the invite view to render verbatim.
pub async fn accept_invite(token: &str) -> Result<OrganizationMember, String> {
  let Some(body) = to_body(&OrganizationInviteAcceptRequest {
    token: token.to_string(),
  }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status("POST", "/invites/accept", Some(&body)).await else {
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
