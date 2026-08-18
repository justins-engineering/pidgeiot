//! Billing API client -- see `docs/api.md`'s "Billing" section for the
//! wire surface and the permission split (overview is member-visible;
//! checkout/portal are manager-only).
//!
//! Like orgs, nothing here is cached in `LocalSession`: the billing panel
//! owns its data via `use_resource`, and the two session mints return a
//! Stripe-hosted URL the caller immediately navigates the whole tab to --
//! there is nothing to cache. `Err` carries the server's own message so
//! the panel can show why a mint was refused (non-manager, no billing
//! account yet, Stripe unreachable).

use crate::api::orgs::{error_text, parse, to_body};
use crate::api::{fetch_json, fetch_json_any_status};
use capsules::{
  BillingCheckoutRequest, BillingPlan, BillingPlanChangeRequest, BillingSessionUrl,
  OrganizationBilling, OrganizationBillingOverview,
};
use uuid::Uuid;

pub async fn overview(org_id: Uuid) -> Option<OrganizationBillingOverview> {
  let response = fetch_json("GET", &format!("/orgs/{org_id}/billing"), None).await?;
  parse(response).await
}

/// Mints a Stripe Checkout session for `plan`; `Ok` is the hosted page URL
/// to navigate to.
pub async fn checkout(org_id: Uuid, plan: BillingPlan) -> Result<String, String> {
  let Some(body) = to_body(&BillingCheckoutRequest { plan }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status(
    "POST",
    &format!("/orgs/{org_id}/billing/checkout"),
    Some(&body),
  )
  .await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    parse::<BillingSessionUrl>(response)
      .await
      .map(|s| s.url)
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
}

/// Moves the org's live subscription to a different paid tier in place --
/// no Stripe-hosted page involved (the portal can't switch a multi-product
/// subscription). `Ok` is Stripe's post-change state; the org row itself is
/// written by the webhook moments later, so callers refetch the overview.
pub async fn change_plan(org_id: Uuid, plan: BillingPlan) -> Result<OrganizationBilling, String> {
  let Some(body) = to_body(&BillingPlanChangeRequest { plan }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) =
    fetch_json_any_status("PUT", &format!("/orgs/{org_id}/billing/plan"), Some(&body)).await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    parse::<OrganizationBilling>(response)
      .await
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
}

/// Mints a Stripe Billing Portal session; `Ok` is the hosted page URL.
pub async fn portal(org_id: Uuid) -> Result<String, String> {
  let Some(response) =
    fetch_json_any_status("POST", &format!("/orgs/{org_id}/billing/portal"), None).await
  else {
    return Err("Network error".to_string());
  };
  if response.ok() {
    parse::<BillingSessionUrl>(response)
      .await
      .map(|s| s.url)
      .ok_or_else(|| "Failed to parse response".to_string())
  } else {
    Err(error_text(&response).await)
  }
}
