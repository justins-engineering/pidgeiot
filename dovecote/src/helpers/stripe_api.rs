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
  use super::form_encode;

  #[test]
  fn form_encoding_escapes_values_and_bracketed_keys() {
    assert_eq!(
      form_encode(&[("email", "a+b@example.com"), ("items[0][price]", "price_1")]),
      "email=a%2Bb%40example.com&items%5B0%5D%5Bprice%5D=price_1"
    );
    assert_eq!(form_encode(&[]), "");
  }
}
