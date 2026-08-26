// Pre-boot error capture for the failure classes Rust can never see: a
// wasm bundle that fails to load or instantiate (no Rust ever runs), and
// plain JS exceptions. Loaded as a static script before the wasm loader
// tag (same slot as theme-init.js), so it survives whatever happens to
// the module. The Rust panic hook covers panics itself and coordinates
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

  var state = { panicked: false, sent: 0, boot: false, last_report: null };
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

  window.addEventListener("error", function (ev) {
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
    if (state.boot || state.panicked) return;
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
})();
