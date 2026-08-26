// Pre-boot error capture for the failure classes Rust can never see: a
// wasm bundle that fails to load or instantiate (no Rust ever runs), and
// plain JS exceptions. Loaded as a static script before the wasm loader
// tag (same slot as theme-init.js), so it survives whatever happens to
// the module. It also decides, before the loader runs, whether this
// browser can instantiate the bundle at all: an old browser is not a
// crash, so it gets the static #app-unsupported notice instead of the
// crash panel and a counting-only report instead of an error. The Rust panic hook covers panics itself and coordinates
// through window.__pidgeiot_err: it sets `panicked` before the abort's
// RuntimeError reaches window.onerror, so the duplicate is suppressed and
// the static crash panel (#app-crash, injected at build time) is revealed
// exactly once, and it stashes its serialized report in `last_report` so
// the panel's note box can resend it with the user's words attached.
//
// Reports go to POST /errors as text/plain (CORS-safelisted: no preflight,
// nothing to negotiate) with credentials omitted -- the automatic class is
// anonymous on the wire everywhere we control the call. The identified
// note is the one deliberate exception: application/json WITH credentials,
// preflighted, because its whole point is "please contact me".
(function () {
  "use strict";
  if (window.__pidgeiot_err) return;

  var state = { panicked: false, sent: 0, last_report: null, unsupported: null };
  // `boot` is flipped by the Rust side once the app runs. The probe below
  // is a prediction; a booted module is the fact, so the setter retracts
  // the unsupported notice if the two ever disagree.
  var booted = false;
  Object.defineProperty(state, "boot", {
    enumerable: true,
    get: function () {
      return booted;
    },
    set: function (v) {
      booted = !!v;
      if (booted && state.hideUnsupported) state.hideUnsupported();
    },
  });
  window.__pidgeiot_err = state;
  // Exposed so the Rust panic hook can reveal the panel itself, pre-abort:
  // whether the abort's thrown RuntimeError surfaces to window.onerror
  // depends on which glue path invoked the wasm, so the handlers below
  // are only a backstop for the reveal, never the primary path.
  state.reveal = function () {
    revealCrash();
  };

  var seen = {};
  var revealed = false;

  function apiHost() {
    // Injected at build time next to __pidgeiot_build; absent in `dx
    // serve` dev, where the Rust hook still reports via its compile-time
    // API host and this shim just logs.
    return window.__pidgeiot_api || null;
  }

  function newId() {
    try {
      return crypto.randomUUID();
    } catch (e) {
      return null;
    }
  }

  // Whose script threw, by the origin of the location or, without one,
  // of the top stack frame. Mirrors capsules::error_source on the server:
  // wasm frames and blobs this origin minted are ours, extension schemes
  // are not, and no URL at all counts as ours so a failure in our own
  // glue is never dropped for want of a filename. Dropping here saves the
  // request; the server folds the same way for pages still running an
  // older copy of this file.
  var KNOWN_SCHEME =
    /^(https?|blob|wasm|chrome-extension|moz-extension|safari-extension|safari-web-extension|ms-browser-extension|webkit-masked-url):/i;
  var EXTENSION_SCHEME =
    /^(chrome-extension|moz-extension|safari-extension|safari-web-extension|ms-browser-extension|webkit-masked-url):/i;

  function firstUrl(text) {
    var tokens = String(text).split(/[\s()<>,]+/);
    for (var i = 0; i < tokens.length; i++) {
      if (KNOWN_SCHEME.test(tokens[i])) return tokens[i];
    }
    return null;
  }

  // V8 prefixes frames with "at "; Gecko and WebKit write fn@url:line:col,
  // so only the part after the last "@" is searched there, which also keeps
  // a URL quoted in the message line from passing as a frame.
  function topFrameUrl(stack) {
    var lines = String(stack).split("\n");
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i].replace(/^\s+/, "");
      var url = null;
      if (line.indexOf("at ") === 0) url = firstUrl(line.slice(3));
      else if (line.lastIndexOf("@") !== -1) url = firstUrl(line.slice(line.lastIndexOf("@") + 1));
      if (url) return url;
    }
    return null;
  }

  function originOf(url) {
    var m = /^([a-z][a-z0-9+.-]*):\/\/([^/?#]+)/i.exec(url);
    return m ? (m[1] + "://" + m[2]).toLowerCase() : null;
  }

  function isForeign(location, stack) {
    var url = (location && firstUrl(location)) || (stack && topFrameUrl(stack));
    if (!url) return false;
    if (/^wasm:/i.test(url)) return false;
    if (/^blob:/i.test(url)) url = url.slice(5);
    if (EXTENSION_SCHEME.test(url)) return true;
    var origin = originOf(url);
    if (!origin || !/^https?:/.test(origin)) return false;
    var own = (window.location.protocol + "//" + window.location.host).toLowerCase();
    return origin !== own;
  }

  function buildReport(kind, message, location, stack) {
    return {
      kind: kind,
      message: String(message).slice(0, 2048),
      location: location ? String(location).slice(0, 256) : null,
      // Query strings never leave the page; the server re-normalizes the
      // path into a route template regardless.
      route: window.location.pathname.split("?")[0],
      stack: stack ? String(stack).slice(0, 8192) : null,
      build: window.__pidgeiot_build || null,
      user_agent: navigator.userAgent,
      breadcrumbs: [],
      session_kind:
        document.cookie.indexOf("session_expiry") !== -1 ? "signed_in" : "anonymous",
      occurred_at_ms: Date.now(),
      client_event_id: newId(),
    };
  }

  function send(kind, message, location, stack) {
    if (!message || message === "Script error.") return; // cross-origin scripts carry no detail
    var extension = /(chrome|moz|safari)-extension:/;
    if (extension.test(location || "") || extension.test(stack || "")) return;
    if (isForeign(location, stack)) return;
    var key = kind + "|" + message + "|" + (location || "");
    if (seen[key]) return;
    seen[key] = true;
    if (state.sent >= 5) return;
    state.sent++;

    var report = buildReport(kind, message, location, stack);
    state.last_report = JSON.stringify(report);
    var api = apiHost();
    if (!api) {
      console.error("pidgeiot error-shim (no API host injected):", report);
      return;
    }
    try {
      // fetch keepalive rather than sendBeacon: same delivery guarantees
      // here (no dying wasm module to outrun), and it lets credentials be
      // omitted, which sendBeacon cannot.
      fetch(api + "/errors", {
        method: "POST",
        keepalive: true,
        credentials: "omit",
        headers: { "Content-Type": "text/plain;charset=UTF-8" },
        body: state.last_report,
      });
    } catch (e) {
      /* reporting must never throw */
    }
  }

  // What the release wasm needs beyond the MVP, from the bundle's own
  // target_features section (rustc's wasm32-unknown-unknown defaults).
  // Each probe is the smallest module that uses exactly one feature, so
  // WebAssembly.validate answers for that feature alone. The section
  // also lists atomics, which a non-shared memory never executes, and two
  // LLVM sub-features of bulk-memory and reference-types.
  var WASM_HEADER = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
  var WASM_PROBES = [
    // (func (param externref))
    ["reference-types", [1, 5, 1, 0x60, 1, 0x6f, 0]],
    // (func (result i32 i32))
    ["multivalue", [1, 6, 1, 0x60, 0, 2, 0x7f, 0x7f]],
    // (memory 1) (func (memory.copy (i32.const 0) (i32.const 0) (i32.const 0)))
    [
      "bulk-memory",
      [1, 4, 1, 0x60, 0, 0, 3, 2, 1, 0, 5, 3, 1, 0, 1,
       0x0a, 0x0e, 1, 0x0c, 0, 0x41, 0, 0x41, 0, 0x41, 0, 0xfc, 0x0a, 0, 0, 0x0b],
    ],
    // (func (drop (i32.extend8_s (i32.const 0))))
    ["sign-ext", [1, 4, 1, 0x60, 0, 0, 3, 2, 1, 0, 0x0a, 8, 1, 6, 0, 0x41, 0, 0xc0, 0x1a, 0x0b]],
    // (func (drop (i32.trunc_sat_f32_s (f32.const 0))))
    [
      "nontrapping-fptoint",
      [1, 4, 1, 0x60, 0, 0, 3, 2, 1, 0, 0x0a, 0x0c, 1, 0x0a, 0, 0x43, 0, 0, 0, 0, 0xfc, 0, 0x1a, 0x0b],
    ],
    // (global (export "g") (mut i32) (i32.const 0))
    ["mutable-globals", [6, 6, 1, 0x7f, 1, 0x41, 0, 0x0b, 7, 5, 1, 1, 0x67, 3, 0]],
  ];

  function missingWasmFeatures() {
    if (typeof WebAssembly !== "object" || typeof WebAssembly.validate !== "function")
      return "WebAssembly unavailable";
    var missing = [];
    for (var i = 0; i < WASM_PROBES.length; i++) {
      var ok = false;
      try {
        ok = WebAssembly.validate(new Uint8Array(WASM_HEADER.concat(WASM_PROBES[i][1])));
      } catch (e) {}
      if (!ok) missing.push(WASM_PROBES[i][0]);
    }
    return missing.length ? "missing wasm features: " + missing.join(", ") : null;
  }

  // Dismissal is remembered per tab: without wasm every navigation is a
  // full page load, and the notice only needs reading once.
  var DISMISSED_KEY = "pidgeiot.unsupported.dismissed";

  function revealUnsupported() {
    var notice = document.getElementById("app-unsupported");
    if (!notice) return;
    try {
      if (sessionStorage.getItem(DISMISSED_KEY)) return;
    } catch (e) {}
    function hide() {
      notice.hidden = true;
      document.removeEventListener("keydown", onKey);
    }
    function dismiss() {
      hide();
      try {
        sessionStorage.setItem(DISMISSED_KEY, "1");
      } catch (e) {}
    }
    function onKey(ev) {
      if (ev.key === "Escape" || ev.key === "Esc" || ev.keyCode === 27) dismiss();
    }
    var btn = document.getElementById("app-unsupported-dismiss");
    if (btn) btn.onclick = dismiss;
    document.addEventListener("keydown", onKey);
    state.hideUnsupported = hide;
    notice.hidden = false;
  }

  function whenDomReady(fn) {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", fn);
    else fn();
  }

  function revealCrash() {
    if (revealed) return;
    revealed = true;
    var panel = document.getElementById("app-crash");
    if (!panel) return;
    var idEl = document.getElementById("app-crash-id");
    if (idEl && state.last_report) {
      try {
        var id = JSON.parse(state.last_report).client_event_id;
        if (id) idEl.textContent = "Error ID: " + id;
      } catch (e) {}
    }
    panel.hidden = false;

    var reportBtn = document.getElementById("app-crash-report");
    var form = document.getElementById("app-crash-form");
    var sendBtn = document.getElementById("app-crash-send");
    var reloadBtn = document.getElementById("app-crash-reload");
    if (reloadBtn)
      reloadBtn.onclick = function () {
        window.location.reload();
      };
    if (reportBtn && form)
      reportBtn.onclick = function () {
        form.hidden = false;
        reportBtn.hidden = true;
        var note = document.getElementById("app-crash-note");
        if (note) note.focus();
      };
    if (sendBtn && form)
      sendBtn.onclick = function () {
        var note = document.getElementById("app-crash-note");
        var text = note && note.value ? note.value.trim() : "";
        if (!text || !state.last_report || !apiHost()) return;
        sendBtn.disabled = true;
        var body;
        try {
          body = JSON.stringify({ note: text.slice(0, 4000), report: JSON.parse(state.last_report) });
        } catch (e) {
          return;
        }
        fetch(apiHost() + "/errors", {
          method: "POST",
          keepalive: true,
          credentials: "include",
          headers: { "Content-Type": "application/json" },
          body: body,
        })
          .catch(function () {})
          .then(function () {
            form.hidden = true;
            var thanks = document.getElementById("app-crash-thanks");
            if (thanks) thanks.hidden = false;
          });
      };
  }

  // On a browser the probe rejected, the loader's failure and whatever
  // the glue throws after it are the expected consequence, not errors.
  window.addEventListener("error", function (ev) {
    if (state.unsupported) return;
    if (state.panicked) {
      // The abort after a Rust panic surfaces here as "RuntimeError:
      // unreachable" -- already reported by the hook, and everything JS
      // throws after the module died is downstream noise.
      revealCrash();
      return;
    }
    var loc = ev.filename ? ev.filename + ":" + ev.lineno + ":" + ev.colno : null;
    send("js_exception", ev.message, loc, ev.error && ev.error.stack);
  });

  window.addEventListener("unhandledrejection", function (ev) {
    if (state.unsupported) return;
    if (state.panicked) {
      revealCrash();
      return;
    }
    var r = ev.reason;
    send(
      "unhandled_rejection",
      r && (r.message || String(r)),
      null,
      r && r.stack
    );
  });

  // Boot watchdog for the white-screen class where no Rust ever runs to
  // report anything. Gated on evidence, not wall clock alone: it only
  // fires once the wasm resource demonstrably finished downloading (a 404
  // or instantiation failure completes the fetch; a slow link does not),
  // so a user on 2G never gets a crash panel over a page that was about
  // to work. Gives up silently after ~2 minutes of incomplete download.
  var watchdogStart = Date.now();
  function watchdog() {
    if (state.boot || state.panicked || state.unsupported) return;
    var wasmDone = false;
    try {
      var entries = performance.getEntriesByType("resource");
      for (var i = 0; i < entries.length; i++) {
        if (/\.wasm(\?|$)/.test(entries[i].name) && entries[i].responseEnd > 0) {
          wasmDone = true;
          break;
        }
      }
    } catch (e) {}
    if (wasmDone) {
      // The "boot watchdog" prefix tags these distinctly so they can be
      // discounted in review against hard instantiation errors.
      send("wasm_boot", "boot watchdog: wasm fetch completed but the app never started", null, null);
      revealCrash();
      return;
    }
    if (Date.now() - watchdogStart < 120000) setTimeout(watchdog, 15000);
  }
  setTimeout(watchdog, 20000);

  state.unsupported = missingWasmFeatures();
  if (state.unsupported) {
    whenDomReady(revealUnsupported);
    send("unsupported_browser", state.unsupported, null, null);
  }
})();
