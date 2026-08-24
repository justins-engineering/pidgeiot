//! VIES lookups -- the European Commission's VAT Information Exchange
//! System, which is the only authority on whether an EU VAT number is a
//! live registration.
//!
//! # What this service actually is
//!
//! VIES is a fan-out: the Commission's endpoint forwards each query to the
//! member state that issued the number, and answers with whatever that
//! member state said. There is no key, no registration and no quota
//! published; there is also no availability guarantee, and individual
//! member states go down routinely and independently of each other. The
//! Commission publishes their live status at `/check-status`, and it is
//! normal to find one or two of the twenty-eight listed `Unavailable` at
//! any given moment.
//!
//! # The consequence for us, which is the whole design
//!
//! A service that is down some of the time cannot be allowed to decide
//! whether a customer may save their own VAT number. So a lookup has
//! exactly three outcomes ([`capsules::ViesOutcome`]) and only ONE of them
//! is bad news for the caller:
//!
//! - `Valid` / `Invalid` -- VIES answered. `valid` is a real boolean in a
//!   real response body.
//! - `Unknown` -- everything else. Transport failure, non-2xx, a body we
//!   could not parse, or an `errorWrappers` envelope (of which
//!   `MS_UNAVAILABLE` is the common one). None of these is evidence about
//!   the number, so none of them refuses anything: the number is stored
//!   `pending` and the scheduled sweep asks again.
//!
//! The distinction is real in the wire protocol, not something we infer.
//! Observed against the live service: with Germany listed `Unavailable`,
//! three checksum-valid German numbers returned
//! `{"actionSucceed":false,"errorWrappers":[{"error":"MS_UNAVAILABLE"}]}`
//! while a checksum-BROKEN German number returned a flat
//! `{"valid":false,...}` in the same second -- VIES rejects a malformed
//! number itself, without asking the member state. That is also why
//! `capsules::tax_id` does not reimplement national check digits.
//!
//! Note the shape of the trap this avoids: `MS_UNAVAILABLE` arrives with
//! HTTP **200**, so a client that only checks the status code and then
//! reads `valid` as `false`-by-absence would tell a customer their genuine
//! VAT number is invalid every time their own tax authority has a bad
//! afternoon.

use capsules::{EuVatId, ViesOutcome};
use serde::Deserialize;
use worker::{Fetch, Method, Request, RequestInit, console_error, console_log};

/// The Commission's REST endpoint. The service also exposes a SOAP
/// interface at `/services/checkVatService` and a GET form at
/// `/rest-api/ms/{country}/vat/{number}`; this POST form is the documented
/// REST one and the only one used here.
const VIES_CHECK_URL: &str = "https://ec.europa.eu/taxation_customs/vies/rest-api/check-vat-number";

/// A definitive answer. `valid` is present on exactly the responses that
/// are answers; an error envelope has no `valid` field at all, which is
/// what makes `Option` the right type here rather than a defaulted bool.
#[derive(Deserialize)]
struct ViesAnswer {
  valid: Option<bool>,
  #[serde(default)]
  #[serde(rename = "errorWrappers")]
  error_wrappers: Vec<ViesErrorWrapper>,
}

#[derive(Deserialize)]
struct ViesErrorWrapper {
  error: Option<String>,
}

/// Asks VIES about one number.
///
/// Never returns an `Err`: every failure is an [`ViesOutcome::Unknown`],
/// because the caller's decision is the same for all of them and giving it
/// a `Result` would invite someone to `?` a transport blip into a refusal.
///
/// Logs name the country and the length only, never the number -- a VAT id
/// is public information, but it identifies a customer, and logs are where
/// customer identifiers pile up without anyone choosing that they should.
pub async fn check_vat(vat: &EuVatId) -> ViesOutcome {
  let body = serde_json::json!({
    "countryCode": vat.country,
    "vatNumber": vat.number,
  })
  .to_string();

  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  if init
    .headers
    .set("Content-Type", "application/json")
    .is_err()
  {
    console_error!("VIES lookup: failed to set Content-Type header");
    return ViesOutcome::Unknown;
  }
  if init.headers.set("Accept", "application/json").is_err() {
    console_error!("VIES lookup: failed to set Accept header");
    return ViesOutcome::Unknown;
  }
  init.body = Some(body.into());

  let Ok(req) = Request::new_with_init(VIES_CHECK_URL, &init) else {
    console_error!("VIES lookup: failed to build request");
    return ViesOutcome::Unknown;
  };

  let mut resp = match Fetch::Request(req).send().await {
    Ok(resp) => resp,
    Err(e) => {
      console_error!("VIES lookup unreachable for {}: {e}", vat.country);
      return ViesOutcome::Unknown;
    }
  };

  let status = resp.status_code();
  let Ok(text) = resp.text().await else {
    console_error!(
      "VIES lookup for {} returned an unreadable body",
      vat.country
    );
    return ViesOutcome::Unknown;
  };

  // A 400 is VIES refusing the REQUEST (a missing field), not answering
  // about the number, so it is no more an "invalid" than a timeout is.
  if !(200..300).contains(&status) {
    console_error!("VIES lookup for {} returned HTTP {status}", vat.country);
    return ViesOutcome::Unknown;
  }

  let Ok(answer) = serde_json::from_str::<ViesAnswer>(&text) else {
    console_error!(
      "VIES lookup for {} returned an unparseable body",
      vat.country
    );
    return ViesOutcome::Unknown;
  };

  if let Some(first) = answer.error_wrappers.first() {
    // MS_UNAVAILABLE, TIMEOUT, SERVICE_UNAVAILABLE, the concurrency
    // limits -- all arrive with HTTP 200 and all mean "no answer". The
    // code is safe to log: it describes the service, not the customer.
    let code = first.error.as_deref().unwrap_or("unspecified");
    console_log!(
      "VIES gave no answer for {} ({} chars): {code}",
      vat.country,
      vat.number.chars().count()
    );
    return ViesOutcome::Unknown;
  }

  match answer.valid {
    Some(true) => ViesOutcome::Valid,
    Some(false) => ViesOutcome::Invalid,
    // A 2xx with neither `valid` nor an error envelope is a shape we do
    // not recognize. Guessing either way would be worse than admitting it.
    None => {
      console_error!(
        "VIES lookup for {} answered 200 with no verdict field",
        vat.country
      );
      ViesOutcome::Unknown
    }
  }
}
