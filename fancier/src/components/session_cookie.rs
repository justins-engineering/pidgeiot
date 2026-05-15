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

const COOKIE_STR_LEN: usize = SESSION_COOKIE_NAME.len()
  // u32::MAX.to_string().len()
  + 10
  + "2025-08-05T17:14:07.837312011Z".len()
  + "=; path=/; SameSite=Strict; max-age=; Secure".len();

fn remove_session_cookie() {
  let html_document = html_document!(window!());

  let mut cookie_str = String::with_capacity(COOKIE_STR_LEN);
  cookie_str.push_str(SESSION_COOKIE_NAME);
  cookie_str.push_str("=0; path=/; SameSite=Strict; expires=Thu, 01 Jan 1970 00:00:00 UTC; Secure");

  let new_cookie = html_document.set_cookie(&cookie_str);

  match new_cookie {
    Ok(_) => {}
    Err(_) => {
      error!("Failed to set cookie");
    }
  }
}

#[component]
pub fn SetSessionCookie(state: bool) -> Element {
  let html_document: web_sys::HtmlDocument = html_document!(window!());

  let create_flow: Resource<Result<_, ory_kratos_client_wasm::apis::Error<_>>> = use_resource(
    move || async move { to_session(&Configuration::create(), None, None, None).await },
  );

  use_effect(move || use_context::<Session>().state.set(state));

  if state {
    if let Some(Ok(session)) = &*create_flow.read()
      && let Some(expires_at) = &session.expires_at
    {
      let timestamp: Result<OffsetDateTime, time::error::Parse> =
        OffsetDateTime::parse(expires_at, &Rfc3339);

      match timestamp {
        Ok(dt) => {
          let duration = (dt - OffsetDateTime::now_utc()).whole_seconds();
          let max_age = if duration > 0 { duration } else { 0 };

          let mut cookie_str = String::with_capacity(COOKIE_STR_LEN);
          cookie_str.push_str(SESSION_COOKIE_NAME);
          cookie_str.push('=');
          cookie_str.push_str(expires_at);
          cookie_str.push_str("; path=/; SameSite=Strict; max-age=");
          cookie_str.push_str(&max_age.to_string());
          cookie_str.push_str("; Secure");

          let new_cookie = html_document.set_cookie(&cookie_str);

          match new_cookie {
            Ok(_) => {
              navigator().replace(Route::Dashboard {});
            }
            Err(_) => {
              error!("Failed to set cookie");
              navigator().replace(Route::Index {});
            }
          }
        }
        Err(err) => {
          error!("{err:?}");
          navigator().replace(Route::Index {});
        }
      }
    };
  } else {
    remove_session_cookie();
    navigator().replace(Route::Index {});
  }
  rsx!()
}

pub async fn session_cookie_valid() {
  let html_document = html_document!(window!());
  let cookie_string = get_cookies!(html_document);
  let cookies = cookie_string.split(';');
  let mut valid = use_context::<crate::Session>().state;

  for cookie in cookies {
    if cookie.contains(SESSION_COOKIE_NAME) {
      let mut c = cookie.split('=');
      if let Some(expiry) = c.next_back() {
        let timestamp: Result<OffsetDateTime, time::error::Parse> =
          OffsetDateTime::parse(expiry.trim(), &Rfc3339);

        match timestamp {
          Ok(dt) => {
            if OffsetDateTime::now_utc() < dt {
              valid.set(true);
              return;
            }
          }
          Err(err) => error!("{err:?}"),
        }
      }
    }
  }

  valid.set(false);
}
