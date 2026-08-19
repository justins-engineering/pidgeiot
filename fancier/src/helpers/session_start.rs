use crate::helpers::{url_query_param, write_session_hint_cookie};
use crate::models::AuthState;
use crate::{Configuration, Create, Session};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::to_session;

/// Brings this tab into its signed-in state from a session Kratos has
/// already established for the browser, and reports whether it worked.
///
/// The cookie that actually authenticates a request is Kratos's, `HttpOnly`
/// and invisible to script, so the only way to learn a session exists (and
/// when it lapses) is to ask -- hence the `whoami` round trip. What comes
/// back is recorded the one way the rest of the app reads it: the hint
/// cookie carrying the expiry, plus the auth signal. Every entry into a
/// signed-in state goes through here, so a session picked up mid-flow is
/// indistinguishable from one that came from the login form.
pub async fn adopt_kratos_session(mut session: Session) -> bool {
  let config = Configuration::create();

  match to_session(&config, None, None, None).await {
    Ok(kratos_session) => {
      let Some(expires_at) = kratos_session.expires_at else {
        // Nothing to write a hint from, and an authenticated shell with no
        // deadline would never notice the session ending.
        error!("Kratos returned a valid session, but missing expiry.");
        return false;
      };

      write_session_hint_cookie(&expires_at);
      // A fresh session is no longer a lapsed one, so the login view stops
      // explaining a sign-out that has been undone.
      session.signed_out.set(false);
      session.state.set(AuthState::Authenticated);
      true
    }
    Err(err) => {
      // The redirect happened but the cookie was dropped, rejected, or
      // never set.
      error!("Kratos session validation failed: {err:?}");
      false
    }
  }
}

/// Whether the address bar shows Kratos handing this browser off to the
/// settings UI mid-flow.
///
/// Completing account recovery does not come back through
/// `/session/local?state=true` the way login and verification do: Kratos
/// issues the session itself and 303s straight to the settings UI with a
/// flow id, so the user can set a new password while the session is still
/// privileged. The SPA therefore arrives at a guarded route holding a real
/// session it has never heard of, and without this would bounce the user
/// back out to the login page it just recovered them past.
///
/// Narrow on purpose: a settings flow id in the URL is something only
/// Kratos puts there, and every other route keeps deciding it is signed out
/// from the hint cookie alone, at no network cost.
pub fn kratos_settings_handoff() -> bool {
  let Some(window) = web_sys::window() else {
    return false;
  };
  let Ok(path) = window.location().pathname() else {
    return false;
  };

  is_settings_handoff(&path, url_query_param("flow").as_deref())
}

/// Pure half of `kratos_settings_handoff`, split out to be testable off a
/// browser. Both slash forms count: wrangler serves the prerendered page at
/// `/settings/` and 307s the bare form to it.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn is_settings_handoff(path: &str, flow: Option<&str>) -> bool {
  matches!(path.trim_end_matches('/'), "/settings") && flow.is_some()
}

#[cfg(test)]
mod tests {
  use super::is_settings_handoff;

  #[test]
  fn settings_with_a_flow_id_is_a_handoff() {
    assert!(is_settings_handoff("/settings", Some("abc")));
    assert!(is_settings_handoff("/settings/", Some("abc")));
  }

  #[test]
  fn settings_without_a_flow_id_is_not() {
    assert!(!is_settings_handoff("/settings", None));
  }

  #[test]
  fn other_routes_are_not_handoffs() {
    // A flow id on a public Kratos route belongs to a flow that has not
    // established anything yet.
    assert!(!is_settings_handoff("/login", Some("abc")));
    assert!(!is_settings_handoff("/recovery", Some("abc")));
    assert!(!is_settings_handoff("/", Some("abc")));
    // Nothing that merely starts the same way counts either.
    assert!(!is_settings_handoff("/settings-export", Some("abc")));
  }
}
