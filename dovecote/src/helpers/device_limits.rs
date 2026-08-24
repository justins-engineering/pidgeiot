use std::cell::RefCell;
use std::collections::HashMap;

use worker::{Cors, Date, Env, Request, Response, console_error};

use super::coap_service::is_allowed_coap_service_ip;

/// One rate-limiter binding together with the window it is configured
/// with. The two travel as a pair because the window is what a limited
/// caller has to wait out, and so what `Retry-After` must say -- keeping
/// them together is what stops the two drifting apart.
pub struct DeviceLimiter {
  binding: &'static str,
  window_secs: u32,
}

/// Per-pigeon throttle on `GET /device/pigeons/:id/shadow`. Sizing lives
/// beside the binding in `wrangler.toml`.
pub const DEVICE_SHADOW_LIMITER: DeviceLimiter = DeviceLimiter {
  binding: "DEVICE_SHADOW_LIMITER",
  window_secs: 60,
};

/// Per-pigeon throttle on `GET /device/pigeons/:id/firmware`. On the
/// short window rather than the long one, for a reason measured against
/// the real binding rather than assumed -- see `wrangler.toml`.
pub const DEVICE_FIRMWARE_LIMITER: DeviceLimiter = DeviceLimiter {
  binding: "DEVICE_FIRMWARE_LIMITER",
  window_secs: 10,
};

/// Per-source-address budget for device requests that fail authentication.
/// Only failures are counted, so it never sees healthy traffic.
const DEVICE_AUTH_FAIL_LIMITER: DeviceLimiter = DeviceLimiter {
  binding: "DEVICE_AUTH_FAIL_LIMITER",
  window_secs: 60,
};

/// How long an address stays short-circuited once it has exhausted its
/// failed-auth budget. Deliberately that limiter's own window, so this
/// memory can never outlive the counter that justified it.
const PENALTY_MS: u64 = DEVICE_AUTH_FAIL_LIMITER.window_secs as u64 * 1000;

/// Ceiling on remembered addresses. A penalty box this full is itself the
/// signature of a spread-out attempt, where per-address memory has stopped
/// paying for itself; forgetting everything costs one extra verify round
/// trip per address and is what stops the map growing without bound.
const MAX_PENALTY_ENTRIES: usize = 256;

thread_local! {
  /// Isolate-local record of addresses already found to be over their
  /// failed-auth budget.
  ///
  /// The rate-limiter binding has no check-without-counting call: every
  /// `limit()` consumes budget. Consulting it before proxying would
  /// therefore charge the healthy devices this limiter exists to protect,
  /// while consulting it only afterwards would leave every repeat attempt
  /// still paying for the Durable Object round trip it is meant to
  /// prevent. This closes that gap: the shared counter decides, and its
  /// verdict is remembered here so the next attempt from the same address
  /// is refused before any round trip happens.
  ///
  /// Per-isolate rather than shared, and that is enough: the counter
  /// behind it is colo-wide, so a fresh isolate's first `limit()` call
  /// already comes back refused and it memorises the verdict after a
  /// single round trip.
  static AUTH_FAILURE_PENALTIES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// The per-pigeon throttle on one non-billable device surface.
///
/// Keyed by pigeon id rather than by address because that is the identity
/// the cost actually follows: one pigeon is one Durable Object, and a
/// device's address is neither stable (carrier NAT) nor unique (the CoAP
/// terminator fronts a whole fleet from one egress address). Requests
/// proxied by the terminator carry the pigeon id in their path exactly as
/// direct HTTPS requests do, so they are covered on the same key.
///
/// Fails open. Counters are approximate and roughly per-colo, which
/// devices being colo-sticky makes acceptable, and which makes this a
/// backstop on runaway volume rather than a precise gate. A limiter fault
/// taking a fleet offline would be a far worse failure than a window of
/// unthrottled polling.
pub async fn device_surface_limit(
  env: &Env,
  limiter: &DeviceLimiter,
  pigeon_id: &str,
  cors: &Cors,
) -> Option<worker::Result<Response>> {
  let binding = limiter.binding;
  let handle = match env.rate_limiter(binding) {
    Ok(handle) => handle,
    Err(e) => {
      console_error!("device limits: {binding} binding unavailable (failing open): {e}");
      return None;
    }
  };

  match handle.limit(pigeon_id.to_string()).await {
    Ok(outcome) if !outcome.success => Some(rate_limited_response(cors, limiter.window_secs)),
    Ok(_) => None,
    Err(e) => {
      console_error!("device limits: {binding} check failed (failing open): {e}");
      None
    }
  }
}

/// One device request's standing against the failed-auth budget, resolved
/// once from the request so it survives the request being consumed by the
/// proxy call in between the two uses.
pub struct DeviceAuthGuard {
  /// `None` means this request is not subject to the budget at all:
  /// either it has no connecting address to key on, or it came from the
  /// CoAP terminator (see [`DeviceAuthGuard::new`]).
  address: Option<String>,
}

impl DeviceAuthGuard {
  /// Resolves the address this request will be counted against, if any.
  ///
  /// The CoAP terminator is exempt, and has to be. Every CoAP device in
  /// the fleet reaches these routes through the terminator's single
  /// egress address, so one device looping on a rotated token would spend
  /// the shared budget and lock every other CoAP device out behind it --
  /// an address budget aimed at one misbehaving device would take down
  /// the entire transport. The exemption reuses the address allowlist
  /// that already establishes which addresses are the terminator
  /// (`COAP_SERVICE_ALLOWED_IPS`, the same anchor gating the internal PSK
  /// route) rather than trusting a header the caller controls.
  ///
  /// Exempting it costs little: the terminator only forwards a request
  /// after completing a DTLS or TLS handshake against a pre-shared key it
  /// resolved for that specific pigeon, so nothing reaches these routes
  /// through it without already holding a per-pigeon credential, and it
  /// applies its own per-source connection admission on the transport
  /// side. The per-pigeon limiters above still apply to everything it
  /// forwards.
  pub fn new(env: &Env, req: &Request) -> DeviceAuthGuard {
    if is_allowed_coap_service_ip(env, req) {
      return DeviceAuthGuard { address: None };
    }
    DeviceAuthGuard {
      address: req.headers().get("CF-Connecting-IP").ok().flatten(),
    }
  }

  /// Whether this address is already known to be over its budget, and so
  /// should be refused before the Durable Object is touched. Reads
  /// isolate-local state only: no binding call, no network, and nothing
  /// charged to an address that has never failed.
  pub fn blocked_response(&self, cors: &Cors) -> Option<worker::Result<Response>> {
    let address = self.address.as_deref()?;
    let now_ms = Date::now().as_millis();
    let blocked = AUTH_FAILURE_PENALTIES
      .with(|penalties| penalty_active_at(&mut penalties.borrow_mut(), address, now_ms));
    blocked.then(|| rate_limited_response(cors, DEVICE_AUTH_FAIL_LIMITER.window_secs))
  }

  /// Charges one authentication failure to this address.
  ///
  /// Call sites invoke this only once the Durable Object has actually
  /// rejected the credential, which is what keeps a healthy device from
  /// ever touching this counter. Fails open like the surface limiters:
  /// a limiter fault means attempts go uncounted, never that a device is
  /// refused.
  pub async fn note_failure(&self, env: &Env) {
    let Some(address) = self.address.as_deref() else {
      return;
    };

    let binding = DEVICE_AUTH_FAIL_LIMITER.binding;
    let handle = match env.rate_limiter(binding) {
      Ok(handle) => handle,
      Err(e) => {
        console_error!("device limits: {binding} binding unavailable (failing open): {e}");
        return;
      }
    };

    match handle.limit(address.to_string()).await {
      Ok(outcome) if !outcome.success => {
        console_error!(
          "device limits: device auth failures from {address} over budget; refusing further attempts from it for {}s without a verify round trip",
          DEVICE_AUTH_FAIL_LIMITER.window_secs
        );
        let now_ms = Date::now().as_millis();
        AUTH_FAILURE_PENALTIES.with(|penalties| {
          record_penalty_at(&mut penalties.borrow_mut(), address, now_ms);
        });
      }
      Ok(_) => {}
      Err(e) => console_error!("device limits: {binding} check failed (failing open): {e}"),
    }
  }
}

/// The one response every device limiter answers with.
///
/// 429 and never 401: the dashboard treats 401 as "session gone" and
/// signs the tab out, and on the device side a 401 is the status that
/// means the credential itself is wrong. `Retry-After` names the window
/// rather than a computed remainder, because the counters are approximate
/// and a caller that waits the full window is certainly clear of it. No
/// device in the fleet reads it today (the Zephyr client surfaces neither
/// response headers nor the status code to its callers, which is also why
/// a limited request is indistinguishable from any other transport
/// failure there, and is retried on the next cycle rather than escalated);
/// it is here for correctness, for anything else that speaks HTTP, and for
/// whoever is debugging this with curl.
fn rate_limited_response(cors: &Cors, window_secs: u32) -> worker::Result<Response> {
  let mut response = Response::error("Too Many Requests", 429).unwrap();
  if response
    .headers_mut()
    .set("Retry-After", &window_secs.to_string())
    .is_err()
  {
    console_error!("device limits: failed to set Retry-After on a 429");
  }
  response.with_cors(cors)
}

/// Expired entries are dropped as they are read, which keeps the common
/// case (an address that served its window and came back healthy) from
/// needing a sweep of its own.
fn penalty_active_at(penalties: &mut HashMap<String, u64>, address: &str, now_ms: u64) -> bool {
  match penalties.get(address) {
    Some(&until_ms) if until_ms > now_ms => true,
    Some(_) => {
      penalties.remove(address);
      false
    }
    None => false,
  }
}

fn record_penalty_at(penalties: &mut HashMap<String, u64>, address: &str, now_ms: u64) {
  if penalties.len() >= MAX_PENALTY_ENTRIES {
    penalties.retain(|_, until_ms| *until_ms > now_ms);
  }
  // Still full after dropping everything expired, so every entry is a
  // live offender and there is no cheaper eviction to make.
  if penalties.len() >= MAX_PENALTY_ENTRIES {
    penalties.clear();
  }
  penalties.insert(address.to_string(), now_ms + PENALTY_MS);
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use super::{MAX_PENALTY_ENTRIES, PENALTY_MS, penalty_active_at, record_penalty_at};

  #[test]
  fn an_address_that_never_failed_is_never_blocked() {
    let mut penalties = HashMap::new();
    assert!(!penalty_active_at(&mut penalties, "203.0.113.7", 1_000));
    assert!(penalties.is_empty());
  }

  #[test]
  fn a_recorded_penalty_blocks_until_its_window_elapses() {
    let mut penalties = HashMap::new();
    record_penalty_at(&mut penalties, "203.0.113.7", 1_000);

    assert!(penalty_active_at(&mut penalties, "203.0.113.7", 1_000));
    assert!(penalty_active_at(
      &mut penalties,
      "203.0.113.7",
      1_000 + PENALTY_MS - 1
    ));
    assert!(!penalty_active_at(
      &mut penalties,
      "203.0.113.7",
      1_000 + PENALTY_MS
    ));
  }

  #[test]
  fn one_addresss_penalty_never_blocks_another() {
    let mut penalties = HashMap::new();
    record_penalty_at(&mut penalties, "203.0.113.7", 1_000);
    assert!(!penalty_active_at(&mut penalties, "203.0.113.8", 1_000));
  }

  #[test]
  fn reading_an_expired_entry_forgets_it() {
    let mut penalties = HashMap::new();
    record_penalty_at(&mut penalties, "203.0.113.7", 1_000);
    penalty_active_at(&mut penalties, "203.0.113.7", 1_000 + PENALTY_MS);
    assert!(penalties.is_empty());
  }

  #[test]
  fn a_repeat_failure_extends_the_window_rather_than_stacking_entries() {
    let mut penalties = HashMap::new();
    record_penalty_at(&mut penalties, "203.0.113.7", 1_000);
    record_penalty_at(&mut penalties, "203.0.113.7", 30_000);

    assert_eq!(penalties.len(), 1);
    assert!(penalty_active_at(
      &mut penalties,
      "203.0.113.7",
      30_000 + PENALTY_MS - 1
    ));
  }

  #[test]
  fn expired_entries_are_swept_before_the_map_is_allowed_to_grow() {
    let mut penalties = HashMap::new();
    for i in 0..MAX_PENALTY_ENTRIES {
      record_penalty_at(&mut penalties, &format!("198.51.100.{i}"), 1_000);
    }
    assert_eq!(penalties.len(), MAX_PENALTY_ENTRIES);

    // Every existing entry has expired by now, so the newcomer displaces
    // them rather than growing the map past its ceiling.
    record_penalty_at(&mut penalties, "203.0.113.7", 1_000 + PENALTY_MS);
    assert_eq!(penalties.len(), 1);
    assert!(penalty_active_at(
      &mut penalties,
      "203.0.113.7",
      1_000 + PENALTY_MS
    ));
  }

  #[test]
  fn a_map_full_of_live_offenders_is_cleared_rather_than_grown() {
    let mut penalties = HashMap::new();
    for i in 0..MAX_PENALTY_ENTRIES {
      record_penalty_at(&mut penalties, &format!("198.51.100.{i}"), 1_000);
    }

    record_penalty_at(&mut penalties, "203.0.113.7", 1_001);
    assert_eq!(penalties.len(), 1);
    assert!(penalty_active_at(&mut penalties, "203.0.113.7", 1_001));
  }
}
