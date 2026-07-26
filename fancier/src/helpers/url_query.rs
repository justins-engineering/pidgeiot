//! Read query parameters straight from the browser's address bar.
//!
//! Why this exists (task #43, prod signup outage): with SSG prerendering
//! (task #42), every statically-rendered page embeds the route it was
//! prerendered AS into its hydration payload — always the bare route with no
//! query string (`/registration?` → `flow: None`, `/session/local?state=false`
//! — booleans prerender as their `Default`). On a full-page load the client
//! restores THAT serialized route during hydration instead of re-parsing
//! `window.location` (confirmed by decoding `initial_dioxus_hydration_data`
//! in the built `registration/index.html`: it contains the literal string
//! "/registration?"), so a route prop like `flow: Option<String>` arrives as
//! `None` even when the address bar plainly says `?flow=<id>`. Kratos's
//! browser self-service flows are driven entirely by full-page 303 redirects
//! carrying `?flow=` (and `/session/local?state=`), so in the SSG-served prod
//! artifact every such handoff silently lost its parameter: registration
//! looped back to a brand-new empty form with no error, and the post-login
//! handoff ran the `state=false` branch and tore the session hint down.
//! `Route::from_str` parses these URLs fine (see the tests in lib.rs) — the
//! router isn't the problem, the hydrated initial route is. Components that
//! consume query params on entry must treat the real URL as the source of
//! truth and use the route prop only as a fallback (the native/SSG server
//! build has no `window`; there the prop is all we have, and the prerender
//! renders the loading state regardless).

/// The value of `key` in the current `window.location` query string, if any.
///
/// Returns `None` on the non-wasm (SSG prerender) build, where there is no
/// browser location to consult.
pub fn url_query_param(key: &str) -> Option<String> {
  #[cfg(target_arch = "wasm32")]
  {
    let search = web_sys::window()?.location().search().ok()?;
    query_param_from_search(&search, key)
  }
  #[cfg(not(target_arch = "wasm32"))]
  {
    let _ = key;
    None
  }
}

/// Pure lookup of `key` in a `location.search`-shaped string (`"?a=1&b=2"`).
///
/// Values are returned verbatim (no percent-decoding): every consumer reads
/// Kratos flow ids (UUIDs) or `state=true|false`, none of which are ever
/// percent-encoded.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn query_param_from_search(search: &str, key: &str) -> Option<String> {
  search
    .strip_prefix('?')
    .unwrap_or(search)
    .split('&')
    .find_map(|pair| {
      let (k, v) = pair.split_once('=')?;
      (k == key && !v.is_empty()).then(|| v.to_owned())
    })
}

#[cfg(test)]
mod tests {
  use super::query_param_from_search;

  #[test]
  fn finds_flow_id() {
    assert_eq!(
      query_param_from_search("?flow=83a8270e-77c4-4a88-a908-939b181fbb5f", "flow"),
      Some("83a8270e-77c4-4a88-a908-939b181fbb5f".to_owned())
    );
  }

  #[test]
  fn finds_key_among_multiple_pairs() {
    assert_eq!(
      query_param_from_search("?return_to=%2Fdashboard&flow=abc", "flow"),
      Some("abc".to_owned())
    );
    assert_eq!(
      query_param_from_search("?state=true", "state"),
      Some("true".to_owned())
    );
  }

  #[test]
  fn missing_or_empty_yields_none() {
    assert_eq!(query_param_from_search("", "flow"), None);
    assert_eq!(query_param_from_search("?", "flow"), None);
    assert_eq!(query_param_from_search("?flow=", "flow"), None);
    assert_eq!(query_param_from_search("?other=x", "flow"), None);
    // key-only pair, no '='
    assert_eq!(query_param_from_search("?flow", "flow"), None);
  }

  #[test]
  fn does_not_match_key_prefixes() {
    assert_eq!(query_param_from_search("?flowx=abc", "flow"), None);
    assert_eq!(query_param_from_search("?reflow=abc", "flow"), None);
  }
}
