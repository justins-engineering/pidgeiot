//! Forwarding an organization's tax identity to its Stripe Customer ahead
//! of Checkout, so the invoice header carries the registration and Stripe
//! Tax can apply the reverse charge or zero rate it earns.
//!
//! The org's business-details row is the source of truth for what the
//! Customer carries. Checkout collects a tax ID only from a Customer that
//! has none, so forwarding first is what keeps a customer from being asked
//! for the same number twice; and when the row holds nothing forwardable,
//! whatever Checkout collected is left where it is, because Checkout is
//! then the only place the number was ever entered.

use capsules::{OrganizationBusinessDetails, TaxIdStatus, normalize_tax_id};
use serde::Deserialize;
use worker::Env;

use super::stripe_api::{StripeError, StripeList, stripe_delete, stripe_get, stripe_post};
use crate::helpers::url_encode_component;

/// A tax ID as Stripe holds it on a Customer.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct StripeTaxIdRow {
  pub id: String,
  #[serde(rename = "type")]
  pub kind: String,
  pub value: String,
}

/// The two Customer fields this module reads: the name it prints on
/// invoices, and the tax IDs it holds (an expanded list).
#[derive(Deserialize)]
struct StripeCustomerIdentity {
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  tax_ids: Option<StripeList<StripeTaxIdRow>>,
}

/// Whether the org's registration goes to Stripe, and why not when not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardDecision<'a> {
  Send { kind: &'static str, value: &'a str },
  Leave(&'static str),
}

/// A registration is forwarded when it is one Stripe can place AND we have
/// no reason to doubt it: VIES confirmed it, or it is a kind nobody checks
/// and the customer declared it. A `pending` number is deliberately not
/// sent even though Stripe would re-validate it -- forwarding it would let
/// an unanswered lookup zero-rate an invoice, and Checkout can collect the
/// number itself in the meantime. A number VIES rejected must never reach
/// an invoice.
pub fn decide_forward(details: &OrganizationBusinessDetails) -> ForwardDecision<'_> {
  let Some(value) = details
    .tax_id
    .as_deref()
    .map(str::trim)
    .filter(|v| !v.is_empty())
  else {
    return ForwardDecision::Leave("no tax ID on file");
  };
  let Some(kind) = details.tax_id_type.stripe_type() else {
    return ForwardDecision::Leave("the registration names no jurisdiction Stripe can place");
  };
  match details.tax_id_status {
    TaxIdStatus::Validated | TaxIdStatus::Unverified => ForwardDecision::Send { kind, value },
    TaxIdStatus::Pending => ForwardDecision::Leave("VIES has not answered yet"),
    TaxIdStatus::Invalid => ForwardDecision::Leave("VIES rejected the number"),
    TaxIdStatus::None => ForwardDecision::Leave("no tax ID on file"),
  }
}

/// The Stripe writes that make a Customer's tax IDs match a decision.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TaxIdReconcile<'a> {
  pub delete: Vec<String>,
  pub create: Option<(&'static str, &'a str)>,
}

/// Against what the Customer already holds: a `Send` leaves exactly one
/// tax ID on the Customer, the org's own, creating it only if no copy is
/// there. Matching is by Stripe type and normalized value, since Stripe
/// keeps the separators its own form was given. A second copy of the
/// same number (two managers checking out at once) is removed like any
/// other stray. A `Leave` touches nothing.
pub fn reconcile_tax_ids<'a>(
  decision: &ForwardDecision<'a>,
  existing: &[StripeTaxIdRow],
) -> TaxIdReconcile<'a> {
  let ForwardDecision::Send { kind, value } = decision else {
    return TaxIdReconcile::default();
  };
  let wanted = normalize_tax_id(value);
  let mut kept = false;
  let mut delete = Vec::new();
  for row in existing {
    let matches = row.kind == *kind && normalize_tax_id(&row.value) == wanted;
    if matches && !kept {
      kept = true;
    } else {
      delete.push(row.id.clone());
    }
  }
  TaxIdReconcile {
    delete,
    create: (!kept).then_some((*kind, *value)),
  }
}

/// What a sync did, for the caller's log line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TaxIdentitySync {
  pub renamed: bool,
  pub deleted: usize,
  pub created: bool,
  /// Why the registration stayed home, when it did.
  pub left: Option<&'static str>,
}

impl TaxIdentitySync {
  pub fn changed(&self) -> bool {
    self.renamed || self.deleted > 0 || self.created
  }
}

/// Brings the Stripe Customer's name and tax IDs into line with the org's
/// business details. One read, then only the writes the read showed to be
/// needed; an org with nothing to send costs no call at all.
///
/// Stale IDs are removed before the org's is created, so a failure in
/// between leaves a Customer with no wrong number on it rather than two
/// numbers -- Checkout then collects.
pub async fn sync_customer_tax_identity(
  env: &Env,
  customer_id: &str,
  details: &OrganizationBusinessDetails,
) -> Result<TaxIdentitySync, StripeError> {
  let business_name = details
    .business_name
    .as_deref()
    .map(str::trim)
    .filter(|n| !n.is_empty());
  let decision = decide_forward(details);

  let mut done = TaxIdentitySync::default();
  if let ForwardDecision::Leave(reason) = decision {
    done.left = Some(reason);
    if business_name.is_none() {
      return Ok(done);
    }
  }

  let customer_path = format!("/v1/customers/{}", url_encode_component(customer_id));
  let customer: StripeCustomerIdentity = stripe_get(
    env,
    &format!(
      "{customer_path}?{}=tax_ids",
      url_encode_component("expand[]")
    ),
  )
  .await?;

  if let Some(name) = business_name
    && customer.name.as_deref() != Some(name)
  {
    stripe_post::<serde_json::Value>(env, &customer_path, &[("name", name)], None).await?;
    done.renamed = true;
  }

  let existing = customer.tax_ids.map(|list| list.data).unwrap_or_default();
  let plan = reconcile_tax_ids(&decision, &existing);
  for id in &plan.delete {
    stripe_delete::<serde_json::Value>(env, &format!("/v1/tax_ids/{}", url_encode_component(id)))
      .await?;
    done.deleted += 1;
  }
  if let Some((kind, value)) = plan.create {
    stripe_post::<serde_json::Value>(
      env,
      "/v1/tax_ids",
      &[
        ("owner[type]", "customer"),
        ("owner[customer]", customer_id),
        ("type", kind),
        ("value", value),
      ],
      None,
    )
    .await?;
    done.created = true;
  }

  Ok(done)
}

#[cfg(test)]
mod tests {
  use super::{ForwardDecision, StripeTaxIdRow, TaxIdReconcile, decide_forward, reconcile_tax_ids};
  use capsules::{OrganizationBusinessDetails, TaxIdStatus, TaxIdType};

  fn details(
    kind: TaxIdType,
    status: TaxIdStatus,
    value: Option<&str>,
  ) -> OrganizationBusinessDetails {
    OrganizationBusinessDetails {
      org_id: uuid::Uuid::nil(),
      business_name: Some("Example GmbH".into()),
      tax_id: value.map(str::to_string),
      tax_id_type: kind,
      tax_id_status: status,
      tax_id_validated_at: None,
      tax_id_checked_at: None,
    }
  }

  fn row(id: &str, kind: &str, value: &str) -> StripeTaxIdRow {
    StripeTaxIdRow {
      id: id.into(),
      kind: kind.into(),
      value: value.into(),
    }
  }

  #[test]
  fn only_a_confirmed_or_declared_registration_is_sent() {
    // The table the whole seam turns on: every status, for a forwardable
    // kind, and what it does.
    let cases: &[(TaxIdType, TaxIdStatus, bool)] = &[
      (TaxIdType::EuVat, TaxIdStatus::Validated, true),
      (TaxIdType::EuVat, TaxIdStatus::Pending, false),
      (TaxIdType::EuVat, TaxIdStatus::Invalid, false),
      (TaxIdType::GbVat, TaxIdStatus::Unverified, true),
      (TaxIdType::AuAbn, TaxIdStatus::Unverified, true),
      (TaxIdType::UsEin, TaxIdStatus::Unverified, true),
      (TaxIdType::Other, TaxIdStatus::Unverified, false),
      (TaxIdType::None, TaxIdStatus::None, false),
    ];
    for (kind, status, sent) in cases {
      let filed = details(*kind, *status, Some("DE123456789"));
      let decision = decide_forward(&filed);
      assert_eq!(
        matches!(decision, ForwardDecision::Send { .. }),
        *sent,
        "{kind}/{status}: {decision:?}"
      );
    }
    assert_eq!(
      decide_forward(&details(
        TaxIdType::EuVat,
        TaxIdStatus::Validated,
        Some("DE123456789")
      )),
      ForwardDecision::Send {
        kind: "eu_vat",
        value: "DE123456789"
      }
    );
  }

  #[test]
  fn a_status_without_a_number_behind_it_is_never_sent() {
    for value in [None, Some(""), Some("   ")] {
      assert!(matches!(
        decide_forward(&details(TaxIdType::EuVat, TaxIdStatus::Validated, value)),
        ForwardDecision::Leave(_)
      ));
    }
  }

  #[test]
  fn a_customer_already_holding_the_number_gets_nothing_created() {
    let decision = ForwardDecision::Send {
      kind: "eu_vat",
      value: "DE123456789",
    };
    // Stripe kept the separators its own form was given; still the same
    // registration.
    let existing = [row("txi_1", "eu_vat", "DE 123 456 789")];
    assert_eq!(
      reconcile_tax_ids(&decision, &existing),
      TaxIdReconcile::default()
    );
  }

  #[test]
  fn a_missing_number_is_created_and_strays_are_removed_first() {
    let decision = ForwardDecision::Send {
      kind: "eu_vat",
      value: "DE123456789",
    };
    let existing = [
      row("txi_old", "eu_vat", "DE999999999"),
      row("txi_gb", "gb_vat", "GB123456789"),
    ];
    let plan = reconcile_tax_ids(&decision, &existing);
    assert_eq!(
      plan.delete,
      vec!["txi_old".to_string(), "txi_gb".to_string()]
    );
    assert_eq!(plan.create, Some(("eu_vat", "DE123456789")));

    // Same type, different value: replaced, not accumulated.
    let plan = reconcile_tax_ids(&decision, &[row("txi_old", "eu_vat", "DE999999999")]);
    assert_eq!(plan.delete, vec!["txi_old".to_string()]);
    assert!(plan.create.is_some());

    // Two copies of the right number: one survives.
    let plan = reconcile_tax_ids(
      &decision,
      &[
        row("txi_a", "eu_vat", "DE123456789"),
        row("txi_b", "eu_vat", "DE123456789"),
      ],
    );
    assert_eq!(plan.delete, vec!["txi_b".to_string()]);
    assert_eq!(plan.create, None);
  }

  #[test]
  fn leaving_touches_nothing_stripe_already_holds() {
    // The org filed nothing forwardable; a number Checkout collected is the
    // only one the customer ever entered and stays.
    let existing = [row("txi_checkout", "gb_vat", "GB123456789")];
    for reason in ["VIES has not answered yet", "no tax ID on file"] {
      assert_eq!(
        reconcile_tax_ids(&ForwardDecision::Leave(reason), &existing),
        TaxIdReconcile::default()
      );
    }
  }
}
