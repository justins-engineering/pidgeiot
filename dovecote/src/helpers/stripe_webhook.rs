use js_sys::{Array, Object, Reflect, Uint8Array};
use serde::Deserialize;
use wasm_bindgen::JsCast;
use web_sys::{CryptoKey, SubtleCrypto, WorkerGlobalScope};

use crate::helpers::constant_time_eq;

/// Worker secret holding the endpoint signing secret (`whsec_...`) for the
/// webhook route. Per-endpoint at Stripe, so staging and production each
/// have their own.
pub const STRIPE_WEBHOOK_SECRET: &str = "STRIPE_WEBHOOK_SECRET";

/// Replay window on a webhook's `t=` timestamp, matching the default every
/// Stripe library ships. Never zero: a strict equality check would reject
/// on ordinary network latency, and a disabled check accepts a captured
/// request forever.
pub const WEBHOOK_TOLERANCE_SECS: i64 = 300;

/// A webhook event envelope. `data.object` stays an untyped `Value`
/// because one endpoint receives every subscribed event type and only the
/// handler for a given `kind` knows what shape to expect -- decoding it
/// eagerly here would mean one union type that has to grow for every event
/// we ever subscribe to.
#[derive(Deserialize, Debug)]
pub struct StripeWebhookEvent {
  pub id: String,
  #[serde(rename = "type")]
  pub kind: String,
  /// Unix seconds. Used both as the replay-ordering key when applying
  /// state and as the audit timestamp on the idempotency row.
  pub created: i64,
  #[serde(default)]
  pub api_version: Option<String>,
  #[serde(default)]
  pub livemode: bool,
  pub data: StripeWebhookEventData,
}

#[derive(Deserialize, Debug)]
pub struct StripeWebhookEventData {
  pub object: serde_json::Value,
}

/// What the sink does with an event of a given type. Named in one place
/// so the list of events the endpoint subscribes to in the Dashboard can
/// be read against the list the code acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookAction {
  /// Any `customer.subscription.*`: write the subscription's state onto
  /// the org that owns it.
  ApplySubscription,
  /// `checkout.session.completed`: bind the customer to the org, then
  /// apply the purchased subscription.
  ApplyCheckoutCompletion,
  /// `invoice.finalization_failed`: nothing to apply, but Stripe keeps the
  /// subscription active while an invoice cannot be finalized, so it must
  /// be seen -- logged and mailed.
  ReportInvoiceFinalizationFailure,
  /// `checkout.session.async_payment_succeeded`: a delayed-notification
  /// payment (ACH Direct Debit) cleared after the session completed.
  /// Entitlement is decided by the subscription's own status, so this is
  /// only logged; it closes the loop the failure case opens.
  ReportAsyncPaymentSucceeded,
  /// `checkout.session.async_payment_failed`: the first debit bounced.
  /// The subscription's own status change follows through Stripe's
  /// dunning, later; this is the earliest signal that a new customer has
  /// not actually paid, so it is logged and mailed.
  ReportAsyncPaymentFailed,
  /// Everything else is acknowledged without acting.
  Acknowledge,
}

pub fn webhook_action(kind: &str) -> WebhookAction {
  match kind {
    "checkout.session.completed" => WebhookAction::ApplyCheckoutCompletion,
    "checkout.session.async_payment_succeeded" => WebhookAction::ReportAsyncPaymentSucceeded,
    "checkout.session.async_payment_failed" => WebhookAction::ReportAsyncPaymentFailed,
    "invoice.finalization_failed" => WebhookAction::ReportInvoiceFinalizationFailure,
    _ if kind.starts_with("customer.subscription.") => WebhookAction::ApplySubscription,
    _ => WebhookAction::Acknowledge,
  }
}

/// The slice of a failed invoice worth reporting. `customer` and
/// `subscription` are kept as raw values because Stripe sends an id string
/// on a webhook but an expanded object on some reads, and a report must
/// not fail to parse over that.
#[derive(Deserialize, Debug, Default)]
pub struct StripeInvoiceFailureRow {
  #[serde(default)]
  pub id: Option<String>,
  #[serde(default)]
  customer: Option<serde_json::Value>,
  #[serde(default)]
  subscription: Option<serde_json::Value>,
  #[serde(default)]
  pub automatic_tax: Option<StripeInvoiceAutomaticTax>,
  #[serde(default)]
  pub last_finalization_error: Option<StripeInvoiceFinalizationError>,
}

#[derive(Deserialize, Debug, Default)]
pub struct StripeInvoiceAutomaticTax {
  #[serde(default)]
  pub enabled: bool,
  /// `requires_location_inputs` is the one Stripe Tax produces when the
  /// customer's address cannot be resolved for a calculation.
  #[serde(default)]
  pub status: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct StripeInvoiceFinalizationError {
  #[serde(default)]
  pub code: Option<String>,
  #[serde(default, rename = "type")]
  pub kind: Option<String>,
  #[serde(default)]
  pub message: Option<String>,
}

fn id_of(value: Option<&serde_json::Value>) -> Option<&str> {
  let value = value?;
  value
    .as_str()
    .or_else(|| value.get("id").and_then(|id| id.as_str()))
}

impl StripeInvoiceFailureRow {
  pub fn customer_id(&self) -> Option<&str> {
    id_of(self.customer.as_ref())
  }

  pub fn subscription_id(&self) -> Option<&str> {
    id_of(self.subscription.as_ref())
  }

  /// One line naming the invoice, whose it is, and why it did not
  /// finalize. Stripe object ids and Stripe's own error text only -- no
  /// amounts, no address.
  pub fn summary(&self) -> String {
    let tax = match &self.automatic_tax {
      Some(tax) if tax.enabled => format!(
        "automatic_tax {}",
        tax.status.as_deref().unwrap_or("status unknown")
      ),
      Some(_) => "automatic_tax off".to_string(),
      None => "automatic_tax absent".to_string(),
    };
    let error = match &self.last_finalization_error {
      Some(err) => format!(
        "{} {}: {}",
        err.kind.as_deref().unwrap_or("error"),
        err.code.as_deref().unwrap_or("no code"),
        err.message.as_deref().unwrap_or("no message")
      ),
      None => "no finalization error recorded".to_string(),
    };
    format!(
      "invoice {} for customer {} (subscription {}): {tax}; {error}",
      self.id.as_deref().unwrap_or("unknown"),
      self.customer_id().unwrap_or("unknown"),
      self.subscription_id().unwrap_or("none"),
    )
  }
}

/// The parsed `Stripe-Signature` header: one timestamp plus every `v1`
/// signature it carries. There can legitimately be more than one during a
/// signing-secret roll, when Stripe signs with both.
#[derive(Debug, PartialEq)]
pub struct StripeSignatureHeader {
  pub timestamp: i64,
  pub signatures: Vec<Vec<u8>>,
}

/// Parses `t=1492774577,v1=5257a86...,v1=...`. Every scheme that isn't
/// `v1` is dropped on the floor -- `v0` in particular is a documented
/// test-mode-only value, and accepting it would be a downgrade attack.
/// Returns `None` if there is no timestamp or no usable `v1`.
pub fn parse_signature_header(raw: &str) -> Option<StripeSignatureHeader> {
  let mut timestamp = None;
  let mut signatures = Vec::new();

  for element in raw.split(',') {
    let Some((scheme, value)) = element.trim().split_once('=') else {
      continue;
    };
    match scheme {
      "t" => timestamp = value.parse::<i64>().ok(),
      "v1" => {
        if let Some(bytes) = hex_decode(value) {
          signatures.push(bytes);
        }
      }
      _ => {}
    }
  }

  match (timestamp, signatures.is_empty()) {
    (Some(timestamp), false) => Some(StripeSignatureHeader {
      timestamp,
      signatures,
    }),
    _ => None,
  }
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
  if !value.len().is_multiple_of(2) {
    return None;
  }
  value
    .as_bytes()
    .chunks(2)
    .map(|pair| {
      let hi = (pair[0] as char).to_digit(16)?;
      let lo = (pair[1] as char).to_digit(16)?;
      Some(((hi << 4) | lo) as u8)
    })
    .collect()
}

/// Rejects a signature whose timestamp sits outside the replay window in
/// EITHER direction. Stripe's own libraries only bound the past, but a
/// far-future timestamp is equally a sign of a forged or replayed header,
/// and nothing legitimate produces one.
pub fn timestamp_within_tolerance(timestamp: i64, now: i64, tolerance: i64) -> bool {
  (now - timestamp).abs() <= tolerance
}

/// Whether any signature on the header matches the MAC we computed.
/// Compared in constant time, and "any" rather than "the first" so a
/// secret roll doesn't reject the half of the traffic signed with the
/// other key.
pub fn signature_matches(expected_mac: &[u8], header: &StripeSignatureHeader) -> bool {
  header
    .signatures
    .iter()
    .any(|candidate| constant_time_eq(expected_mac, candidate))
}

/// The bytes Stripe actually signs: `{timestamp}.{raw_body}`. Built from
/// the raw request bytes -- any reserialization of the JSON changes them
/// and breaks the signature.
pub fn signed_payload(timestamp: i64, body: &[u8]) -> Vec<u8> {
  let mut payload = Vec::with_capacity(24 + body.len());
  payload.extend_from_slice(timestamp.to_string().as_bytes());
  payload.push(b'.');
  payload.extend_from_slice(body);
  payload
}

/// Full webhook verification: parse the header, bound the replay window,
/// HMAC-SHA256 the signed payload with the endpoint secret, and compare in
/// constant time. `now` is passed in rather than read here so the caller
/// owns the clock and this stays exercisable against fixed vectors.
pub async fn verify_webhook_signature(
  secret: &str,
  raw_header: &str,
  body: &[u8],
  now: i64,
) -> Result<(), String> {
  let Some(header) = parse_signature_header(raw_header) else {
    return Err("Malformed Stripe-Signature header".into());
  };

  if !timestamp_within_tolerance(header.timestamp, now, WEBHOOK_TOLERANCE_SECS) {
    return Err("Stripe-Signature timestamp outside tolerance".into());
  }

  let mac = hmac_sha256(secret.as_bytes(), &signed_payload(header.timestamp, body)).await?;

  if signature_matches(&mac, &header) {
    Ok(())
  } else {
    Err("Stripe-Signature does not match".into())
  }
}

/// HMAC-SHA256 via WebCrypto, the same `SubtleCrypto`-through-`web-sys`
/// route `helpers/access.rs` already uses for Access RS256 verification,
/// rather than adding an `hmac` crate for one call site.
pub async fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
  let subtle = subtle_crypto()?;

  let algorithm = Object::new();
  Reflect::set(&algorithm, &"name".into(), &"HMAC".into())
    .map_err(|e| format!("building HMAC algorithm failed: {e:?}"))?;
  Reflect::set(&algorithm, &"hash".into(), &"SHA-256".into())
    .map_err(|e| format!("building HMAC algorithm failed: {e:?}"))?;

  let usages = Array::new();
  usages.push(&"sign".into());

  let key_data = Uint8Array::from(key);
  let key_promise = subtle
    .import_key_with_object("raw", key_data.unchecked_ref(), &algorithm, false, &usages)
    .map_err(|e| format!("importKey call failed: {e:?}"))?;
  let key_value = wasm_bindgen_futures::JsFuture::from(key_promise)
    .await
    .map_err(|e| format!("importKey rejected: {e:?}"))?;
  let key: CryptoKey = key_value.unchecked_into();

  let sign_promise = subtle
    .sign_with_str_and_u8_array("HMAC", &key, data)
    .map_err(|e| format!("sign call failed: {e:?}"))?;
  let signature = wasm_bindgen_futures::JsFuture::from(sign_promise)
    .await
    .map_err(|e| format!("sign rejected: {e:?}"))?;

  Ok(Uint8Array::new(&signature).to_vec())
}

fn subtle_crypto() -> Result<SubtleCrypto, String> {
  let global: WorkerGlobalScope = js_sys::global().unchecked_into();
  let crypto = global
    .crypto()
    .map_err(|e| format!("crypto unavailable in this isolate: {e:?}"))?;
  Ok(crypto.subtle())
}

#[cfg(test)]
mod tests {
  use super::{
    StripeInvoiceFailureRow, StripeSignatureHeader, WebhookAction, parse_signature_header,
    signature_matches, signed_payload, timestamp_within_tolerance, webhook_action,
  };

  #[test]
  fn every_subscribed_event_dispatches_and_nothing_else_acts() {
    // The eight the endpoint has always carried, plus the finalization
    // failure Stripe Tax makes possible. Anything not listed is acked and
    // must stay that way, or an unsubscribed event could change state.
    let cases: &[(&str, WebhookAction)] = &[
      (
        "customer.subscription.created",
        WebhookAction::ApplySubscription,
      ),
      (
        "customer.subscription.updated",
        WebhookAction::ApplySubscription,
      ),
      (
        "customer.subscription.deleted",
        WebhookAction::ApplySubscription,
      ),
      (
        "customer.subscription.paused",
        WebhookAction::ApplySubscription,
      ),
      (
        "customer.subscription.resumed",
        WebhookAction::ApplySubscription,
      ),
      (
        "customer.subscription.pending_update_applied",
        WebhookAction::ApplySubscription,
      ),
      (
        "customer.subscription.pending_update_expired",
        WebhookAction::ApplySubscription,
      ),
      (
        "checkout.session.completed",
        WebhookAction::ApplyCheckoutCompletion,
      ),
      (
        "invoice.finalization_failed",
        WebhookAction::ReportInvoiceFinalizationFailure,
      ),
      (
        "checkout.session.async_payment_succeeded",
        WebhookAction::ReportAsyncPaymentSucceeded,
      ),
      (
        "checkout.session.async_payment_failed",
        WebhookAction::ReportAsyncPaymentFailed,
      ),
      ("invoice.finalized", WebhookAction::Acknowledge),
      ("invoice.paid", WebhookAction::Acknowledge),
      ("customer.tax_id.updated", WebhookAction::Acknowledge),
      ("checkout.session.expired", WebhookAction::Acknowledge),
      ("customer.updated", WebhookAction::Acknowledge),
      ("", WebhookAction::Acknowledge),
    ];
    for (kind, action) in cases {
      assert_eq!(webhook_action(kind), *action, "{kind}");
    }
  }

  #[test]
  fn a_failed_invoice_is_summarised_from_either_id_shape() {
    let on_webhook: StripeInvoiceFailureRow = serde_json::from_str(
      r#"{"id":"in_1","customer":"cus_1","subscription":"sub_1",
          "automatic_tax":{"enabled":true,"status":"requires_location_inputs"},
          "last_finalization_error":{"type":"invalid_request_error",
            "code":"customer_tax_location_invalid",
            "message":"The customer's location could not be determined."}}"#,
    )
    .unwrap();
    assert_eq!(on_webhook.customer_id(), Some("cus_1"));
    assert_eq!(on_webhook.subscription_id(), Some("sub_1"));
    let summary = on_webhook.summary();
    assert!(summary.contains("invoice in_1 for customer cus_1 (subscription sub_1)"));
    assert!(summary.contains("automatic_tax requires_location_inputs"));
    assert!(summary.contains("invalid_request_error customer_tax_location_invalid"));

    // Expanded objects on the same fields still parse and still name ids.
    let expanded: StripeInvoiceFailureRow = serde_json::from_str(
      r#"{"id":"in_2","customer":{"id":"cus_2","object":"customer"},"subscription":null}"#,
    )
    .unwrap();
    assert_eq!(expanded.customer_id(), Some("cus_2"));
    assert_eq!(expanded.subscription_id(), None);
    assert!(
      expanded
        .summary()
        .contains("(subscription none): automatic_tax absent; no finalization error recorded")
    );

    // A body with nothing recognisable still produces a report line.
    let empty: StripeInvoiceFailureRow = serde_json::from_str("{}").unwrap();
    assert!(
      empty
        .summary()
        .starts_with("invoice unknown for customer unknown")
    );
  }

  // Fixed vector, computed outside this code so it checks the
  // implementation rather than agreeing with it:
  //   printf '%s' '1754956800.{"id":"evt_test","type":"ping"}' \
  //     | openssl dgst -sha256 -hmac 'whsec_test_secret' -r
  // The WebCrypto half of verification only exists inside a JS isolate, so
  // these tests pin every decision made around it; the HMAC itself is
  // exercised live under `wrangler dev`.
  const FIXTURE_BODY: &[u8] = br#"{"id":"evt_test","type":"ping"}"#;
  const FIXTURE_TIMESTAMP: i64 = 1754956800;
  const GOOD_MAC_HEX: &str = "ab997c6012432059f295c4df65bc67bda3cab67ce85597272035cfbfdba0f7a5";

  fn header(raw: &str) -> StripeSignatureHeader {
    parse_signature_header(raw).expect("header should parse")
  }

  fn mac_bytes(hex: &str) -> Vec<u8> {
    super::hex_decode(hex).expect("test vector should be valid hex")
  }

  #[test]
  fn parses_timestamp_and_every_v1() {
    let parsed = header(&format!("t=1754956800,v1={GOOD_MAC_HEX},v1={GOOD_MAC_HEX}"));
    assert_eq!(parsed.timestamp, FIXTURE_TIMESTAMP);
    assert_eq!(parsed.signatures.len(), 2);
    assert_eq!(parsed.signatures[0], mac_bytes(GOOD_MAC_HEX));
  }

  #[test]
  fn v0_is_ignored_so_it_cannot_be_used_to_downgrade() {
    assert_eq!(
      parse_signature_header(&format!("t=1754956800,v0={GOOD_MAC_HEX}")),
      None
    );
    let parsed = header(&format!(
      "t=1754956800,v0=deadbeef,v1={GOOD_MAC_HEX},v2=cafe"
    ));
    assert_eq!(parsed.signatures.len(), 1);
    assert_eq!(parsed.signatures[0], mac_bytes(GOOD_MAC_HEX));
  }

  #[test]
  fn malformed_headers_do_not_parse() {
    assert_eq!(parse_signature_header(""), None);
    assert_eq!(parse_signature_header("garbage"), None);
    assert_eq!(parse_signature_header(&format!("v1={GOOD_MAC_HEX}")), None);
    assert_eq!(parse_signature_header("t=notanumber,v1=aabb"), None);
    assert_eq!(parse_signature_header("t=1754956800,v1=notahexvalue"), None);
    assert_eq!(parse_signature_header("t=1754956800,v1=abc"), None);
  }

  #[test]
  fn good_signature_matches_and_tampering_does_not() {
    let parsed = header(&format!("t=1754956800,v1={GOOD_MAC_HEX}"));
    assert!(signature_matches(&mac_bytes(GOOD_MAC_HEX), &parsed));

    // A tampered body, or a MAC computed under a different secret, both
    // surface here identically: a different MAC than the header carries.
    let mut other = mac_bytes(GOOD_MAC_HEX);
    other[0] ^= 0x01;
    assert!(!signature_matches(&other, &parsed));
    assert!(!signature_matches(&[], &parsed));
    assert!(!signature_matches(&other[..16], &parsed));
  }

  #[test]
  fn one_matching_signature_is_enough_during_a_secret_roll() {
    let parsed = header(&format!("t=1754956800,v1=00112233,v1={GOOD_MAC_HEX}"));
    assert!(signature_matches(&mac_bytes(GOOD_MAC_HEX), &parsed));
  }

  #[test]
  fn stale_and_future_timestamps_fall_outside_the_window() {
    let now = FIXTURE_TIMESTAMP;
    assert!(timestamp_within_tolerance(now, now, 300));
    assert!(timestamp_within_tolerance(now - 300, now, 300));
    assert!(timestamp_within_tolerance(now + 300, now, 300));
    assert!(!timestamp_within_tolerance(now - 301, now, 300));
    assert!(!timestamp_within_tolerance(now + 301, now, 300));
    assert!(!timestamp_within_tolerance(now - 86_400, now, 300));
  }

  #[test]
  fn signed_payload_is_timestamp_dot_raw_body() {
    // Byte-for-byte the string the openssl vector above was taken over --
    // if this ever drifts, GOOD_MAC_HEX stops meaning what it claims to.
    assert_eq!(
      signed_payload(FIXTURE_TIMESTAMP, FIXTURE_BODY),
      br#"1754956800.{"id":"evt_test","type":"ping"}"#.to_vec()
    );
  }
}
