use crate::{Configuration, Create, Route, SESSION_COOKIE_NAME, Session};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::to_session;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

macro_rules! window {
  () => {
    web_sys::window().expect("Could not access window")
  };
}

macro_rules! html_document {
  ($window:expr) => {
    web_sys::wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlDocument>(
      $window
        .document()
        .expect("Could not access window document"),
    )
    .expect("Could not access HTMLDocument")
  };
}

macro_rules! get_cookies {
  ($html_document:expr) => {
    $html_document
      .cookie()
      .expect("Could not access HTMLDocument cookies")
  };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuthState {
  Pending,
  Authenticated,
  Unauthenticated,
}

impl AuthState {
  #[inline]
  pub fn is_authenticated(&self) -> bool {
    matches!(self, AuthState::Authenticated)
  }
}

const COOKIE_STR_LEN: usize = SESSION_COOKIE_NAME.len()
  // u32::MAX.to_string().len()
  + 10
  + "2025-08-05T17:14:07.837312011Z".len()
  + "=; path=/; SameSite=Strict; max-age=; Secure".len();

// Separate the pure WASM/DOM logic into synchronous helpers
fn write_session_hint_cookie(expires_at: &str) {
  let timestamp: Result<OffsetDateTime, time::error::Parse> =
    OffsetDateTime::parse(expires_at, &Rfc3339);

  if let Ok(dt) = timestamp {
    let duration = (dt - OffsetDateTime::now_utc()).whole_seconds();
    let max_age = if duration > 0 { duration } else { 0 };

    let mut cookie_str = String::with_capacity(COOKIE_STR_LEN);
    cookie_str.push_str(SESSION_COOKIE_NAME);
    cookie_str.push('=');
    cookie_str.push_str(expires_at);
    cookie_str.push_str("; path=/; SameSite=Strict; max-age=");
    cookie_str.push_str(&max_age.to_string());
    cookie_str.push_str("; Secure");

    let html_document = html_document!(window!());
    if html_document.set_cookie(&cookie_str).is_err() {
      error!("Failed to set session hint cookie");
    }
  } else {
    error!("Failed to parse session expiry timestamp");
  }
}

fn remove_session_cookie() {
  let html_document = html_document!(window!());
  let cookie_str = format!(
    "{}=0; path=/; SameSite=Strict; expires=Thu, 01 Jan 1970 00:00:00 UTC; Secure",
    SESSION_COOKIE_NAME
  );

  if html_document.set_cookie(&cookie_str).is_err() {
    error!("Failed to remove session hint cookie");
  }
}

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

pub async fn session_cookie_valid() -> bool {
  let html_document = html_document!(window!());
  let cookie_string = get_cookies!(html_document);
  let cookies = cookie_string.split(';');

  for cookie in cookies {
    if cookie.contains(SESSION_COOKIE_NAME) {
      let mut c = cookie.split('=');
      if let Some(expiry) = c.next_back() {
        let timestamp: Result<OffsetDateTime, time::error::Parse> =
          OffsetDateTime::parse(expiry.trim(), &Rfc3339);

        match timestamp {
          Ok(dt) => {
            // Return true immediately if we find a valid, unexpired cookie
            if OffsetDateTime::now_utc() < dt {
              return true;
            }
          }
          Err(err) => error!("Failed to parse cookie expiry: {err:?}"),
        }
      }
    }
  }

  // Default to false if the loop finishes without returning true
  false
}
