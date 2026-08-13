use crate::helpers::{remove_session_cookie, session_hint_seconds_remaining, sleep_ms};
use crate::models::AuthState;
use crate::{LocalSession, Session};
use dioxus::prelude::*;

// Longest gap between hint-cookie checks while signed in. One `setTimeout`
// armed for the whole remaining session would be cheaper still, but
// browsers count those in monotonic time: a laptop that suspends past the
// deadline wakes with the timer unfired and keeps showing an authenticated
// shell for as long as it slept. Re-reading the cookie at most once a
// minute bounds that, and costs a string split and a clock comparison --
// no request, which is the point. Background tabs are throttled to about
// this cadence anyway.
const MAX_TICK_MS: i64 = 60_000;

/// Tears this tab down to a signed-out state after the server stopped
/// recognising the session, and records that it happened involuntarily so
/// the login view can explain why the user is looking at it.
///
/// Deliberately a no-op unless a session was actually established. Two
/// callers depend on that: a visitor who was never signed in must not be
/// told they were "signed out" of a session they never had, and a page
/// that fires several requests at once must not run the teardown once per
/// 401 that comes back.
pub fn session_lost() {
  let Some(mut session) = try_consume_context::<Session>() else {
    return;
  };

  if !(session.state)().is_authenticated() {
    return;
  }

  // The cached flocks/pigeons/alerts belong to the identity that just went
  // away. Leaving them behind would keep rendering one account's fleet to
  // whoever signs in next on this tab.
  if let Some(mut local) = try_consume_context::<LocalSession>() {
    local.flocks.write().clear();
    local.pigeons.write().clear();
    local.alerts.write().clear();
  }

  remove_session_cookie();
  session.signed_out.set(true);
  session.state.set(AuthState::Unauthenticated);
}

/// Signs the tab out the moment its session lapses, rather than leaving an
/// idle one looking at data that is no longer being refreshed.
///
/// Deadline-driven rather than polled: the hint cookie already carries the
/// expiry, so this sleeps toward it and sends no traffic at all. That does
/// leave one gap on purpose -- a session revoked server-side before its
/// expiry is not visible from here, and the tab stays optimistic until its
/// next request, which `session_lost` then catches. Closing that gap would
/// mean a periodic `whoami` on every page, and the pages where an idle tab
/// actually shows live data (the graph and log viewers) are already polling
/// dovecote and so already reach `session_lost` on their own.
///
/// Caller runs this only while authenticated, so a visitor who is not
/// signed in arms no timer at all.
pub async fn watch_session_expiry() {
  loop {
    match session_hint_seconds_remaining() {
      Some(remaining) if remaining > 0 => {
        // Overshoot the deadline by a second so the wake lands past it
        // rather than just short of it and having to sleep again.
        let sleep_for = (remaining * 1_000 + 1_000).min(MAX_TICK_MS);
        sleep_ms(sleep_for as i32).await;
      }
      // Expired, or the cookie is gone -- signed out in another tab, or
      // dropped by the browser at its own max-age.
      _ => {
        session_lost();
        return;
      }
    }
  }
}
