use crate::SESSION_COOKIE_NAME;
use crate::helpers::browser::{get_cookies, html_document, window};
use dioxus::logger::tracing::error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const COOKIE_STR_LEN: usize = SESSION_COOKIE_NAME.len()
  + 10
  + "2025-08-05T17:14:07.837312011Z".len()
  + "=; path=/; SameSite=Strict; max-age=; Secure".len();

// Separate the pure WASM/DOM logic into synchronous helpers
pub fn write_session_hint_cookie(expires_at: &str) {
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

pub fn remove_session_cookie() {
  let html_document = html_document!(window!());
  let cookie_str = format!(
    "{}=0; path=/; SameSite=Strict; expires=Thu, 01 Jan 1970 00:00:00 UTC; Secure",
    SESSION_COOKIE_NAME
  );

  if html_document.set_cookie(&cookie_str).is_err() {
    error!("Failed to remove session hint cookie");
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
