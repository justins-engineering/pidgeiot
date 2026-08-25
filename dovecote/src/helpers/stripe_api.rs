use serde::Deserialize;
use serde::de::DeserializeOwned;
use worker::{Env, Fetch, Method, Request, RequestInit};

use crate::helpers::url_encode_component;

/// Stripe's REST origin. Overridable per environment via the
/// `STRIPE_API_BASE` var purely so a local stub can stand in for Stripe
/// while no key exists; every deployed environment leaves it unset and
/// talks to Stripe itself.
const STRIPE_API_BASE_DEFAULT: &str = "https://api.stripe.com";
const STRIPE_API_BASE_VAR: &str = "STRIPE_API_BASE";

/// Worker secret holding the Stripe restricted/secret API key. Never
/// logged, never echoed into an error message -- `StripeError` carries
/// Stripe's own error fields only.
const STRIPE_SECRET_KEY: &str = "STRIPE_SECRET_KEY";

/// A failed Stripe call, mapped from either a transport failure (no
/// `status`) or Stripe's own JSON error envelope. `message` is Stripe's
/// own text; request bodies are never carried here, since billing payloads
/// contain customer PII.
#[derive(Debug)]
pub struct StripeError {
  pub status: Option<u16>,
  pub kind: String,
  pub code: Option<String>,
  pub message: String,
}

impl std::fmt::Display for StripeError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match (self.status, &self.code) {
      (Some(status), Some(code)) => write!(
        f,
        "stripe {} {} ({}): {}",
        status, self.kind, code, self.message
      ),
      (Some(status), None) => write!(f, "stripe {} {}: {}", status, self.kind, self.message),
      (None, _) => write!(f, "stripe {}: {}", self.kind, self.message),
    }
  }
}

impl StripeError {
  fn transport(message: impl Into<String>) -> Self {
    StripeError {
      status: None,
      kind: "transport_error".into(),
      code: None,
      message: message.into(),
    }
  }
}

#[derive(Deserialize)]
struct StripeErrorEnvelope {
  error: StripeErrorBody,
}

#[derive(Deserialize)]
struct StripeErrorBody {
  #[serde(rename = "type")]
  kind: Option<String>,
  code: Option<String>,
  message: Option<String>,
}

/// Encodes parameters the way Stripe's API expects them: form-urlencoded,
/// not JSON, on every request. Nested parameters use bracket syntax in the
/// KEY (`items[0][price]`), so callers pass them pre-bracketed and this
/// only has to encode -- keeping one flat, obvious representation instead
/// of a nested builder that would have to reinvent the same convention.
pub fn form_encode(params: &[(&str, &str)]) -> String {
  params
    .iter()
    .map(|(k, v)| format!("{}={}", url_encode_component(k), url_encode_component(v)))
    .collect::<Vec<_>>()
    .join("&")
}

fn api_base(env: &Env) -> String {
  env
    .var(STRIPE_API_BASE_VAR)
    .map(|v| v.to_string())
    .ok()
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| STRIPE_API_BASE_DEFAULT.to_string())
}

/// POSTs form-encoded parameters to a Stripe REST path (e.g.
/// `/v1/customers`) and parses the JSON response into `T`.
///
/// `idempotency_key` should be set on anything that creates or charges:
/// Stripe deduplicates retries of the same key for 24 hours, which is the
/// only thing standing between a retried Worker invocation and a duplicate
/// charge.
pub async fn stripe_post<T: DeserializeOwned>(
  env: &Env,
  path: &str,
  params: &[(&str, &str)],
  idempotency_key: Option<&str>,
) -> Result<T, StripeError> {
  stripe_request(
    env,
    Method::Post,
    path,
    Some(form_encode(params)),
    idempotency_key,
  )
  .await
}

pub async fn stripe_get<T: DeserializeOwned>(env: &Env, path: &str) -> Result<T, StripeError> {
  stripe_request(env, Method::Get, path, None, None).await
}

async fn stripe_request<T: DeserializeOwned>(
  env: &Env,
  method: Method,
  path: &str,
  body: Option<String>,
  idempotency_key: Option<&str>,
) -> Result<T, StripeError> {
  let Ok(key) = env.secret(STRIPE_SECRET_KEY).map(|k| k.to_string()) else {
    return Err(StripeError::transport(format!(
      "{STRIPE_SECRET_KEY} is not configured for this environment"
    )));
  };

  let mut init = RequestInit::default();
  init.with_method(method);

  if init
    .headers
    .set("Authorization", &format!("Bearer {key}"))
    .is_err()
  {
    return Err(StripeError::transport("failed to set Authorization header"));
  }

  if let Some(body) = body {
    if init
      .headers
      .set("Content-Type", "application/x-www-form-urlencoded")
      .is_err()
    {
      return Err(StripeError::transport("failed to set Content-Type header"));
    }
    init.body = Some(body.into());
  }

  if let Some(idempotency_key) = idempotency_key
    && init
      .headers
      .set("Idempotency-Key", idempotency_key)
      .is_err()
  {
    return Err(StripeError::transport(
      "failed to set Idempotency-Key header",
    ));
  }

  let url = format!("{}{path}", api_base(env));
  let Ok(req) = Request::new_with_init(&url, &init) else {
    return Err(StripeError::transport("failed to build Stripe request"));
  };

  let mut resp = match Fetch::Request(req).send().await {
    Ok(resp) => resp,
    Err(e) => return Err(StripeError::transport(format!("fetch failed: {e}"))),
  };

  let status = resp.status_code();
  let Ok(text) = resp.text().await else {
    return Err(StripeError::transport(format!(
      "could not read Stripe response body (HTTP {status})"
    )));
  };

  if status >= 400 {
    return Err(match serde_json::from_str::<StripeErrorEnvelope>(&text) {
      Ok(envelope) => StripeError {
        status: Some(status),
        kind: envelope.error.kind.unwrap_or_else(|| "api_error".into()),
        code: envelope.error.code,
        message: envelope
          .error
          .message
          .unwrap_or_else(|| "no message returned".into()),
      },
      // A non-Stripe error body means something between us and Stripe
      // answered (a proxy, an outage page). Say so rather than reporting
      // the HTML back as if Stripe had said it.
      Err(_) => StripeError {
        status: Some(status),
        kind: "api_error".into(),
        code: None,
        message: "unparseable error response".into(),
      },
    });
  }

  serde_json::from_str::<T>(&text).map_err(|e| StripeError {
    status: Some(status),
    kind: "response_parse_error".into(),
    code: None,
    message: format!("could not parse Stripe response: {e}"),
  })
}

/// Whether outbound Stripe calls can work in this environment at all --
/// lets cron-path callers skip cleanly (dev, or an env whose billing isn't
/// provisioned yet) instead of failing per org.
pub fn stripe_configured(env: &Env) -> bool {
  env
    .secret(STRIPE_SECRET_KEY)
    .ok()
    .is_some_and(|k| !k.to_string().trim().is_empty())
}

/// The generic `{ "data": [...] }` envelope Stripe list endpoints return.
/// Only `data` is modeled; pagination is a caller concern (every caller
/// here queries by explicit lookup_keys, well under one page).
#[derive(Deserialize)]
pub struct StripeList<T> {
  pub data: Vec<T>,
}

#[derive(Deserialize)]
pub struct StripePrice {
  pub id: String,
  #[serde(default)]
  pub lookup_key: Option<String>,
  #[serde(default)]
  pub recurring: Option<StripeRecurring>,
}

#[derive(Deserialize)]
pub struct StripeRecurring {
  /// The Billing Meter id a metered price reads from -- absent on licensed
  /// (flat) prices.
  #[serde(default)]
  pub meter: Option<String>,
}

#[derive(Deserialize)]
struct StripeMeter {
  event_name: String,
}

/// Fetches the active prices for the given lookup_keys. Everything billing
/// touches resolves through lookup_keys at run time -- prices can be
/// recreated at new amounts without orphaning code that would otherwise
/// have pinned their generated ids.
pub async fn resolve_prices_by_lookup_keys(
  env: &Env,
  lookup_keys: &[&str],
) -> Result<Vec<StripePrice>, StripeError> {
  let mut path = String::from("/v1/prices?active=true");
  for key in lookup_keys {
    path.push('&');
    path.push_str(&url_encode_component("lookup_keys[]"));
    path.push('=');
    path.push_str(&url_encode_component(key));
  }
  let list: StripeList<StripePrice> = stripe_get(env, &path).await?;
  Ok(list.data)
}

/// Resolves the meter event_name behind a metered price: lookup_key ->
/// price -> bound meter -> event_name. Two GETs, but it keeps the meter's
/// name a property of the catalog rather than a constant here -- the same
/// no-pinned-ids rule as `resolve_prices_by_lookup_keys`.
pub async fn resolve_meter_event_name(
  env: &Env,
  price_lookup_key: &str,
) -> Result<String, StripeError> {
  let prices = resolve_prices_by_lookup_keys(env, &[price_lookup_key]).await?;
  let Some(price) = prices
    .into_iter()
    .find(|p| p.lookup_key.as_deref() == Some(price_lookup_key))
  else {
    return Err(StripeError::transport(format!(
      "no active price with lookup_key '{price_lookup_key}'"
    )));
  };
  let Some(meter_id) = price.recurring.and_then(|r| r.meter) else {
    return Err(StripeError::transport(format!(
      "price '{price_lookup_key}' is not bound to a billing meter"
    )));
  };
  let meter: StripeMeter = stripe_get(env, &format!("/v1/billing/meters/{meter_id}")).await?;
  Ok(meter.event_name)
}

/// The three prices a paid-tier Checkout session carries: the licensed
/// tier itself plus both metered overage prices (pooled message overage,
/// and that tier's own per-device overage rate). Attached together at
/// checkout so the subscription can bill overage without a later
/// subscription edit.
pub struct CheckoutPrices {
  pub tier_price_id: String,
  pub message_overage_price_id: String,
  pub device_overage_price_id: String,
}

/// Resolves the three Checkout price ids for a tier by lookup_key, in one
/// list call. Missing catalog entries are an error, not a guess -- a
/// session created without its overage prices would silently sell
/// unmetered usage.
pub async fn resolve_checkout_prices(
  env: &Env,
  tier: capsules::BillingPlan,
) -> Result<CheckoutPrices, StripeError> {
  let tier_key = tier.as_str();
  let device_key = format!("device-overage-{tier_key}");
  let prices =
    resolve_prices_by_lookup_keys(env, &[tier_key, "message-overage", &device_key]).await?;

  let find = |key: &str| {
    prices
      .iter()
      .find(|p| p.lookup_key.as_deref() == Some(key))
      .map(|p| p.id.clone())
      .ok_or_else(|| StripeError::transport(format!("no active price with lookup_key '{key}'")))
  };

  Ok(CheckoutPrices {
    tier_price_id: find(tier_key)?,
    message_overage_price_id: find("message-overage")?,
    device_overage_price_id: find(&device_key)?,
  })
}

#[derive(Deserialize)]
struct StripeCustomer {
  id: String,
}

/// Creates the Stripe customer an org's billing hangs off. Idempotency key
/// is derived from the org id, so a retried request within Stripe's replay
/// window returns the same customer instead of minting a duplicate; the
/// caller still writes the id back with a keep-the-first COALESCE.
pub async fn create_customer(
  env: &Env,
  org_id: &str,
  org_name: &str,
  email: Option<&str>,
) -> Result<String, StripeError> {
  let mut params = vec![("name", org_name), ("metadata[org_id]", org_id)];
  if let Some(email) = email {
    params.push(("email", email));
  }
  let idempotency_key = format!("customer-{org_id}");
  let customer: StripeCustomer =
    stripe_post(env, "/v1/customers", &params, Some(&idempotency_key)).await?;
  Ok(customer.id)
}

#[derive(Deserialize)]
struct StripeCheckoutSession {
  #[serde(default)]
  url: Option<String>,
}

/// The complete form body a paid-tier Checkout session is created with:
/// licensed tier price plus both metered overage prices (metered items
/// carry no quantity), the org named twice (`client_reference_id` and
/// `subscription_data[metadata]`) so the webhook can bind the result even
/// if the org row's customer id were somehow missing, and the tax set.
///
/// Built apart from the request so the exact set is pinned by a test.
/// Stripe Tax is active on the account but computes nothing for a session
/// that does not ask, and it can only compute against an address it has
/// been given -- hence the address collection, and the `customer_update`
/// entries that let Checkout write that address (and the legal business
/// name) back onto the Customer it was handed, where every later
/// subscription invoice reads them from.
pub fn checkout_session_params<'a>(
  customer_id: &'a str,
  org_id: &'a str,
  tier: capsules::BillingPlan,
  prices: &'a CheckoutPrices,
  success_url: &'a str,
  cancel_url: &'a str,
) -> Vec<(&'static str, &'a str)> {
  vec![
    ("mode", "subscription"),
    ("customer", customer_id),
    ("client_reference_id", org_id),
    ("success_url", success_url),
    ("cancel_url", cancel_url),
    ("line_items[0][price]", &prices.tier_price_id),
    ("line_items[0][quantity]", "1"),
    ("line_items[1][price]", &prices.message_overage_price_id),
    ("line_items[2][price]", &prices.device_overage_price_id),
    ("subscription_data[metadata][plan]", tier.as_str()),
    ("subscription_data[metadata][org_id]", org_id),
    ("automatic_tax[enabled]", "true"),
    ("billing_address_collection", "required"),
    ("customer_update[address]", "auto"),
    ("customer_update[name]", "auto"),
    // Checkout shows this form only to a Customer with no tax ID yet, so
    // an org whose registration was forwarded ahead of the session never
    // sees it. `if_supported` is what keeps a sale abroad B2B: wherever
    // Checkout can collect a tax ID, a buyer without one cannot pay.
    ("tax_id_collection[enabled]", "true"),
    ("tax_id_collection[required]", "if_supported"),
  ]
}

/// Creates a subscription-mode Checkout session for a paid tier; the body
/// is `checkout_session_params`.
pub async fn create_checkout_session(
  env: &Env,
  customer_id: &str,
  org_id: &str,
  tier: capsules::BillingPlan,
  prices: &CheckoutPrices,
  success_url: &str,
  cancel_url: &str,
) -> Result<String, StripeError> {
  let params = checkout_session_params(customer_id, org_id, tier, prices, success_url, cancel_url);
  let session: StripeCheckoutSession =
    stripe_post(env, "/v1/checkout/sessions", &params, None).await?;

  session
    .url
    .ok_or_else(|| StripeError::transport("checkout session created but carried no redirect URL"))
}

/// Moves a live subscription to a different paid tier in one Subscriptions
/// Update call: the licensed item is re-priced to the new tier's flat
/// price, and the per-device overage item to that tier's own rate (added
/// outright on a subscription that predates the metered composition). The
/// pooled message-overage item shares one rate across tiers, so it is left
/// alone. `metadata[plan]` is rewritten in the same call because the
/// webhook resolves the tier from metadata before the licensed item --
/// leaving the old name there would apply the old tier right back.
///
/// `create_prorations` in both directions: an upgrade charges the price
/// difference for the rest of the period onto the next invoice, a
/// downgrade credits it.
///
/// Turning Stripe Tax on at the account does not reach into subscriptions
/// that already exist. This is the one write made to a live subscription,
/// so it is where one created before tax was enabled converges.
pub fn subscription_tier_params<'a>(
  licensed_item_id: &'a str,
  device_overage_item_id: Option<&'a str>,
  prices: &'a CheckoutPrices,
  tier: capsules::BillingPlan,
) -> Vec<(&'static str, &'a str)> {
  let mut params = vec![
    ("items[0][id]", licensed_item_id),
    ("items[0][price]", prices.tier_price_id.as_str()),
    ("items[1][price]", prices.device_overage_price_id.as_str()),
    ("proration_behavior", "create_prorations"),
    ("metadata[plan]", tier.as_str()),
    ("automatic_tax[enabled]", "true"),
  ];
  if let Some(device_item_id) = device_overage_item_id {
    params.push(("items[1][id]", device_item_id));
  }
  params
}

/// Applies `subscription_tier_params` to a live subscription. No
/// idempotency key on purpose -- a retry that sets the same prices again
/// is a semantic no-op, while a deterministic key would make a later
/// legitimate switch back to this tier replay the stale cached response
/// instead of applying.
pub async fn update_subscription_tier(
  env: &Env,
  subscription_id: &str,
  licensed_item_id: &str,
  device_overage_item_id: Option<&str>,
  prices: &CheckoutPrices,
  tier: capsules::BillingPlan,
) -> Result<capsules::StripeSubscriptionRow, StripeError> {
  let params = subscription_tier_params(licensed_item_id, device_overage_item_id, prices, tier);
  stripe_post(
    env,
    &format!(
      "/v1/subscriptions/{}",
      url_encode_component(subscription_id)
    ),
    &params,
    None,
  )
  .await
}

#[derive(Deserialize)]
struct StripePortalSession {
  url: String,
}

/// Mints a hosted Billing Portal session for an existing customer -- plan
/// changes, card updates and cancellation all happen on Stripe's page, so
/// none of those flows need building here.
pub async fn create_portal_session(
  env: &Env,
  customer_id: &str,
  return_url: &str,
) -> Result<String, StripeError> {
  let session: StripePortalSession = stripe_post(
    env,
    "/v1/billing_portal/sessions",
    &[("customer", customer_id), ("return_url", return_url)],
    None,
  )
  .await?;
  Ok(session.url)
}

/// The slice of a `checkout.session.completed` webhook object the handler
/// needs: who paid (customer), what it bought (subscription), and which
/// org started the session.
#[derive(Deserialize, Debug)]
pub struct StripeCheckoutSessionRow {
  #[serde(default)]
  pub customer: Option<String>,
  #[serde(default)]
  pub subscription: Option<String>,
  #[serde(default)]
  pub client_reference_id: Option<String>,
}

/// Fetches a subscription's current state -- used when a completed
/// checkout names a subscription whose own lifecycle events may not have
/// arrived yet (Stripe delivers events unordered).
pub async fn fetch_subscription(
  env: &Env,
  subscription_id: &str,
) -> Result<capsules::StripeSubscriptionRow, StripeError> {
  stripe_get(
    env,
    &format!(
      "/v1/subscriptions/{}",
      url_encode_component(subscription_id)
    ),
  )
  .await
}

/// Posts one usage figure to a Stripe billing meter. `identifier` is the
/// deduplication key: deterministic per (org, period, day) at the call
/// sites, so a same-day replay of the same figure cannot double-bill.
pub async fn post_meter_event(
  env: &Env,
  event_name: &str,
  stripe_customer_id: &str,
  value: i64,
  identifier: &str,
) -> Result<(), StripeError> {
  let value = value.to_string();
  stripe_post::<serde_json::Value>(
    env,
    "/v1/billing/meter_events",
    &[
      ("event_name", event_name),
      ("payload[stripe_customer_id]", stripe_customer_id),
      ("payload[value]", &value),
      ("identifier", identifier),
    ],
    None,
  )
  .await
  .map(|_| ())
}

#[cfg(test)]
mod tests {
  use super::{CheckoutPrices, checkout_session_params, form_encode, subscription_tier_params};
  use capsules::BillingPlan;

  #[test]
  fn form_encoding_escapes_values_and_bracketed_keys() {
    assert_eq!(
      form_encode(&[("email", "a+b@example.com"), ("items[0][price]", "price_1")]),
      "email=a%2Bb%40example.com&items%5B0%5D%5Bprice%5D=price_1"
    );
    assert_eq!(form_encode(&[]), "");
  }

  fn prices() -> CheckoutPrices {
    CheckoutPrices {
      tier_price_id: "price_tier".into(),
      message_overage_price_id: "price_msg".into(),
      device_overage_price_id: "price_dev".into(),
    }
  }

  /// The tax-related half of the session, in full. Every entry here is
  /// something Stripe Tax needs before it computes anything; dropping any
  /// one of them silently produces untaxed subscriptions again.
  const TAX_PARAMS: &[(&str, &str)] = &[
    ("automatic_tax[enabled]", "true"),
    ("billing_address_collection", "required"),
    ("customer_update[address]", "auto"),
    ("customer_update[name]", "auto"),
    ("tax_id_collection[enabled]", "true"),
    ("tax_id_collection[required]", "if_supported"),
  ];

  #[test]
  fn checkout_session_body_is_exactly_the_documented_set() {
    let prices = prices();
    let params = checkout_session_params(
      "cus_1",
      "org-1",
      BillingPlan::Growth,
      &prices,
      "https://app/ok",
      "https://app/no",
    );
    let mut expected: Vec<(&str, &str)> = vec![
      ("mode", "subscription"),
      ("customer", "cus_1"),
      ("client_reference_id", "org-1"),
      ("success_url", "https://app/ok"),
      ("cancel_url", "https://app/no"),
      ("line_items[0][price]", "price_tier"),
      ("line_items[0][quantity]", "1"),
      ("line_items[1][price]", "price_msg"),
      ("line_items[2][price]", "price_dev"),
      ("subscription_data[metadata][plan]", "growth"),
      ("subscription_data[metadata][org_id]", "org-1"),
    ];
    expected.extend_from_slice(TAX_PARAMS);
    assert_eq!(params, expected);
  }

  #[test]
  fn every_tax_parameter_appears_once_for_every_tier() {
    let prices = prices();
    for tier in [
      BillingPlan::Builder,
      BillingPlan::Growth,
      BillingPlan::Scale,
      BillingPlan::Fleet,
    ] {
      let params = checkout_session_params("cus_1", "org-1", tier, &prices, "s", "c");
      for (key, value) in TAX_PARAMS {
        let found: Vec<&str> = params
          .iter()
          .filter(|(k, _)| k == key)
          .map(|(_, v)| *v)
          .collect();
        assert_eq!(found, vec![*value], "{key} for {tier:?}");
      }
      // Exemption is Stripe Tax's decision from the address and tax ID,
      // never asserted here by hand.
      assert!(
        params.iter().all(|(k, _)| !k.contains("tax_exempt")),
        "{tier:?} set tax_exempt by hand"
      );
      // And the whole thing survives form encoding with its brackets.
      let body = form_encode(&params);
      assert!(body.contains("automatic_tax%5Benabled%5D=true"));
      assert!(body.contains("tax_id_collection%5Brequired%5D=if_supported"));
    }
  }

  #[test]
  fn plan_change_asks_for_tax_and_addresses_the_device_item_only_when_it_exists() {
    let prices = prices();
    let with_item = subscription_tier_params("si_lic", Some("si_dev"), &prices, BillingPlan::Scale);
    assert!(with_item.contains(&("automatic_tax[enabled]", "true")));
    assert!(with_item.contains(&("items[1][id]", "si_dev")));
    assert!(with_item.contains(&("items[1][price]", "price_dev")));
    assert!(with_item.contains(&("metadata[plan]", "scale")));

    let without_item = subscription_tier_params("si_lic", None, &prices, BillingPlan::Scale);
    assert!(without_item.contains(&("automatic_tax[enabled]", "true")));
    assert!(without_item.iter().all(|(k, _)| *k != "items[1][id]"));
    assert!(without_item.contains(&("items[1][price]", "price_dev")));
  }
}
