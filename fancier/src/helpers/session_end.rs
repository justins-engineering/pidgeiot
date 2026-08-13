use crate::helpers::remove_session_cookie;
use crate::models::AuthState;
use crate::{LocalSession, Session};
use dioxus::prelude::*;

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
