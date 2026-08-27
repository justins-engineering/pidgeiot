# Error reporting & automatic trace collection (task #33)

Status: design doc, no code changes. Scope: when a real user breaks something in
`fancier` or trips a failure in `dovecote`, we should already hold enough context
to debug it without writing back to ask "what were you doing?" — and the user
should get a screen that says something honest with a one-click way to attach
their case to what we already captured.

Grounded in: `fancier/src/api/helpers.rs`, `fancier/src/lib.rs`,
`fancier/src/main.rs`, `fancier/src/components/feedback_modal.rs`,
`fancier/src/views/wrapper.rs`, `fancier/src/views/error.rs`,
`fancier/scripts/build-release.sh`, `fancier/Dioxus.toml`, `fancier/wrangler.toml`,
`fancier/public/_headers`, `dovecote/src/lib.rs` (the `POST /feedback` route),
`dovecote/src/helpers/alerts.rs`, `dovecote/src/helpers/ops_probe.rs`,
`dovecote/src/helpers/feedback.rs`, `dovecote/src/helpers/retention.rs`,
`dovecote/src/scheduled.rs`, `dovecote/wrangler.toml`, `capsules/src/feedback.rs`,
`infra/migrations/2026-08-17-billing-usage.sql`, and the built release artifact at
`target/dx/fancier/release/web/public/assets/fancier_bg-dxh*.wasm`.
`docs/design/alerts-triggers.md` is the house style this follows.

---

## 0. What "an error" means here

Five distinct failure classes, which matter because no single mechanism catches
more than two of them. Keeping them apart is most of the design.

| Class | Example | Who can see it |
|---|---|---|
| **A. Rust panic** | `unwrap()` on a `None`, a slice index out of range, an `expect` in a view | A Rust panic hook — and nothing else (§1.1–1.3) |
| **B. Wasm boot failure** | the `.wasm` 404s, fails to instantiate, OOMs on a low-memory phone | Only a JS handler installed before boot; no Rust ever runs |
| **C. JS exception** | a `web_sys` call throwing, a bug in `theme-init.js`, a third-party script | `window.onerror` / `unhandledrejection` |
| **D. Failed API call** | `GET /flocks` answering 500, a fetch that never resolves | `dispatch()` in `api/helpers.rs` already sees every one (§1.4) |
| **E. Backend error** | dovecote's `console_error!` paths — DB open failure, DO dispatch failure | Workers Logs, which today retains nothing (§1.7) |

Class A is the one that produces the "the page just froze" support ticket, and it
is the class we currently have *zero* visibility into.

---

## 1. Current state

### 1.1 There is no panic hook in a production build

`fancier` installs no panic hook of its own — no `std::panic::set_hook`, no
`console_error_panic_hook` (absent from `fancier/Cargo.toml`, and nothing in
`fancier/src/` calls `set_hook`).

Dioxus ships one, but it is dev-only. `dioxus-web` declares
`panic_hook: true` by default (`dioxus-web-0.7.10/src/cfg.rs:101`), and the code
that acts on it lives in `devtools.rs`, whose module declaration is:

```rust
// dioxus-web-0.7.10/src/lib.rs:36
#[cfg(all(feature = "devtools", debug_assertions))]
mod devtools;
```

`scripts/build-release.sh:71` builds with `--release`, so `debug_assertions` is
off and `devtools` is not compiled in at all. **The production bundle installs no
panic hook.** The default `std` hook writes to a stderr that goes nowhere on
`wasm32-unknown-unknown`, so today a production panic is completely silent: no
console message, no beacon, nothing.

### 1.2 `ErrorBoundary` cannot catch panics on wasm

This is a hard platform limit, not a configuration gap. `dioxus-core` documents it
on the type itself:

```rust
// dioxus-core-0.7.10/src/error_boundary.rs:27-34
/// A panic in a component that was caught by an error boundary.
/// <div class="warning">
/// WASM currently does not support caching unwinds, so this struct will not be
/// created in WASM.
/// </div>
pub(crate) struct CapturedPanic(pub(crate) Box<dyn Any + Send + 'static>);
```

and again at the call site:

```rust
// dioxus-core-0.7.10/src/any_props.rs:82-84
// on wasm this massively bloats binary sizes and we can't even capture the panic
// so do nothing
#[cfg(not(target_arch = "wasm32"))]
```

The cause is the target's panic strategy. `rustc --print target-spec-json --target
wasm32-unknown-unknown` reports `"panic-strategy": "abort"`, so `catch_unwind` can
never catch anything. A panic aborts the module.

The consequence is the single most important fact in this document: **after a Rust
panic the wasm module is dead.** Its memory is poisoned, the Dioxus event loop
stops, and no further Rust code will run. Any design that plans to render a nice
Dioxus error screen *after* a panic, or to `spawn_local` an async POST from Rust
*after* a panic, does not work. Whatever we do must happen either synchronously
inside the panic hook (which runs *before* the abort, with Rust state still
readable) or from JavaScript, which is unaffected.

`ErrorBoundary` is still worth having — it catches the *recoverable* class, where a
component returns `Element::Err` via `?` on a `Result` or an explicit `.throw()`.
That is a real and useful category, just not the one that causes frozen pages.

### 1.3 The shipped wasm has no name section, so stack traces are numeric

Parsing the section table of the current release artifact
(`target/dx/fancier/release/web/public/assets/fancier_bg-dxh7a1e5a63c0523eb1.wasm`,
4.9 MB) gives:

```
section 1 (type)      897        section 9  (elem)      6587
section 2 (import)    15110      section 12 (datacount) 2
section 3 (func)      4707       section 10 (code)      3874726
section 4 (table)     11         section 11 (data)      982742
section 5 (mem)       3          section 0  (custom) name="target_features" 157
section 6 (global)    159
section 7 (export)    2312
```

The only custom section is `target_features`. There is **no `name` section**, so
any JS-visible stack frame from wasm reads `wasm-function[3812]` — a raw function
index, meaningless without a symbol map we do not publish.

Note the trap here: running `strings` over that same binary *does* print
`fancier::views::pigeon::PigeonView` and friends, which makes it look like symbols
survived. They did not. Those strings live in the **data** section — they are
Dioxus's `#[component]` name literals and `tracing` event metadata (`event
fancier/src/api/helpers.rs:66`), readable by Rust code at runtime but invisible to
the JS stack-trace machinery.

`dx` has a flag for this: `--keep-names`, documented as *"the name section allows
tools like console_error_panic_hook to print backtraces with human-readable
function names without any browser extension"* (`dioxus-cli-0.7.10/src/cli/
target.rs:106-112`). `build-release.sh:71` does not pass it. §2.6 recommends
against adopting it for now and explains why.

### 1.4 `dispatch()` is the one funnel every API call already passes through

`fancier/src/api/helpers.rs:18` — every `fetch_json`, `fetch_json_any_status` and
`fetch_bytes` call in `src/api/*` goes through this single function, which already
holds `method`, `path`, and `response.status()`, and already has one cross-cutting
behavior grafted onto it (the 401 → `session_lost()` check at line 55). It is the
natural and only place a breadcrumb ring or a failed-request capture needs to be
written. Adding a second cross-cutting concern here is a continuation of an
established pattern, not a new one.

One subtlety worth carrying into the design: a fetch that fails at the network
layer returns `None` from `.ok()?` at line 44-46 *before* there is any status to
read. "500" and "the request never completed" are already distinguishable here,
and a breadcrumb should preserve that distinction.

### 1.5 The feedback path already exists end to end and is the right skeleton

- `capsules/src/feedback.rs` — request type, four byte caps
  (`MAX_FEEDBACK_BODY_BYTES` etc.), and a pure, unit-tested email formatter. The
  module header states the reason it lives in `capsules`: dovecote is a wasm-only
  `cdylib` whose tests can't run on a host target, so testable pure logic belongs
  here. The same reasoning applies to error signature computation (§3.2).
- `dovecote/src/lib.rs:2681` — `POST /feedback`: public, *optionally*
  authenticated (`require_auth_session(...).ok()`, so no session is never a 401),
  Content-Type checked, raw body capped before parsing, then each field capped,
  answering 202 without waiting on delivery.
- `dovecote/src/helpers/feedback.rs` — fire-and-log send via
  `send_via_usesend`, degrading to a logged no-op wherever `OPS_ALERT_EMAIL` is
  unset (production `[vars]` only).
- `fancier/src/components/feedback_modal.rs` + `api/feedback.rs` — remount-fresh
  modal, opened from a `FeedbackForm(Signal<bool>)` context provided by `Wrapper`
  (`views/wrapper.rs:30`) and consumed by both `Navbar` (lines 139, 342) and
  `Footer` (line 268). It already collects `page_context` from
  `window.location.pathname` and prefills the contact email from the Kratos
  identity.

An error report is a feedback submission with a machine-generated body. Almost
none of this needs to be rebuilt.

The route's own comment is also directly on point for §3.5:

> No per-IP rate limiter here -- that's platform-level (a Cloudflare WAF rule or
> Turnstile), not something to hand-roll in-route.

### 1.6 Cloudflare RUM collects performance, not errors

Cloudflare injects the Web Analytics beacon at the edge, with EU, UK and Swiss
visitors excluded (`build-release.sh` used to bake the tag in and no longer does,
because the exclusion works by not injecting the snippet at all). It collects
Core Web Vitals and Performance-API resource timings, and explicitly holds no
client-side state and does no fingerprinting. It captures **no JavaScript errors, no
exceptions, and no stack traces**. It is not a partial solution here; it is
orthogonal, and it should be left exactly as it is.

### 1.7 dovecote's `console_error!` output is currently retained nowhere

```toml
# dovecote/wrangler.toml:11-14
[observability.logs]
enabled = true
invocation_logs = true
head_sampling_rate = 0
```

`head_sampling_rate` is the fraction of incoming requests whose logs are persisted
to Workers Logs. Its range is 0–1 and its default is 1. **Set to 0, no invocation
is sampled, so nothing is written to Workers Logs at all.** Retention would be 7
days on a paid plan if anything were being kept.

This makes the hundreds of `console_error!` sites in `dovecote/src/` effectively
write-only: they are visible in a live `wrangler tail` and nowhere else. Every one of this codebase's best-effort/fire-and-log paths — the Postgres
mirror sync, alert email delivery, the retention sweep — reports its failures into
that void.

`git log -L 11,15:dovecote/wrangler.toml` shows the `head_sampling_rate = 0` line
arriving in `dd0054d` ("Overhaul Pigeon DOs, update capsules, sync DO/PG") — an
unrelated commit, with no rationale recorded. It reads as incidental rather than
deliberate. `fancier/wrangler.toml:21-22` separately has `[observability] enabled
= false`, which is correct and should stay: that Worker is a thin
markdown-negotiation shim over static assets and has nothing worth logging.

### 1.8 SSG and hydration constraints any of this must respect

- Every public route is prerendered to `public/<route>/index.html`
  (`build-release.sh:71`), and auth-gated routes prerender as `AuthGuard`'s
  "Verifying session..." placeholder because `Session.state` starts at
  `AuthState::Pending` and the cookie check lives in a `use_future` that never
  resolves during the synchronous render pass (`src/lib.rs:161-186, 219-233`).
  Anything hung off `AuthState` is therefore SSG-safe by construction — the same
  property task #49 relied on to keep the session-expiry notice out of
  `login/index.html`.
- **Nothing that touches `window` may run during the prerender pass.** The build
  compiles a native "server" target to drive it. Client capture must be
  `web_sys`-guarded (which it is naturally — `web_sys::window()` returns `None`)
  or, better, live in a JS file that only the browser ever loads.
- Hydration restores the *prerendered* route, not `window.location`, which is why
  `helpers/url_query.rs::url_query_param` exists. An error report must therefore
  read its route from `window.location.pathname` directly, never from a route
  prop — the same rule task #43 established.
- There is **no CSP** on the site (`fancier/public/_headers` sets cache and RFC
  8288 `Link` headers only), so a new script file needs no policy change.
- `Dioxus.toml:19-21, 32-34` already loads a static pre-boot script,
  `assets/theme-init.js`, in **both** release and dev. That is the exact slot and
  precedent for the JS shim in §2.2 — a sibling file, one line of config, no
  inline script, no bundler involvement.

---

## 2. Client capture

### 2.1 Recommendation: a Rust panic hook and a JS pre-boot shim, split by what each can see

Two mechanisms, because §1.2 forces it. Neither is a fallback for the other; they
cover disjoint classes.

**The Rust panic hook (class A — the important one).** Installed as the first
statement of `App` (or, more precisely, at the top of `fancier::App` before any
other hook), it runs *synchronously before the abort*, while Rust state is still
fully readable. That gives us, for free and with no binary-size cost:

- `PanicHookInfo::payload()` — the panic message, e.g. `called Option::unwrap()
  on a None value` or a real `expect` string;
- `PanicHookInfo::location()` — **`file`, `line` and `column` of the panic site**.

That location is a better grouping key and a better debugging pointer than a wasm
stack trace would be, and it is available whether or not the name section exists.
It is also the only "trace" wasm can give us: there is no unwinding, so
`std::backtrace` is unavailable and a Rust backtrace is not obtainable at any
price. The honest scope of "automatic trace collection" on this frontend is
**panic message + panic location + a breadcrumb ring**, not a call stack.

Because the hook runs pre-abort it can also read the breadcrumb ring out of a
Rust-side `static`, which is why breadcrumbs can stay in Rust (§2.3) rather than
being mirrored into JS.

**The JS shim (classes B and C).** A new `fancier/assets/error-shim.js`, added to
both `[web.resource] script` lists in `Dioxus.toml` alongside `theme-init.js`, so
it is installed *before* the wasm loader tag and survives whatever happens to the
module. It:

1. registers `window.onerror` and `window.addEventListener('unhandledrejection')`;
2. sets a `window.__pidgeiot_err = { panicked: false, sent: 0, boot: false }`
   marker the Rust hook can flip;
3. arms a boot watchdog — if no Rust code has reported "booted" within ~15s,
   report a class-B boot failure, since that is precisely the white-screen case
   where no Rust will ever run to report anything;
4. reveals the static failure panel (§5.1).

**Deduplication between the two.** A Rust panic aborts the module, which surfaces
to JS as a thrown `RuntimeError: unreachable` out of the wasm export — so
`window.onerror` will fire for a panic the Rust hook already reported. The hook
sets `window.__pidgeiot_err.panicked = true` *before* beaconing; the JS handler
suppresses any report within a few seconds of that flag being set. The flag is
also what the failure panel keys off, so the panel appears exactly once.

**Class D** needs no new mechanism — it is a breadcrumb (§2.3), not a report, with
one exception noted there.

### 2.2 Transport: `navigator.sendBeacon` with a `text/plain` body

`sendBeacon` is the right primitive and the reasons are specific:

- It is **synchronous to call and fire-and-forget**, so the panic hook can invoke
  it without awaiting anything — which it cannot do, since there is no runtime
  left to await on.
- The browser owns delivery after the call returns, so it survives the module
  abort and survives the page being closed immediately afterward.
- It is available in the already-enabled `web-sys` `Navigator` feature:
  `send_beacon_with_opt_str` (`web-sys-0.3.97/src/features/gen_Navigator.rs:526`),
  ungated. No new `web-sys` features, no new crates.

**The body must be `text/plain`, carrying JSON as a string.** `pidgeiot.com` and
`api.pidgeiot.com` are cross-origin, and `Content-Type: application/json` is not
CORS-safelisted, so a JSON beacon triggers a preflight — which `sendBeacon` cannot
negotiate, so the report is dropped silently. `send_beacon_with_opt_str` sends
`text/plain;charset=UTF-8`, which is safelisted, making it a simple request the
browser sends unconditionally. `sendBeacon` ignores the response entirely, so the
CORS headers on the way back are irrelevant to whether dovecote receives it.

Two consequences to design around:

- The ingest route must accept `text/plain` (and, for the ordinary
  non-panic path, `application/json`). This is the one place we deliberately
  diverge from `POST /feedback`'s strict `application/json` check, and the
  divergence needs a comment saying why.
- The Beacon spec sets credentials mode to `include`, and `ory_kratos_session`
  carries `cookies.domain: pidgeiot.com`, so `api.pidgeiot.com` is same-site and
  the cookie rides along. **dovecote can therefore attribute the report to a real
  user server-side, from the session, without the client ever sending an
  identity.** That is both the better engineering answer and the better privacy
  answer (§2.7).

For the non-panic paths (JS shim, "Report a problem" button) an ordinary
`fetch(..., {keepalive: true})` is fine and gives us a status code; `sendBeacon` is
reserved for the panic hook, where nothing else works.

### 2.3 The context envelope

New in `capsules` (mirroring `capsules/src/feedback.rs` — types, caps, and a pure
tested function, in a crate whose tests actually run on a host target):

```rust
pub enum ErrorKind { RustPanic, WasmBoot, JsException, UnhandledRejection, ApiFailure }

pub struct ErrorReport {
  pub kind: ErrorKind,
  pub message: String,            // panic payload, or JS error message
  pub location: Option<String>,   // "src/views/pigeon.rs:412:18", or JS file:line:col
  pub stack: Option<String>,      // JS stack when there is one; None for panics
  pub route: String,              // normalized, see below
  pub build: String,              // release identity, see 2.4
  pub user_agent: Option<String>,
  pub breadcrumbs: Vec<Breadcrumb>,
  pub session_kind: SessionKind,  // SignedIn | Anonymous -- NOT an id
  pub occurred_at_ms: u64,
}

pub struct Breadcrumb {
  pub age_ms: u32,                // relative to the error, not a wall clock
  pub kind: BreadcrumbKind,       // Nav | Api | Ui
  pub detail: String,             // "GET /flocks/:flock_id/pigeons -> 500"
}
```

Caps in the same style as the feedback module: `MAX_ERROR_REPORT_BYTES` (16 KiB),
`MAX_ERROR_MESSAGE_BYTES` (2 KiB), `MAX_ERROR_STACK_BYTES` (8 KiB),
`MAX_ERROR_BREADCRUMBS` (20).

**Route normalization is load-bearing and does double duty.**
`/flocks/c84932d0-.../pigeons/59d0c929...` becomes
`/flocks/:flock_id/pigeons/:pigeon_id`. It makes grouping work (otherwise every
pigeon produces its own signature), and in the same stroke it strips tenant
identifiers out of the stored report. Pure function, unit-tested in `capsules`,
alongside the signature hash (§3.2). Query strings and fragments are dropped
entirely, never normalized — `?flow=<kratos-flow-id>` and `?token=<invite-token>`
are exactly the sort of thing that must not land in an error store.

**Breadcrumbs** are a fixed-size ring (20 entries) in a Rust-side
`thread_local!`/`static`, written from three places: `dispatch()` in
`api/helpers.rs` (method, normalized path, status or "network failure"), a route
change (from the existing `PageTitle` component in `views/wrapper.rs`, which
already runs `use_route` on every navigation), and a small number of explicit UI
markers if they earn their place later. **Detail strings record shape only —
method, normalized path, status — never request or response bodies.**

Keeping the ring in Rust is safe specifically because the panic hook runs before
the abort and can read it. The JS shim cannot, so a class-B/C report simply
carries no breadcrumbs; for a boot failure there is nothing to carry anyway.

### 2.4 Build identity

Grouping and "is this fixed?" both need to know which build a report came from.
`build-release.sh` already computes content-hashed asset names
(`fancier_bg-dxh7a1e5a63c0523eb1.wasm`) — that hash is a perfect build identity and
is already unique per release. The build script should write it into a
`window.__pidgeiot_build` global in the same head-injection pass that already
writes titles and OG tags (`build-release.sh:239-258`), where both
the JS shim and the Rust hook can read it. No new constant, no `.env` entry, no
duplicate of something that already exists.

### 2.5 Sampling and the panic-loop guard

**Do not sample.** Sampling is for high-volume telemetry; this is a pre-revenue
platform where a single user's single panic is the entire signal we are building
this to catch. Throwing away 90% of the one report we will get this month is
exactly backwards. Volume control comes from grouping (§3.2) and caps, not from
dropping reports.

The DoS concern the task raises is real but is not a sampling problem, and it is
mostly self-limiting:

- **A Rust panic cannot loop.** The module is dead after the first one. One panic
  produces exactly one report, per page load, by construction.
- **A JS exception can loop** — e.g. an error thrown from a `requestAnimationFrame`
  callback. The shim caps itself at **5 reports per page load** and dedupes
  identical `message + location` pairs within that budget. This is the actual
  runaway risk and it is bounded in the client.
- **Server-side**, §3.5 puts a real limiter in front of the route.

### 2.6 On `--keep-names`: recommend not adopting it yet

It would be one word on `build-release.sh:71`, and it would turn class-B/C JS
stack frames from `wasm-function[3812]` into readable Rust symbols. Against that:

- It does nothing for class A, which is the class that matters, because the panic
  hook already yields an exact `file:line:col`.
- It costs binary size — roughly a few hundred KB raw for ~4,400 functions'
  mangled names, likely well under 100 KB gzipped, but **that is an estimate and
  should be measured before adopting, not asserted**. This repo has already been
  burned by wasm-size and asset-ordering effects on mobile LCP (`build-release.sh:
  212-218` on why the wasm must never be preloaded).

Recommendation: leave it off, and revisit only if a real incident arrives where
the panic location genuinely was not enough. If we do revisit, measure the raw and
gzipped delta and re-run the Lighthouse mobile check before keeping it.

### 2.7 What is never collected

This is a policy, and the point of writing it down is that it constrains the
implementation rather than describing it.

**Never sent by the client:**
- Any user identity — no email, no user id, no org id, no display name. The
  client sends only `SessionKind::SignedIn | Anonymous`. dovecote resolves the
  real identity from the session cookie server-side (§3.3), the same way `POST
  /feedback` already refuses to trust `FeedbackSubmitter` from the body.
- Cookie values, `localStorage` contents, `Authorization` headers, device tokens,
  PSKs, or any credential. (CLAUDE.md's handling rule applies here as much as
  anywhere: credential values are referenced, never carried.)
- Telemetry values, shadow `target_config`/`current_config` contents, firmware
  bytes, or device log chunks. None of these appear in an error report even when
  the error happened while rendering them.
- Form field values, textarea contents, clipboard, or any DOM scrape.
- Full URLs. Query strings and fragments are dropped; the path is normalized to a
  route template with ids replaced by parameter names (§2.3).
- Request or response bodies, in breadcrumbs or anywhere else.

**Never stored by the server:**
- The client IP. Cloudflare necessarily sees it and the rate limiter keys on it
  (§3.5), but it is not written to any row.
- Anything beyond a coarse user agent string, which is kept because "only on
  Safari 17" is a real and common root cause.

**Residual risk, stated rather than hidden:** a panic *message* can contain
whatever a `expect`/`panic!` format string interpolated, and that could be user
data. Two mitigations: the message is byte-capped, and the codebase adopts the
rule that panic and `expect` messages must not interpolate user or device data.
That rule is cheap to follow and worth adding to CLAUDE.md's conventions section
when this ships.

**A `/privacy/` page update is required before Phase 1 goes to production.**
`fancier/src/views/privacy.rs`'s "What we collect" section covers account data,
device data, and server-side "Web logs" (`IP address, user agent, timestamps, and
the routes requested`) — it says nothing about diagnostics collected *from the
browser*, because today there are none. That stops being true the moment this
ships. The "What we don't do" list ("no third-party advertising or ad-tracking
scripts", "no tracking cookies") stays accurate under this design, which is
another point in favor of §3.1's first-party recommendation.

---

## 3. Server storage

### 3.1 Options considered

**Sentry (or any hosted error-tracking SaaS).** Honestly assessed, it is a poor
fit here on three independent grounds, any one of which would be enough:

- *It does not do the thing we need.* Sentry's value on a JS frontend is source-map
  symbolication of stack traces. We have no stack traces worth symbolicating — we
  have a panic message and a `file:line:col` from a Rust hook (§1.3, §2.1). The
  `sentry` Rust crate does not support `wasm32-unknown-unknown` as a browser
  target, so realistically this would be the JS SDK receiving hand-built events,
  i.e. paying for a platform to store a payload we assembled ourselves.
- *Cost and footprint.* The JS SDK is a substantial addition to a bundle whose
  size is already an actively managed constraint, and the budget for this is
  described as tiny.
- *Lock-in ethos.* The platform's stated position is first-party and no-lock-in;
  the `/open-source` page and the AGPL licensing make that a public claim, not
  just an internal preference. Routing our users' diagnostic data to a third party
  contradicts it, and would have to be disclosed on `/privacy/`.

**Workers Logs + Logpush.** Free-ish and already partly configured, and it will
matter for class E (§4). But it cannot be the primary store for client errors: it
is a log stream, not a queryable grouped dataset; retention is 7 days on the paid
plan; there is no grouping, no first-seen/last-seen, no "is this fixed"; and
Logpush to R2 solves durability while making the data *less* queryable, not more.
The support panel (task #30) would have nothing to render from it.

**Postgres.** Already the source of truth for every cross-cutting feature the
platform has, already reached from every Worker path via Hyperdrive, already the
thing this team ships features against fastest, and already the store the support
panel will be querying for everything else it shows.

### 3.2 Recommendation: Postgres, two tables, signature grouping

Two tables, because a group and an occurrence want different lifetimes and
different retention.

**Signature** = a truncated SHA-256 over `(kind, normalized_message, location)`:

- *normalized message* replaces UUIDs, long hex ids and bare integers with
  placeholders, so `pigeon 59d0c929... not found` and `pigeon a3f21b04... not
  found` are one group rather than two;
- *location* is the panic's `file:line:col` for class A, and `file:line:col` from
  the JS error for classes B/C.

Both the normalizer and the signature function are **pure and live in `capsules`
with unit tests**, exactly as `format_feedback_email` does and for the identical
reason recorded in that module's header: dovecote's wasm-only `cdylib` cannot run
host tests, and `capsules` can. Route normalization (§2.3) joins them there, so
`fancier` and `dovecote` can never disagree about what a route or a signature is.

A note on line numbers as a grouping input: any edit to a file shifts them, so a
group's identity does not survive a refactor. That is acceptable and arguably
correct — a new build is a new signature, and "this reappeared after we thought we
fixed it" surfaces as a new group with a fresh alert rather than silently merging
into an old one. The `build` column makes the distinction visible.

### 3.3 Schema

Following `infra/migrations/2026-08-17-billing-usage.sql` exactly: idempotent,
`SET ROLE dovecote` at the top so objects are app-owned from creation, explicit
`ALTER TABLE ... OWNER TO` at the bottom as self-healing, commented staging
variants, `RESET ROLE` at the end.

```sql
CREATE TABLE IF NOT EXISTS error_groups (
  signature    TEXT PRIMARY KEY,
  kind         TEXT NOT NULL,
  message      TEXT NOT NULL,          -- exemplar, capped
  location     TEXT,
  first_seen   TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen    TIMESTAMPTZ NOT NULL DEFAULT now(),
  first_build  TEXT,
  last_build   TEXT,
  occurrences  BIGINT NOT NULL DEFAULT 0,
  notified_at  TIMESTAMPTZ,            -- claimed once; see 6.1
  resolved_at  TIMESTAMPTZ             -- set by hand / by the support panel
);

CREATE TABLE IF NOT EXISTS error_events (
  id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  signature    TEXT NOT NULL REFERENCES error_groups(signature) ON DELETE CASCADE,
  occurred_at  TIMESTAMPTZ NOT NULL,
  received_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  user_id      UUID,                   -- server-resolved from the session, never from the body
  route        TEXT,
  build        TEXT,
  user_agent   TEXT,
  stack        TEXT,
  breadcrumbs  JSONB,
  report_note  TEXT                    -- user's words, when they came via "Report a problem"
);

CREATE INDEX IF NOT EXISTS idx_error_events_signature ON error_events(signature);
CREATE INDEX IF NOT EXISTS idx_error_events_occurred  ON error_events(occurred_at);
CREATE INDEX IF NOT EXISTS idx_error_groups_last_seen ON error_groups(last_seen DESC);
```

Ingest is one upsert plus one insert:

```sql
INSERT INTO error_groups (signature, kind, message, location, first_build, last_build, occurrences)
VALUES ($1,$2,$3,$4,$5,$5,1)
ON CONFLICT (signature) DO UPDATE
  SET last_seen = now(),
      last_build = EXCLUDED.last_build,
      occurrences = error_groups.occurrences + 1
RETURNING (xmax = 0) AS is_new;
```

`xmax = 0` on the returned row distinguishes an insert from an update, which is
what §6.1 keys the new-signature alert off — no second round trip.

Note that `breadcrumbs` is written as JSONB but should be read back with a
`::text` cast if it is ever read through `tokio-postgres`, per the constraint
recorded at `helpers/alerts.rs:14-20` (this workspace's `tokio-postgres` is not
built with `with-serde_json-1`).

There is also a runtime `ensure_error_tables` following the
`ensure_alert_tables`/`ensure_billing_usage_tables` convention, so a database the
migration has not reached self-heals — and, as `helpers/alerts.rs:43-51` records,
it must not attempt to create triggers.

### 3.4 Retention

Ride the existing 5-minute cron in `scheduled.rs`, like `retention.rs` and
`ops_probe.rs` already do, and for the same recorded reason: the Cloudflare
account allows only 5 cron triggers and prod+staging already consume 2.

- `error_events`: delete rows older than 90 days, **and** keep at most the newest
  200 events per signature. The second rule is what stops a single high-volume
  group from dominating the table; 200 exemplars is far more than anyone will
  read.
- `error_groups`: keep indefinitely. They are small, and first-seen history is the
  most useful thing in the table.
- Batch-limited per invocation, matching `retention.rs`'s
  `RETENTION_BATCH_LIMIT` reasoning about a cron invocation's CPU budget.

### 3.5 Rate limiting the ingest route

`POST /feedback`'s comment says a limiter belongs at the platform level rather
than hand-rolled in-route, and that was the right call there. It is worth
revisiting *for this route specifically*, because a first-party primitive now
exists and is directly usable:

- Cloudflare's rate-limiting binding is stable and configured in `wrangler.toml`
  as `[[ratelimits]]` with a `namespace_id`, a `limit`, and a `period` of 10 or 60
  seconds.
- `worker` v0.8.5 — the version already in `Cargo.lock` — exposes it:
  `env.rate_limiter("ERROR_INGEST_LIMITER")` returning a `RateLimiter` with
  `async fn limit(&self, key: String) -> Result<RateLimitOutcome>`
  (`worker-0.8.5/src/rate_limit.rs`), plus an `Error::RateLimitExceeded` variant.

Recommendation: key on `CF-Connecting-IP`, roughly **20 reports / 60s**, answering
**429** over the limit. The key is used and discarded — the address is never
written to a row (§2.7). Each environment gets its own `namespace_id`, since two
bindings sharing one namespace share counters across Workers.

**429 specifically, and never 401.** Task #49 made 401 a cross-cutting sign-out
signal on the client (`api/helpers.rs:6-11, 55-57`): any dovecote route answering
401 for a reason other than "the session no longer resolves" will silently sign
dashboard users out. An error-ingest route that 401'd a stale-session report would
sign the user out *in the middle of reporting a bug*, which is about the worst
possible failure mode for this feature. The route is optionally authenticated,
exactly like `POST /feedback`, and must never answer 401.

---

## 4. dovecote's own errors (class E)

Two changes, in increasing order of ambition. Only the first is urgent.

**4.1 — Turn logging back on. One line, do it in Phase 1.**
`head_sampling_rate = 0` → `1` in `dovecote/wrangler.toml` (all three env blocks).
Cost is a fraction of a cent per million requests at current volume; benefit is
that the hundreds of existing `console_error!` sites start producing 7 days of queryable
history instead of nothing. This is the single highest value-per-effort item in
this document, and it should not wait for any of the rest of it. It needs a
production deploy, so it is owner-gated (§8).

If sampling was set to 0 deliberately for a cost reason nobody recorded, an
intermediate `0.1` still leaves error logs mostly intact while cutting invocation
log volume; but note that head sampling drops *whole invocations*, so a 0.1 rate
discards 90% of errors too. Full rate is the right answer for a platform at this
volume.

**4.2 — Selective in-band capture, Phase 3.** Not every `console_error!` deserves
a database row; most are best-effort sync failures that are individually
uninteresting and collectively meaningful. Add a small
`helpers::errors::report_server_error(env, kind, message, context)` that writes
into the same `error_groups`/`error_events` tables with `kind: 'server'`, and call
it from the handful of sites where a failure means a user's request actually
broke — `get_db_client` failure, DO dispatch failure, a 500 returned from a
dashboard route. Best-effort and logged, never blocking, matching the convention
CLAUDE.md states for every DB write from this codebase.

Deliberately **not** recommended: a tail worker. It is the textbook answer, but it
is a second Worker script to deploy, version and reason about, and it would
duplicate what 4.1 plus a targeted 4.2 already give us at a fraction of the
complexity. Revisit if and when the volume justifies it.

---

## 5. What the user sees

### 5.1 The failure screen has to be static HTML, not a Dioxus component

This falls directly out of §1.2: after a panic there is no Dioxus left to render
anything. A `views::error::AppCrashed` component would never mount.

So the failure panel is markup that is already in the DOM, hidden, revealed by the
JS shim. Concretely: a `<div id="app-crash" hidden>` injected into every
prerendered page by `build-release.sh`'s existing head/body injection pass, styled
with a few inline rules so it does not depend on the app's CSS having loaded (a
class-B boot failure may well be a failure to load anything). The shim sets
`hidden = false` and fills in the error id.

Content: a plain statement that something broke, the short error id we generated,
a "Reload" button, and a "Tell us what happened" button that opens a minimal
plain-JS text box POSTing to the same ingest route with the same error id and the
user's note (landing in `error_events.report_note`). It cannot open the Dioxus
feedback modal — same reason.

This is deliberately austere. It is the one screen in the product that must work
when nothing else does, so it depends on nothing.

### 5.2 `ErrorBoundary` for the recoverable class

Worth adding, with clear eyes about what it does: it catches components that
return `Element::Err`, not panics. Wrap `Wrapper`'s `Outlet` (`views/wrapper.rs:
35`) in an `ErrorBoundary` whose `handle_error` renders a real Dioxus failure card
— chrome intact, navigation still working, "Report this" prefilled — and reports
through the same ingest route with `kind: JsException`-adjacent semantics.

Practical caveat: today almost nothing in `fancier` returns `Err` from a component
(the codebase's convention is `Option<T>` collapsing to `None`, per
`api/helpers.rs`), so this boundary will rarely fire *until* code starts using it.
That is fine — it makes `?` on a `Result` a safe thing to write in a view for the
first time, which is a genuine improvement over the current choice between
`unwrap` and silent `None`. Note it also needs a `SuspenseBoundary`-style check
against SSG: `handle_error` must not run during prerender, which it won't, since
nothing throws during the synchronous pass.

### 5.3 "Report a problem", more prominent than today's feedback link

Per the owner's direction. Today the only affordance is a "Send Feedback" text
link in the footer (`components/footer.rs:268`) and an item in the navbar menus
(`components/navbar.rs:139, 342`) — both require the user to already be looking
for it.

Recommendation: a **persistent, low-key floating button** in the bottom-inline-end
corner on authenticated routes, rendered from `Wrapper` next to the existing
conditional `FeedbackModal`. Small, `btn-circle btn-sm` with a message icon and an
`aria-label`, never covering content, no badge, no animation. It opens the same
`FeedbackModal`, which gains:

- a fourth category, `FeedbackCategory::Problem` (extending the existing enum in
  `capsules/src/feedback.rs`, whose wire format is already snake_case and whose
  unknown values already 400);
- automatic attachment of the last N breadcrumbs and the current build, so a
  user's "it broke when I clicked save" arrives with the request trail attached —
  **this is the piece that makes it possible to debug without asking them to
  reproduce**, and it works even when nothing crashed at all;
- when it is opened from the §5.2 error card, the error id prefilled and shown, so
  the user's words and the captured trace are joined server-side.

The existing footer and navbar entries stay. Keep the copy "Report a problem" on
the floating button and "Send Feedback" in the footer — they are genuinely
different intents arriving at the same form.

Whether the floating button also appears on marketing pages is an owner call
(§8): it is more visible there, and also more visible to prospects.

---

## 6. The review loop

### 6.1 Email on a new signature, reusing the machinery that exists

When the §3.3 upsert returns `is_new`, send one email to `OPS_ALERT_EMAIL` via
`send_via_usesend`. Three existing precedents make this nearly free:

- `ops_probe.rs` — notify on *transitions* only, so one outage is one email;
- `alerts.rs::apply_alert_transition` — the fired/cleared state machine, same
  shape;
- `billing_usage_periods.warned_at` — a nullable timestamp claimed atomically
  (`UPDATE ... WHERE warned_at IS NULL RETURNING`) so concurrent consumers cannot
  double-send. `error_groups.notified_at` is claimed the same way.

The subject follows the established convention (`[OPS]`, `[FEEDBACK]`,
`[SEVERITY]`): `[ERROR] New: <message excerpt>`. Body carries the signature, kind,
location, route, build, and the occurrence count.

Only *new* signatures notify. A group that is already firing daily has already
been reported once and does not need to mail again; that is the same restraint
`apply_alert_transition`'s `(Firing, true)` no-op arm applies.

Note the built-in reason this cannot become a mail flood: an unbounded number of
distinct signatures would mean an unbounded number of distinct panic sites, which
is not a thing that happens. A per-hour cap on new-signature emails is available
if it ever does, but is not worth building up front.

### 6.2 The support panel, later

Task #30's role-aware first-party support panel is the right home for the review
UI, and this design deliberately produces exactly the data it will want: groups
ordered by `last_seen`, occurrence counts, first/last build, per-group event
exemplars with breadcrumbs, and a `resolved_at` to mark one closed. Until it
exists, `psql` against the same tables answers every question, which is the point
of choosing Postgres in §3.1.

Reuse `docs/design/tenancy-isolation.md`'s reasoning when it is built: an error
store is cross-tenant by nature and must sit behind an operator-role check, not a
flock ACL.

---

## 7. Phasing and estimates

Estimates are in this team's demonstrated units. The reference point is the
feedback feature (task #13), which shipped `capsules` types with a unit-tested
formatter, a public dovecote route, and a wired-up fancier modal as **three atomic
commits in a single day** (2026-08-08). Recent throughput has been 17–34 commits
per working day.

### Phase 1 — capture, store, alert, report link (~1.5 sessions)

The smallest thing that debugs a real user's breakage.

1. `dovecote/wrangler.toml`: `head_sampling_rate` 0 → 1 (§4.1). *Minutes.* Ship
   this first and separately; it stands alone.
2. `capsules`: `ErrorReport`/`Breadcrumb`/`ErrorKind`, byte caps, and the three
   pure functions — message normalizer, route normalizer, signature hash — with
   unit tests. Mirrors `capsules/src/feedback.rs` one for one.
3. `infra/migrations/2026-08-19-error-reporting.sql`: the two tables, in the
   established `SET ROLE` / `OWNER TO` / `RESET ROLE` form.
4. `dovecote`: `helpers/errors.rs` (ensure-tables, upsert-and-insert, new-signature
   email) and the `POST /errors` route in `lib.rs` — optionally authenticated,
   `text/plain` *and* JSON accepted, capped, 202, never 401, rate-limited via the
   new `[[ratelimits]]` binding. Structurally a copy of `POST /feedback`.
5. `fancier`: the Rust panic hook, the breadcrumb ring, breadcrumb writes in
   `dispatch()` and on navigation, and `assets/error-shim.js` wired into both
   `[web.resource]` script lists.
6. `build-release.sh`: emit `window.__pidgeiot_build`, inject the hidden crash
   panel.
7. `docs/api.md`: an "Error reporting" section plus rows in the "Rate & size
   limits" table. `views/privacy.rs`: disclose the collection.

Deliverable: a production panic produces a row we can read, an email we receive,
and a screen that tells the user something true.

### Phase 2 — grouping, review UI, and the prominent report affordance (~1 session)

8. The floating "Report a problem" button, `FeedbackCategory::Problem`, automatic
   breadcrumb/build attachment, error-id joining (§5.3).
9. `ErrorBoundary` around `Wrapper`'s `Outlet` with a real Dioxus failure card
   (§5.2).
10. Retention sweep on the existing cron (§3.4).
11. Read routes (`GET /errors`, `GET /errors/:signature`) behind an operator
    check, and a first read-only view in the support panel — or deferred wholesale
    into task #30 if that lands first.

### Phase 3 — backend capture polish (~0.5–1 session)

12. `report_server_error` and its call sites (§4.2).
13. Whatever the first month of real data says is missing. Deliberately unplanned.

Phases 1 and 2 are independently shippable and each is worth having alone. Phase 3
should not start before there is real Phase 1 data, because its whole job is to
close gaps we have not yet observed.

---

## 8. Decisions needed from the owner

1. **Production deploy of the `head_sampling_rate` change (§4.1).** One line, but
   it is a production deploy, which is gated. Recommend doing it immediately and
   separately from everything else.
2. **Confirm no cost objection to full log sampling.** The change is cheap at
   current volume, but nobody recorded why it was set to 0, so it is worth an
   explicit "yes" rather than an assumption.
3. **Does the floating "Report a problem" button appear on public marketing pages,
   or only behind the login?** Recommend authenticated routes only to start —
   maximum value where breakage actually happens, no effect on the marketing
   pages' conversion surface.
4. **`/privacy/` copy.** The page needs a diagnostics-collection paragraph before
   Phase 1 reaches production. Happy to draft it against the §2.7 policy for
   review.
5. **Retention windows.** 90 days of events and 200 events per signature are
   proposed, not derived from anything. Easy to change later; worth a glance now.

Nothing else in this document needs a decision to proceed.

**Decisions recorded (owner, 2026-08-19):** 1 and 2 are done — the sampling flip
is committed and deploying, with no cost objection. 3: authenticated routes only.
4: the paragraph drafted in the review file was approved verbatim and now lives on
`/privacy/`. 5: 90 days / 200 per signature / groups kept indefinitely, adopted
with the review's conditions (received_at-keyed sweep, normalized redacted group
exemplar, manual-report eviction exemption, junk-group aging, erasure hook).

---

## 9. Summary of recommendations

- **Capture with a Rust panic hook plus a pre-boot JS shim**, because wasm's
  `panic=abort` means `ErrorBoundary` cannot see panics and the module is dead
  afterward. The hook runs before the abort, which is what makes panic message,
  `file:line:col`, and a Rust-side breadcrumb ring all reachable.
- **Send via `navigator.sendBeacon` with a `text/plain` body**, so it needs no
  runtime, survives the abort, and avoids a CORS preflight `sendBeacon` cannot
  negotiate. The session cookie rides along, so identity is resolved server-side
  and never sent by the client.
- **Store in Postgres**, two tables, signature-grouped. Not Sentry (it symbolicates
  stack traces we do not have, costs bundle size and money, and contradicts the
  no-lock-in position); not Workers Logs alone (no grouping, 7-day retention).
- **Turn `head_sampling_rate` back to 1 now** — the hundreds of `console_error!` sites in
  dovecote currently report into nothing at all. Highest value per unit of effort
  in this document, and independent of everything else.
- **Reuse what exists**: `POST /feedback`'s optionally-authenticated capped-body
  route shape, `send_via_usesend` for delivery, `ops_probe.rs`'s notify-on-
  transition discipline for new-signature alerts, the 5-minute cron for retention,
  `capsules`' pure-and-tested-logic convention for signatures and normalization,
  and `dispatch()` as the one place every API call already passes.
- **Make the crash screen static HTML**, not a Dioxus component, since Dioxus is
  gone by the time it is needed.
- **Promote the report affordance** to a persistent floating button that attaches
  breadcrumbs and build automatically — useful even when nothing crashed.
- **Do not sample client reports.** At this volume every report is the signal.
  Bound the runaway case in the client (5/page load) and with a real rate limiter
  server-side (`worker` 0.8.5 exposes Cloudflare's binding), answering 429 —
  **never 401**, which task #49 turned into a sign-out signal.
