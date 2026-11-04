//! The one budget every Cloudflare API call of the rules shares: uploads, deletes, re-tags and the
//! daily sweep all spend the owner user's 1,200 calls per 5 minutes, which the dashboard and
//! wrangler share, and over which Cloudflare blocks every one of that user's calls for five
//! minutes. A refused call answers [`CF_API_BUSY`] whether this Worker's limiter or Cloudflare
//! refused it, and is never retried on its own: the person retries.

use worker::{Cors, Env, Response, console_error};

use super::device_limits::rate_limited_response;

/// The rate-limiter binding, configured beside its sizing in `wrangler.toml`.
pub const CF_API_LIMITER: &str = "CF_API_LIMITER";

/// The binding's window, and so the wait a caller refused by the limiter is told.
pub const CF_API_WINDOW_SECS: u32 = 10;

/// The wait told after a Cloudflare 429 without a readable `retry-after`: its documented block.
const CLOUDFLARE_BLOCK_SECS: u32 = 300;

/// The shortest wait told after a Cloudflare 429: a sooner retry only spends another call
/// against the same block.
const CLOUDFLARE_MIN_WAIT_SECS: u32 = 60;

/// The limiter's one key: the budget is the owner user's, not any account's.
const CF_API_KEY: &str = "cf-api";

/// The answer to a Cloudflare API call refused for budget. Never 401, which signs the dashboard
/// out. The text holds for both waits, the limiter's seconds and Cloudflare's minutes.
pub const CF_API_BUSY: (u16, &str) = (
  429,
  "Too Many Requests: rule deploys are busy, retry in a few minutes",
);

/// Whether a limiter verdict refuses: `Some(false)` is over budget, `None` a limiter fault. A
/// fault goes ahead, since Cloudflare's own 429 still answers busy behind it, and a limiter
/// outage must not stop every deploy.
pub fn over_budget(allowed: Option<bool>) -> bool {
  allowed == Some(false)
}

/// The wait to tell for a Cloudflare API answer, `None` unless it is a 429: never shorter than
/// Cloudflare's own `retry-after` (seconds), so a person retrying as told is not refused again.
/// Every other status is the caller's to read.
pub fn cloudflare_wait(status: u16, retry_after: Option<&str>) -> Option<u32> {
  (status == 429).then(|| {
    retry_after
      .and_then(|s| s.trim().parse::<u32>().ok())
      .map_or(CLOUDFLARE_BLOCK_SECS, |secs| {
        secs.max(CLOUDFLARE_MIN_WAIT_SECS)
      })
  })
}

/// Spends one call of the budget and says whether to go ahead; a limiter fault goes ahead (see
/// [`over_budget`]). Call it last, immediately before the Cloudflare call and after every local
/// refusal, since every check counts. Only a person's action is refusable, once, at its first
/// call; a follow-up cleanup (a dry run's delete, a deploy's delete of the old bundle) calls it
/// too, so it counts, and goes ahead whatever it answers, so nothing is orphaned.
pub async fn cf_api_allowed(env: &Env) -> bool {
  let allowed = match env.rate_limiter(CF_API_LIMITER) {
    Ok(limiter) => match limiter.limit(CF_API_KEY.to_string()).await {
      Ok(outcome) => Some(outcome.success),
      Err(e) => {
        console_error!("cf-api: limiter check failed (failing open): {e}");
        None
      }
    },
    Err(e) => {
      console_error!("cf-api: {CF_API_LIMITER} binding unavailable (failing open): {e}");
      None
    }
  };
  !over_budget(allowed)
}

/// [`CF_API_BUSY`] as a response, with `Retry-After` set to `wait_secs`: [`CF_API_WINDOW_SECS`]
/// for the limiter, [`cloudflare_wait`] for Cloudflare.
pub fn busy_response(cors: &Cors, wait_secs: u32) -> worker::Result<Response> {
  rate_limited_response(cors, wait_secs, CF_API_BUSY.1)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn an_allowed_call_or_a_limiter_fault_goes_ahead() {
    assert!(over_budget(Some(false)));
    assert!(!over_budget(Some(true)));
    assert!(!over_budget(None));
  }

  // Every other Cloudflare answer is the upload path's to read, never "busy".
  #[test]
  fn only_a_cloudflare_429_reads_as_busy() {
    for status in [200, 400, 401, 403, 404, 409, 413, 500, 502, 503] {
      assert_eq!(cloudflare_wait(status, Some("300")), None, "{status}");
    }
  }

  #[test]
  fn a_cloudflare_429_never_tells_a_shorter_wait_than_cloudflare_did() {
    for secs in [0, 1, 59, 60, 61, 300, 301, 3600] {
      let wait = cloudflare_wait(429, Some(&secs.to_string())).unwrap();
      assert!(wait >= secs && wait >= CLOUDFLARE_MIN_WAIT_SECS, "{secs}");
    }
    assert_eq!(cloudflare_wait(429, Some(" 120 ")), Some(120));
  }

  #[test]
  fn a_cloudflare_429_without_a_readable_wait_tells_the_whole_block() {
    for header in [
      None,
      Some(""),
      Some("soon"),
      Some("-1"),
      Some("Wed, 21 Oct 2026 07:28:00 GMT"),
    ] {
      assert_eq!(cloudflare_wait(429, header), Some(300), "{header:?}");
    }
  }
}
