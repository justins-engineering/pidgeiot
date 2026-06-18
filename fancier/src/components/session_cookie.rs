use crate::helpers::{remove_session_cookie, write_session_hint_cookie};
use crate::models::AuthState;
use crate::{Configuration, Create, Route, Session};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::to_session;

#[component]
pub fn SetSessionCookie(state: bool) -> Element {
  let mut session = use_context::<Session>();
  let nav = use_navigator();

  use_future(move || async move {
    if state {
      // state = true: Kratos redirect after successful login or verification.
      // We must now ask the Kratos backend to validate the secure HttpOnly cookie
      // and give us the session metadata (like expiry).
      let config = Configuration::create();

      match to_session(&config, None, None, None).await {
        Ok(kratos_session) => {
          if let Some(expires_at) = kratos_session.expires_at {
            write_session_hint_cookie(&expires_at);
            session.state.set(AuthState::Authenticated);
            nav.replace(Route::Dashboard {});
          } else {
            error!("Kratos returned a valid session, but missing expiry.");
            session.state.set(AuthState::Unauthenticated);
            nav.replace(Route::Index {});
          }
        }
        Err(err) => {
          // This handles edge cases where the redirect happened, but the
          // HttpOnly cookie was dropped or invalid.
          error!("Kratos session validation failed post-redirect: {err:?}");
          session.state.set(AuthState::Unauthenticated);
          nav.replace(Route::Index {});
        }
      }
    } else {
      // state = false: Kratos redirect after logout.
      // Tear down the UI hint and global state.
      remove_session_cookie();
      session.state.set(AuthState::Unauthenticated);
      nav.replace(Route::Index {});
    }
  });

  rsx! {
    div { class: "flex items-center justify-center min-h-screen",
      p { "Synchronizing secure session..." }
    }
  }
}
