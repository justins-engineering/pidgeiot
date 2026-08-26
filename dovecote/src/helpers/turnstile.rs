//! Cloudflare Turnstile verification for `POST /contact`.
//!
//! The widget on fancier's contact form hands the browser a one-time
//! token; this asks Cloudflare's siteverify endpoint whether that token is
//! genuine before an enquiry is stored. The secret is a Worker secret,
//! never a var: it is the one value that lets anyone mint passing tokens
//! for our site key.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::future::{Either, select};
use serde::Deserialize;
use worker::{
  AbortController, Delay, Env, Fetch, Method, Request, RequestInit, console_error, console_log,
};

/// Name of the Worker secret. Unset means the route runs without
/// verification -- see `verify_turnstile` for why that is the chosen
/// failure direction.
pub const TURNSTILE_SECRET: &str = "TURNSTILE_SECRET";

const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// Siteverify normally answers in well under a second. Long enough to ride
/// out a slow edge, short enough that a Cloudflare outage answers the
/// sender with a 503 instead of a request that hangs.
const SITEVERIFY_TIMEOUT: Duration = Duration::from_secs(5);

/// Cloudflare's published ceiling on a response token's length. Anything
/// longer is not a token and is refused without a round trip.
const MAX_TOKEN_BYTES: usize = 2048;

/// Logged once per isolate rather than once per submission: a missing
/// secret is a deployment gap that needs to be visible, not a log flood.
static MISSING_SECRET_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnstileVerdict {
  /// The token verified, or there is no secret to verify against.
  Passed,
  /// No token, or one Cloudflare did not issue for this site.
  Refused,
  /// Siteverify could not be asked, or rejected our own secret. Not
  /// evidence about the sender either way.
  Unavailable,
}

#[derive(Deserialize)]
struct SiteverifyAnswer {
  success: bool,
  #[serde(default, rename = "error-codes")]
  error_codes: Vec<String>,
}

/// Asks siteverify about one token.
///
/// Fails **open** when no secret is configured. A contact form that
/// refuses every visitor because a secret was never set loses real
/// enquiries to a misconfiguration nobody is looking at, and the
/// honeypot, fill-time floor and per-IP limiter still stand. Every other
/// failure is a refusal or a 503, never a pass: once a secret exists the
/// only way past this check is a token Cloudflare vouches for.
///
/// Logs never carry the token or the address -- the token is single-use
/// and worthless once spent, but a log full of visitor IPs is a habit
/// worth not starting. Cloudflare's error codes are a fixed vocabulary,
/// not sender data, so they are logged whole.
pub async fn verify_turnstile(
  env: &Env,
  token: Option<&str>,
  remote_ip: Option<&str>,
) -> TurnstileVerdict {
  let secret = env
    .secret(TURNSTILE_SECRET)
    .ok()
    .map(|s| s.to_string())
    .filter(|s| !s.trim().is_empty());
  let Some(secret) = secret else {
    if !MISSING_SECRET_LOGGED.swap(true, Ordering::Relaxed) {
      console_error!("contact: {TURNSTILE_SECRET} is not set; accepting submissions unverified");
    }
    return TurnstileVerdict::Passed;
  };

  let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
    return TurnstileVerdict::Refused;
  };
  if token.len() > MAX_TOKEN_BYTES {
    return TurnstileVerdict::Refused;
  }

  let mut body = serde_json::json!({ "secret": secret, "response": token });
  if let Some(ip) = remote_ip.map(str::trim).filter(|ip| !ip.is_empty()) {
    body["remoteip"] = serde_json::Value::String(ip.to_string());
  }

  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  if init
    .headers
    .set("Content-Type", "application/json")
    .is_err()
    || init.headers.set("Accept", "application/json").is_err()
  {
    console_error!("contact: failed to set Turnstile siteverify headers");
    return TurnstileVerdict::Unavailable;
  }
  init.body = Some(body.to_string().into());

  let Ok(req) = Request::new_with_init(SITEVERIFY_URL, &init) else {
    console_error!("contact: failed to build Turnstile siteverify request");
    return TurnstileVerdict::Unavailable;
  };

  // The fetch is raced against a timer rather than left to the platform's
  // own (much longer) fetch timeout, and aborted when the timer wins so
  // the connection is not left open behind a response already sent.
  let controller = AbortController::default();
  let signal = controller.signal();
  let fetch = Fetch::Request(req);
  let send = pin!(fetch.send_with_signal(&signal));
  let deadline = pin!(Delay::from(SITEVERIFY_TIMEOUT));
  let mut resp = match select(send, deadline).await {
    Either::Left((Ok(resp), _)) => resp,
    Either::Left((Err(e), _)) => {
      console_error!("contact: Turnstile siteverify unreachable: {e}");
      return TurnstileVerdict::Unavailable;
    }
    Either::Right(((), _)) => {
      controller.abort();
      console_error!("contact: Turnstile siteverify timed out");
      return TurnstileVerdict::Unavailable;
    }
  };

  let status = resp.status_code();
  let Ok(text) = resp.text().await else {
    console_error!("contact: Turnstile siteverify returned an unreadable body");
    return TurnstileVerdict::Unavailable;
  };
  if !(200..300).contains(&status) {
    console_error!("contact: Turnstile siteverify returned HTTP {status}");
    return TurnstileVerdict::Unavailable;
  }
  let Ok(answer) = serde_json::from_str::<SiteverifyAnswer>(&text) else {
    console_error!("contact: Turnstile siteverify returned an unparseable body");
    return TurnstileVerdict::Unavailable;
  };

  if answer.success {
    return TurnstileVerdict::Passed;
  }

  let codes = answer.error_codes.join(",");
  // A rejected secret is our misconfiguration, not the visitor's failed
  // challenge; refusing them with 403 would tell them to try again when
  // no retry can succeed.
  if answer
    .error_codes
    .iter()
    .any(|c| c == "missing-input-secret" || c == "invalid-input-secret")
  {
    console_error!("contact: Turnstile rejected our secret ({codes}); check {TURNSTILE_SECRET}");
    return TurnstileVerdict::Unavailable;
  }
  console_log!("contact: Turnstile refused a token ({codes})");
  TurnstileVerdict::Refused
}
