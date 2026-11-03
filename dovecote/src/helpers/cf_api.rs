//! The one budget every Cloudflare API call of the rules shares: uploads, deletes, re-tags and the
//! daily sweep all spend the same token's 1,200 calls per 5 minutes. A call over budget answers
//! [`CF_API_BUSY`] whether this Worker's limiter or Cloudflare refused it, and is never retried
//! on its own: the person retries, a minute later.
#![expect(dead_code, reason = "its caller is the rule upload path")]

use worker::{Cors, Env, Response, console_error};

/// The rate-limiter binding, configured beside its sizing in `wrangler.toml`.
pub const CF_API_LIMITER: &str = "CF_API_LIMITER";

/// The binding's window, and so the wait a refused caller is told.
pub const CF_API_WINDOW_SECS: u32 = 60;

/// The limiter's one key: the budget is the token's, not any account's.
const CF_API_KEY: &str = "cf-api";

/// The answer to a Cloudflare API call refused for budget. Never 401, which signs the dashboard
/// out.
pub const CF_API_BUSY: (u16, &str) = (
  429,
  "Too Many Requests: rule deploys are busy, retry in a minute",
);

/// The refusal for a limiter verdict: `Some(false)` is over budget, `None` a limiter fault. A
/// fault goes ahead, since Cloudflare's own 429 still answers busy behind it, and a limiter
/// outage must not stop every deploy.
pub fn limiter_refusal(allowed: Option<bool>) -> Option<(u16, &'static str)> {
  (allowed == Some(false)).then_some(CF_API_BUSY)
}

/// The refusal for a Cloudflare API status: its 429 is the same budget seen from the other side.
/// Every other status is the caller's to read.
pub fn cloudflare_refusal(status: u16) -> Option<(u16, &'static str)> {
  (status == 429).then_some(CF_API_BUSY)
}

/// Spends one call of the budget, or answers busy.
pub async fn cf_api_permit(env: &Env, cors: &Cors) -> Option<worker::Result<Response>> {
  let allowed = match env.rate_limiter(CF_API_LIMITER) {
    Ok(limiter) => match limiter.limit(CF_API_KEY.to_string()).await {
      Ok(outcome) => Some(outcome.success),
      Err(e) => {
        console_error!("cf-api: limiter check failed (going ahead): {e}");
        None
      }
    },
    Err(e) => {
      console_error!("cf-api: {CF_API_LIMITER} binding unavailable (going ahead): {e}");
      None
    }
  };
  limiter_refusal(allowed).map(|_| busy_response(cors))
}

/// [`CF_API_BUSY`] as a response, with `Retry-After` set to the limiter's window.
pub fn busy_response(cors: &Cors) -> worker::Result<Response> {
  let mut response = Response::error(CF_API_BUSY.1, CF_API_BUSY.0).unwrap();
  if response
    .headers_mut()
    .set("Retry-After", &CF_API_WINDOW_SECS.to_string())
    .is_err()
  {
    console_error!("cf-api: failed to set Retry-After on a 429");
  }
  response.with_cors(cors)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_limiter_and_cloudflare_refuse_with_one_answer() {
    assert_eq!(limiter_refusal(Some(false)), Some(CF_API_BUSY));
    assert_eq!(cloudflare_refusal(429), Some(CF_API_BUSY));
  }

  // A limiter fault must not stop deploys: Cloudflare's own 429 still answers busy behind it.
  #[test]
  fn an_allowed_call_or_a_limiter_fault_goes_ahead() {
    assert_eq!(limiter_refusal(Some(true)), None);
    assert_eq!(limiter_refusal(None), None);
  }

  // Every other Cloudflare answer is the upload path's to read, never "busy".
  #[test]
  fn only_a_cloudflare_429_reads_as_busy() {
    for status in [200, 400, 401, 403, 404, 409, 413, 500, 502, 503] {
      assert_eq!(cloudflare_refusal(status), None, "{status}");
    }
  }

  // 401 signs the dashboard out, and the wait the text names is the limiter's window.
  #[test]
  fn busy_is_429_retry_in_a_minute() {
    assert_eq!(CF_API_BUSY.0, 429);
    assert!(CF_API_BUSY.1.contains("retry in a minute"));
    assert_eq!(CF_API_WINDOW_SECS, 60);
  }
}
