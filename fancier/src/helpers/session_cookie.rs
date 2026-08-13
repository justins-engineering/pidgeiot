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

/// Seconds left on the session hint cookie, or `None` when no readable,
/// parseable hint cookie is present.
///
/// This is readable at all only because the hint cookie is written by
/// `write_session_hint_cookie` above rather than by Kratos: its value is
/// the RFC 3339 expiry Kratos reported at sign-in. The session cookie that
/// actually authenticates a request is `HttpOnly` and invisible to script,
/// so the hint is the only way to know when a session is due to lapse
/// without asking the network.
///
/// Takes the furthest-out expiry when several cookies match, matching the
/// browser's own behaviour of sending all of them: while any one is still
/// live the session it stands for may be too.
pub fn session_hint_seconds_remaining() -> Option<i64> {
  let html_document = html_document!(window!());
  let cookie_string = get_cookies!(html_document);

  cookie_string
    .split(';')
    .filter(|cookie| cookie.contains(SESSION_COOKIE_NAME))
    .filter_map(|cookie| {
      let expiry = cookie.split('=').next_back()?;
      let timestamp: Result<OffsetDateTime, time::error::Parse> =
        OffsetDateTime::parse(expiry.trim(), &Rfc3339);

      match timestamp {
        Ok(dt) => Some((dt - OffsetDateTime::now_utc()).whole_seconds()),
        Err(err) => {
          error!("Failed to parse cookie expiry: {err:?}");
          None
        }
      }
    })
    .max()
}

pub async fn session_cookie_valid() -> bool {
  session_hint_seconds_remaining().is_some_and(|remaining| remaining > 0)
}
