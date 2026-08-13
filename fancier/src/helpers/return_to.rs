use crate::local_storage;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

// Namespaced and versioned like every other client-only key (see
// local_storage.rs).
const RETURN_TO_KEY: &str = "pidgeiot.return_to.v1";

// Signing back in has to survive a full page load -- Kratos posts the login
// form as a real form submission and 303s back into the app -- so the
// interrupted destination cannot live in a Signal. It is scoped tightly
// instead: a destination is only honoured for a sign-in that follows soon
// after the sign-out that recorded it. Anything older belongs to an attempt
// the user has since abandoned, and dropping them on a page they no longer
// remember asking for is worse than the dashboard.
const MAX_AGE_SECONDS: i64 = 30 * 60;

#[derive(Serialize, Deserialize)]
struct ReturnTo {
  path: String,
  saved_at: i64,
}

/// Records where the user was when their session ended.
///
/// Reads the address bar rather than taking the router's current `Route` as
/// an argument: the only way to get that inside the auth layout is
/// `use_route`, which subscribes the layout to route changes and would
/// re-render the whole signed-in subtree on every navigation just to keep a
/// value that is read at most once.
pub fn stash_return_to() {
  let Some(window) = web_sys::window() else {
    return;
  };
  let location = window.location();
  let Ok(path) = location.pathname() else {
    return;
  };
  let search = location.search().unwrap_or_default();

  local_storage::save(
    RETURN_TO_KEY,
    &ReturnTo {
      path: path + &search,
      saved_at: OffsetDateTime::now_utc().unix_timestamp(),
    },
  );
}

/// Reads and clears the stashed destination. Clearing on read is what keeps
/// one interrupted visit from redirecting every later sign-in.
pub fn take_return_to() -> Option<String> {
  let stashed: ReturnTo = local_storage::load(RETURN_TO_KEY)?;
  local_storage::remove(RETURN_TO_KEY);

  let age = OffsetDateTime::now_utc().unix_timestamp() - stashed.saved_at;
  // A negative age means the clock moved backwards between writing and
  // reading; treat that as unusable rather than as arbitrarily fresh.
  (0..=MAX_AGE_SECONDS).contains(&age).then_some(stashed.path)
}

pub fn clear_return_to() {
  local_storage::remove(RETURN_TO_KEY);
}
