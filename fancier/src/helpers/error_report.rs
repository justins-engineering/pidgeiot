//! Client-side error capture: the Rust panic hook and the breadcrumb ring.
//!
//! On wasm the panic strategy is abort -- no unwinding, no `catch_unwind`,
//! no backtrace at any price, and after a panic the module is dead: no
//! Dioxus render, no `spawn_local`, nothing async. Everything here
//! therefore happens synchronously inside the panic hook, which runs
//! before the abort while Rust state is still readable, and delivery is
//! `navigator.sendBeacon` with a `text/plain` body -- fire-and-forget, the
//! browser owns delivery after the call returns, and `text/plain` is
//! CORS-safelisted so no preflight (which `sendBeacon` cannot negotiate)
//! ever drops it. The report carries no identity: dovecote's text/plain
//! branch is anonymous by construction.
//!
//! Classes this file cannot see -- a wasm that never boots, JS exceptions
//! -- belong to `assets/error-shim.js`, installed pre-boot. The two
//! coordinate through `window.__pidgeiot_err`: the hook flips `panicked`
//! (so the shim treats anything JS throws after the abort -- including
//! the abort's own `RuntimeError: unreachable` -- as already-reported
//! noise), reveals the shim's crash panel itself rather than waiting for
//! that error to maybe surface, and stashes the serialized report so the
//! panel's "tell us what happened" note can resend it with the user's
//! words attached.
//!
//! Everything is compiled out on non-wasm targets: the SSG prerender runs
//! this crate as a native binary, where no `window` exists and a panic
//! hook would be noise.

#[cfg(target_arch = "wasm32")]
mod imp {
  use std::cell::RefCell;
  use std::collections::VecDeque;

  use capsules::{
    Breadcrumb, BreadcrumbKind, ErrorKind, ErrorReport, MAX_ERROR_BREADCRUMB_DETAIL_BYTES,
    MAX_ERROR_BREADCRUMBS, MAX_ERROR_MESSAGE_BYTES, MAX_ERROR_REPORTS_PER_PAGE, SessionKind,
    normalize_route, truncate_bytes,
  };
  use wasm_bindgen::{JsCast, JsValue};

  thread_local! {
    static BREADCRUMBS: RefCell<VecDeque<(f64, BreadcrumbKind, String)>> =
      RefCell::new(VecDeque::with_capacity(MAX_ERROR_BREADCRUMBS));
  }

  fn push_crumb(kind: BreadcrumbKind, detail: String) {
    let detail = truncate_bytes(&detail, MAX_ERROR_BREADCRUMB_DETAIL_BYTES).to_string();
    BREADCRUMBS.with(|ring| {
      let mut ring = ring.borrow_mut();
      // A rerender without a route change would otherwise double-record
      // the same navigation back to back.
      if ring
        .back()
        .is_some_and(|(_, k, d)| *k == kind && *d == detail)
      {
        return;
      }
      if ring.len() >= MAX_ERROR_BREADCRUMBS {
        ring.pop_front();
      }
      ring.push_back((js_sys::Date::now(), kind, detail));
    });
  }

  /// One line per API round trip, from the single funnel every request
  /// already passes through (`api/helpers.rs::dispatch`). Shape only --
  /// method, route template, status -- never bodies or query params.
  /// `None` status means the fetch promise itself failed: "500" and "the
  /// request never completed" are different facts worth keeping apart.
  pub fn breadcrumb_api(method: &str, path: &str, status: Option<u16>) {
    let outcome = match status {
      Some(s) => s.to_string(),
      None => "network failure".to_string(),
    };
    push_crumb(
      BreadcrumbKind::Api,
      format!("{method} {} -> {outcome}", normalize_route(path)),
    );
  }

  /// One line per route change, fed from the component that already runs
  /// `use_route` on every navigation.
  pub fn breadcrumb_nav(path: &str) {
    push_crumb(BreadcrumbKind::Nav, normalize_route(path));
  }

  fn snapshot_breadcrumbs() -> Vec<Breadcrumb> {
    let now = js_sys::Date::now();
    BREADCRUMBS.with(|ring| {
      ring
        .borrow()
        .iter()
        .map(|(at, kind, detail)| Breadcrumb {
          age_ms: (now - at).max(0.0).min(u32::MAX as f64) as u32,
          kind: *kind,
          detail: detail.clone(),
        })
        .collect()
    })
  }

  /// The breadcrumb trail + build hash as the feedback form's attachable
  /// diagnostics block -- the same shape-only content an automatic report
  /// carries, formatted for a human reading an ops email.
  pub fn diagnostics_string() -> Option<String> {
    let crumbs = snapshot_breadcrumbs();
    let build = window_string_global("__pidgeiot_build").unwrap_or_else(|| "unknown".to_string());
    let mut out = format!("build: {build}");
    for c in &crumbs {
      out.push_str(&format!(
        "\n-{}ms {} {}",
        c.age_ms,
        match c.kind {
          BreadcrumbKind::Nav => "nav",
          BreadcrumbKind::Api => "api",
          BreadcrumbKind::Ui => "ui",
        },
        c.detail
      ));
    }
    Some(truncate_bytes(&out, capsules::MAX_FEEDBACK_DIAGNOSTICS_BYTES).to_string())
  }

  fn window_string_global(name: &str) -> Option<String> {
    let window = web_sys::window()?;
    js_sys::Reflect::get(&window, &JsValue::from_str(name))
      .ok()?
      .as_string()
  }

  fn err_marker() -> Option<js_sys::Object> {
    let window = web_sys::window()?;
    js_sys::Reflect::get(&window, &JsValue::from_str("__pidgeiot_err"))
      .ok()?
      .dyn_into::<js_sys::Object>()
      .ok()
  }

  fn marker_number(marker: &js_sys::Object, key: &str) -> f64 {
    js_sys::Reflect::get(marker, &JsValue::from_str(key))
      .ok()
      .and_then(|v| v.as_f64())
      .unwrap_or(0.0)
  }

  fn set_marker(marker: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(marker, &JsValue::from_str(key), value);
  }

  /// The hint cookie fancier writes at sign-in is the only session signal
  /// readable here (the Kratos cookie is HttpOnly) -- one bit, never an
  /// identity. Deliberately avoids the panicking `window!` macros: this
  /// runs inside the panic hook, where a second panic is an instant abort
  /// with the report lost.
  fn session_kind_hint() -> SessionKind {
    let signed_in = web_sys::window()
      .and_then(|w| w.document())
      .and_then(|d| d.dyn_into::<web_sys::HtmlDocument>().ok())
      .and_then(|d| d.cookie().ok())
      .is_some_and(|cookies| {
        cookies.split(';').any(|c| {
          c.trim_start()
            .starts_with(crate::config::SESSION_COOKIE_NAME)
        })
      });
    if signed_in {
      SessionKind::SignedIn
    } else {
      SessionKind::Anonymous
    }
  }

  /// Installs the hook that turns a production panic from perfect silence
  /// into one stored report. Replaces the default hook, which on this
  /// target writes to a stderr that goes nowhere.
  pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(report_panic));
  }

  fn report_panic(info: &std::panic::PanicHookInfo) {
    let message = info
      .payload()
      .downcast_ref::<&str>()
      .map(|s| s.to_string())
      .or_else(|| info.payload().downcast_ref::<String>().cloned())
      .unwrap_or_else(|| "panic with non-string payload".to_string());
    let location = info
      .location()
      .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));

    let Some(window) = web_sys::window() else {
      return;
    };
    let route = window
      .location()
      .pathname()
      .map(|p| normalize_route(&p))
      .unwrap_or_else(|_| "/".to_string());
    let user_agent = window.navigator().user_agent().ok();

    let report = ErrorReport {
      kind: ErrorKind::RustPanic,
      message: truncate_bytes(&message, MAX_ERROR_MESSAGE_BYTES).to_string(),
      location,
      stack: None,
      route,
      build: window_string_global("__pidgeiot_build"),
      user_agent,
      breadcrumbs: snapshot_breadcrumbs(),
      session_kind: session_kind_hint(),
      occurred_at_ms: js_sys::Date::now().max(0.0) as u64,
      client_event_id: Some(uuid::Uuid::now_v7()),
    };
    let Ok(body) = serde_json::to_string(&report) else {
      return;
    };

    // The marker is flipped BEFORE the beacon: any JS error thrown after
    // the abort must be suppressed as downstream noise, and the serialized
    // report is stashed so the panel's note flow can attach the user's
    // words to this exact report (joined by client_event_id).
    let marker = err_marker();
    if let Some(marker) = &marker {
      set_marker(marker, "panicked", &JsValue::TRUE);
      set_marker(marker, "last_report", &JsValue::from_str(&body));
    }

    // Shared page-load budget with the JS shim. A panic can't loop (the
    // module is dead), but a page cycling through JS errors and a late
    // panic still stays bounded. Budget exhaustion skips only the beacon,
    // never the panel below -- the user still needs a true screen.
    let sent = marker
      .as_ref()
      .map(|m| marker_number(m, "sent"))
      .unwrap_or(0.0);
    if sent < MAX_ERROR_REPORTS_PER_PAGE as f64 {
      if let Some(marker) = &marker {
        set_marker(marker, "sent", &JsValue::from_f64(sent + 1.0));
      }
      let url = format!("{}/errors", crate::config::API_HOST);
      let _ = window
        .navigator()
        .send_beacon_with_opt_str(&url, Some(&body));
    }

    // Reveal the crash panel from here, synchronously, while the hook is
    // guaranteed a chance to run -- whether and when the abort's thrown
    // `RuntimeError: unreachable` surfaces to window.onerror depends on
    // which glue path invoked the wasm, so the shim's handlers are only a
    // backstop for the reveal, never the primary path.
    if let Some(marker) = &marker
      && let Ok(reveal) = js_sys::Reflect::get(marker, &JsValue::from_str("reveal"))
      && let Some(reveal) = reveal.dyn_ref::<js_sys::Function>()
    {
      let _ = reveal.call0(marker);
    }
  }

  /// Disarms the shim's boot watchdog -- proof Rust ran at all is exactly
  /// the signal a class-B (never-booted) report keys on the absence of.
  pub fn mark_booted() {
    if let Some(marker) = err_marker() {
      set_marker(&marker, "boot", &JsValue::TRUE);
    }
  }
}

/// The shim's link-scanner rule is a JS regex and the server's is Rust, so
/// nothing but running both over the same messages can keep them honest.
/// The pattern is read out of the file that actually ships rather than
/// restated here: a copy would agree with itself forever.
#[cfg(test)]
mod tests {
  use capsules::is_link_scanner_noise;
  use regex::Regex;

  const SHIM: &str = include_str!("../../assets/error-shim.js");

  fn shim_pattern() -> Regex {
    let after = SHIM
      .split_once("var LINK_SCANNER_NOISE =")
      .expect("the shim no longer defines LINK_SCANNER_NOISE")
      .1;
    let source = after
      .trim_start()
      .strip_prefix('/')
      .and_then(|rest| rest.split_once("/;"))
      .expect("LINK_SCANNER_NOISE is not a one-line regex literal")
      .0;
    Regex::new(source).expect("the shim's pattern does not mean the same thing in Rust")
  }

  // The production report, the variant most other sites see, and the
  // normalized form the server matches after its own redaction pass.
  const SCANNER: [&str; 4] = [
    "Object Not Found Matching Id:5, MethodName:simulateEvent, ParamCount:4",
    "Object Not Found Matching Id:1, MethodName:update, ParamCount:4",
    "Object Not Found Matching Id:<int>, MethodName:simulateEvent, ParamCount:<int>",
    "Uncaught 'Object Not Found Matching Id:2, MethodName:update, ParamCount:4'",
  ];

  const OURS: [&str; 4] = [
    "Object Not Found in the flock list",
    "Object Not Found Matching Id:5, MethodName:simulateEvent",
    "Object Not Found Matching Id:5, ParamCount:4",
    "called Option::unwrap() on a None value",
  ];

  #[test]
  fn the_shim_drops_exactly_what_the_server_folds() {
    let pattern = shim_pattern();
    for message in SCANNER {
      assert!(pattern.is_match(message), "the shim would send: {message}");
      assert!(
        is_link_scanner_noise(message),
        "the server would mail: {message}"
      );
    }
    for message in OURS {
      assert!(
        !pattern.is_match(message),
        "the shim would drop our own: {message}"
      );
      assert!(
        !is_link_scanner_noise(message),
        "the server would silence our own: {message}"
      );
    }
  }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
  pub fn breadcrumb_api(_method: &str, _path: &str, _status: Option<u16>) {}
  pub fn breadcrumb_nav(_path: &str) {}
  pub fn diagnostics_string() -> Option<String> {
    None
  }
  pub fn install_panic_hook() {}
  pub fn mark_booted() {}
}

pub use imp::*;
