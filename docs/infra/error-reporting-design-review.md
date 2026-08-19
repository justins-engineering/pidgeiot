# Review: error reporting & automatic trace collection (second reviewer)

Reviewed: `docs/infra/error-reporting-design.md` as of 2026-08-19, against the
owner's binding decisions (cost basis settled, sampling flipped to 1 and
deploying, report affordance on authed routes only, retention 90 d / 200 per
signature / groups kept) and the owner's privacy mandate: collect nothing
personally identifiable or sensitive if it can be avoided.

## Verdict: sound-after-listed-changes

The capture architecture is correct and unusually well-grounded: the
panic-hook/JS-shim split, the `sendBeacon` + `text/plain` transport reasoning,
the Postgres/signature storage model, and the never-401 rule all hold up.
Every load-bearing platform claim I checked is true in the current tree:

- `dispatch()` is the single funnel and 401-is-sign-out is exactly as
  described (`fancier/src/api/helpers.rs:18`, `:55`).
- `POST /feedback` is shaped as described — optional session resolved
  server-side, never trusted from the body, raw-body cap before parse
  (`dovecote/src/lib.rs:2681-2771`).
- No panic hook exists anywhere in `fancier` (no `set_hook`, no
  `console_error_panic_hook` in `Cargo.toml` or `src/`).
- `worker` 0.8.5 really ships the rate-limiter binding
  (`rate_limit.rs`: `RateLimiter::limit(key) -> RateLimitOutcome { success }`,
  plus `Error::RateLimitExceeded`), and `web-sys` 0.3.97 has
  `send_beacon_with_opt_str` ungated (`gen_Navigator.rs:526`).
- The sampling flip is already committed: `dovecote/wrangler.toml:17` has
  `head_sampling_rate = 1` with a rationale comment.

The required changes cluster in two places the design underweights: the ingest
route as an *adversarial* surface (the source is AGPL-public, so attackers can
compute real signature inputs), and the identity policy, which the team lead's
refinement fixes and which maps cleanly onto the transport split. The
`console_error!` audit also found four production-firing sites that log email
addresses or user-configured endpoint URLs — newly retained the moment the
sampling deploy lands, so they gate it.

---

## Findings

### High

**H1. The new-signature email is an unauthenticated mail cannon (§6.1).**
The design's claim that "an unbounded number of distinct signatures … is not a
thing that happens" is false under adversarial input: every crafted report
with a random message *is* a new signature, and each new signature sends one
ops email. At the proposed 20 reports / 60 s per IP, a single IP can drive
~28,800 ops emails per day through useSend — cost, sender reputation, and an
inbox in which real reports drown. `notified_at` dedupes per signature and
does nothing here. The Cloudflare rate-limit binding's counters are also
approximately per-colo, not global, so the 20/60 s bound is per IP *per data
center* — fine for its purpose, but it cannot be the email guard.
**Required in Phase 1, not deferred:** a global budget on new-signature
notifications — e.g. at most 5 per hour, claimed atomically in the same
`WHERE … IS NULL RETURNING` style as `billing_usage_periods.warned_at`, with
overflow folded into the next allowed email as "N further new signatures
suppressed". This is ~30 lines and turns the worst abuse outcome from "ops
inbox destroyed" into "one summary line".

**H2. Identity resolution must be bound to Content-Type, or text/plain is a
CSRF vector for forged identified reports (§2.2, §3.5).** `text/plain` is
CORS-safelisted precisely so any web page on the internet can POST it
cross-origin, with credentials, with no preflight. If the ingest route
resolves the session on a text/plain request, a hostile page can file a
report *as* any visiting signed-in user — and once manual reports carry
identity (see the policy below), that is a forged support request under the
user's name, inviting us to contact them about something they never sent.
The fix is structural and free: **text/plain requests are always anonymous
automatic kinds — the handler never resolves a session for them; the
identified manual kind is accepted only as `application/json`,** which is not
safelisted, forces a CORS preflight, and is therefore gated to `ROOT_URL` by
the existing `build_cors` machinery. This single rule simultaneously makes
"anonymous by construction" real (the automatic branch has no code path that
reads the cookie) and makes identified reports unforgeable cross-origin. The
design's current wording ("optionally authenticated, text/plain *and* JSON
accepted") permits the vulnerable combination and must be tightened.

### Medium

**M1. `error_groups` can be poisoned and grows without bound (§3.2-§3.4).**
Three sub-issues, all downstream of the ingest being unauthenticated and the
source being public:
(a) Anyone can compute a *real* group's signature inputs from the published
source and flood that group — and "keep newest 200 per signature" then evicts
the genuine exemplars in favor of the attacker's junk. Exempt events carrying
`report_note` (manual reports) from eviction, and consider keeping the oldest
few events per group alongside the newest.
(b) Crafted *unique* messages bypass the 200-per-signature cap entirely (each
is a new group), and groups are kept forever, so junk groups accumulate
without bound. Add a group-level sweep — e.g. delete groups with
`occurrences <= 2`, no `report_note` events, and `last_seen` older than 30
days — so noise ages out while every group that ever mattered stays.
(c) `last_build` is overwritten from client-claimed data on every upsert;
validate `build` against the known shape (`dxh` + 16 hex chars) and reject or
blank nonconforming values, so the catalog of "which builds still throw this"
stays meaningful.

**M2. Retention must key on `received_at`, never `occurred_at` (§3.3-§3.4).**
`occurred_at` is client-claimed. A report stamped far in the future never ages
out of a sweep keyed on it. Sweep on `received_at` (server clock), and clamp
`occurred_at` to within ±24 h of server time on ingest. The
`idx_error_events_occurred` index should be on `received_at` for the same
reason.

**M3. State as a MUST: the server re-derives everything derivable (§2.3,
§3.2).** The design gets this right implicitly (the `ErrorReport` struct
carries no signature field — keep it that way) but never states it. Three
explicit rules: the server never accepts a client-computed signature; the
server re-normalizes the message *and* the route server-side before storing
(otherwise a hostile or buggy client stores raw URLs — the exact
`?flow=`/`?token=` material §2.3 exists to exclude — for 90 days); and
breadcrumb `detail` strings are validated for length but otherwise treated as
untrusted free text (see M5).

**M4. The message normalizer must redact email-like and token-like substrings,
and the group exemplar must be the normalized form (§2.7, §3.2, §3.3).** The
normalizer replaces UUIDs, hex ids, and integers — but a panic message that
interpolated an email address or a base64 token keeps it verbatim, and §3.3
stores the exemplar `message` on `error_groups`, which is retained *forever*.
Two changes: (a) add email-pattern and long-base64/hex-run redaction to the
normalizer (it is a pure function in `capsules` with tests, so this is cheap
and provable); (b) store the normalized message as the group exemplar — the
raw capped message lives only on the 90-day event row. With both, indefinite
aggregate retention is genuinely compatible with the privacy stance; without
them it is not. The design's proposed CLAUDE.md rule (panic/`expect` messages
never interpolate user or device data) is right and should ship with Phase 1,
but a rule about future code is a hope, not a control — the normalizer is the
control.

**M5. Every stored report field is hostile input to two downstream renderers
(§6.1, §6.2).** `message`, `stack`, `user_agent`, `report_note`, and `route`
are attacker-controlled strings that will be embedded in (a) ops emails —
strip newlines and control characters before putting the message excerpt in
the `[ERROR] New: …` subject, and remember a crafted message can be
phishing-shaped text aimed at the operator reading the email — and (b) the
future support panel (task #30), which must render them as text, never
markup. One sentence each in the design now is much cheaper than a retrofit.

**M6. The crash screen's "error id" join is undefined (§5.1, §3.3).**
`sendBeacon` cannot read a response, so the id shown on the crash panel must
be client-generated — but `error_events` has no column for it; the
server-generated `id` UUID cannot serve. Add a `client_event_id` (client-
minted UUID, shape-validated) that the panic beacon carries and the follow-up
note references. Treat it as a correlation hint, not a key — ids can be
reused by an attacker, so the note flow should attach, not overwrite.

### Low

**L1. The 15 s boot watchdog will false-positive on slow links (§2.1).** The
wasm is 4.9 MB raw; slow-3G users can legitimately exceed 15 s, producing junk
class-B reports and a crash panel over a page that was about to work. Gate the
watchdog on evidence the download finished (Resource Timing / readyState)
rather than wall-clock alone, or lengthen it, and tag watchdog-originated
reports distinctly so they can be discounted in review.

**L2. Filter unactionable foreign errors client-side (§2.1).** Cross-origin
scripts surface as bare `"Script error."` with no detail, and browser
extensions throw from `chrome-extension://` frames. Both create junk groups —
and, per H1, junk emails. Drop them in the shim before sending.

**L3. `ensure_error_tables` on the hot unauthenticated path (§3.3).** The
ensure-tables convention runs DDL per request; on an abuse-facing route that
is an extra Postgres round trip an attacker controls the volume of. Run it
once per isolate (a `static` flag) or only on insert failure.

**L4. Doc/state drift and one deploy-time verification.** (a) §1.7 says 367
`console_error!` sites, §4.1 says "~100"; the current count is 386 — pick one
number or say "hundreds". (b) §4.1 prescribes the flip "all three env
blocks"; the committed edit is the single top-level `[observability.logs]`
block (`wrangler.toml:11-17`). Whether `[env.staging]` inherits observability
should be verified on the next staging deploy (dashboard shows it per script)
— if it does not inherit, staging still retains nothing and needs its own
block. (c) §8 decisions 1, 2, 3, and 5 are now made; record them in the doc
so it stops presenting settled questions as open.

### Info

**I1. The crash panel is a report affordance on public pages — deliberately,
I assume, but make it a decision.** The hidden panel (with its "Tell us what
happened" box) is injected into every prerendered page, marketing pages
included, while the owner's decision scopes the *floating button* to authed
routes. I read the decision as governing the persistent affordance, not the
crash screen — a crash screen without a report box is strictly worse, and
under the identity policy below an anonymous visitor's note stores no
identity. Flagging so the wider surface is chosen rather than accidental.

---

## Identity policy: recommendation

**Adopt the split, sharpened into the transport layer.** Precisely:

- **Automatic reports** (`RustPanic`, `WasmBoot`, `JsException`,
  `UnhandledRejection`, `ApiFailure`, and ErrorBoundary captures) are
  **anonymous by construction**. Stored: signature, kind, normalized message,
  location, route template, build, capped user agent, breadcrumbs, the
  1-bit `session_kind`, and timestamps. `user_id` is always NULL; the handler
  never calls `require_auth_session` for these kinds — the resolution code
  lives only inside the manual branch, so there is no code path that *can*
  read the cookie.
- **Manual reports** (the crash-panel note, and the floating-button flow via
  `POST /feedback`) are **identified**, because the submitter is asking to be
  contacted and helped. Identity is server-resolved from the session, never
  taken from the body (the `FeedbackSubmitter` precedent), stored as
  `user_id` + `report_note` on the event row, and erased on account deletion.
- **Enforcement is H2's rule:** identified manual submissions are
  `application/json` only (preflighted, CORS-gated to `ROOT_URL`);
  `text/plain` is always treated as anonymous-automatic regardless of any
  kind claimed in the body. Make the policy structural in the schema too:
  `CHECK (user_id IS NULL OR report_note IS NOT NULL)`.
- **The sendBeacon cookie question:** acceptable as scoped. The Beacon spec
  forces credentials-include and offers no opt-out, so the cookie transits on
  the panic path; transit to our own first-party API over TLS is not
  collection, and the automatic branch is cookie-blind by code structure.
  Everywhere fancier controls the call — the JS shim's and ErrorBoundary's
  `fetch(..., keepalive)` paths — set `credentials: 'omit'` so the cookie
  never even leaves the browser. That makes the anonymous claim true on the
  wire for every path where we have a choice, and true by construction on the
  one path where we don't.

**Does anonymity weaken debugging? Not materially.** (i) When a user writes
in, `received_at` + `build` + route template + UA isolates their event at this
platform's volume — support asks "roughly when, which page", which they ask
anyway. (ii) When the user reports through the crash panel or error card, the
`client_event_id` (M6) joins their identified words to their anonymous crash
exactly — consented, precise, and better than ambient identity because the
user initiated it. (iii) The panic location and breadcrumbs debug the *bug*
without needing to know *who* — the design's own §2.1 argues location is the
grouping key, and no fix in this codebase's history needed to know which user
triggered the panic.

The design's §2.2/§2.7 text ("dovecote can therefore attribute the report to
a real user server-side… that is both the better engineering answer and the
better privacy answer") should be rewritten to this policy: it was the better
answer *than client-sent identity*, but resolving identity on automatic
reports at all is collection the platform does not need.

---

## Retention sanity check (90 d / 200 per signature / groups forever)

**Keep it — conditional on M1, M2, and M4.** With automatic events anonymous,
90 days of purely technical rows is comfortable, and 200 exemplars per
signature is a read-volume cap, not a privacy surface. Groups-forever is fine
*only* once the exemplar is the normalized, redacted message (M4) — as drafted
it would retain raw panic text indefinitely, which is the one place the
current design conflicts with its own §2.7. Two additions:

- **Erasure hook:** the privacy page already commits to manual account
  deletion via email; that runbook (and any future automated flow) must
  include `DELETE FROM error_events WHERE user_id = $1` (or null out
  `user_id`/`report_note`). Without it, "manual reports carry identity" plus
  90-day retention leaves identified rows orphaned past the account.
- **Optional tightening, not required:** if the owner wants a stricter
  posture, the cheap dial is 30 days for identified rows
  (`user_id IS NOT NULL`) and 90 for anonymous ones — identified rows are
  support tickets, and a ticket untouched for a month is dead. I would ship
  90/90 and revisit with real data.

---

## `console_error!` / `console_log!` audit (gates the sampling deploy)

386 sites reviewed (pattern sweep for credential/PII/body interpolation, plus
targeted reads of every hit). **Four production-firing sites log data the
owner's mandate says to avoid; they should land before or with the sampling
deploy.** The flip is already committed, so if it deploys first, fix forward
immediately — Workers Logs' 7-day retention means exposure self-heals a week
after the fix ships.

Production-firing — fix before/with the deploy:

1. `dovecote/src/helpers/alerts.rs:1161` — `useSend accepted mail to {to}
   (subject: {subject})` logs the recipient's email address on **every
   successful send** (alerts, invites, billing warnings — customer
   addresses, not just the ops inbox). Added in `bb8d951` for delivery
   visibility, which is worth keeping — log the alert definition id / owner
   kind+id / a redacted address instead of the address.
2. `dovecote/src/helpers/alerts.rs:1153` — same interpolation on the failure
   path (`useSend send to {to} returned HTTP {status}`).
3. `dovecote/src/helpers/orgs.rs:779` — invite send failure logs the
   recipient email. Log the org id and error only.
4. `dovecote/src/queue.rs:358-360` — logs the user-configured telemetry
   `endpoint.url` on forward failure. Users can and do embed credentials in
   such URLs (userinfo, `?token=` query — InfluxDB-style endpoints
   especially). Log scheme+host or just the pigeon id.

Staging/dev-only (fires only where `OPS_ALERT_EMAIL`/`RESEND_API_KEY` are
unset) — fix opportunistically in the same pass:

5. `dovecote/src/helpers/feedback.rs:19-21` — logs the **full feedback email
   body** including resolved user id, email, and message when
   `OPS_ALERT_EMAIL` is unset. Log the subject and byte count.
6. `dovecote/src/helpers/orgs.rs:763-766` — logs the invite URL containing
   the **live single-use invite token** plus the recipient email when the
   transport is unconfigured. A credential in logs; the dev convenience can
   stay dev-only by gating on something dev-specific, or log the token's
   last 4 characters.
7. `dovecote/src/helpers/alerts.rs:1123-1125` — recipient email + subject
   when `RESEND_API_KEY` is unset.

Reviewed and acceptable: `helpers/auth.rs:34` (Kratos error debug — no cookie
or identity content); `lib.rs:3837` (logs the secret's *name*, a const, not
its value); `lib.rs:804-805` (logs the CF-Connecting-IP of a *denied* caller
to the locked internal PSK route — necessary abuse logging, covered by the
privacy page's existing web-logs disclosure); user-id interpolations
(`lib.rs:866`, `helpers/usage.rs:354` and similar) — pseudonymous ids needed
for debugging, consistent with the web-logs clause; `helpers/pigeons.rs:100,
205,297` (a `Headers::set` failure could theoretically echo an Authorization
value, but is unreachable for well-formed tokens — no action). No site logs
request bodies, connection strings, PSKs, minted tokens, telemetry values, or
shadow contents.

---

## `/privacy/` diagnostics paragraph — DRAFT FOR OWNER APPROVAL

Written to the recommended final policy (automatic anonymous, manual
identified), styled to slot into `views/privacy.rs`'s "What we collect"
section after the "Web logs" paragraph, matching its "Lead word." convention.
No em dashes.

> Error diagnostics. If the dashboard hits a bug, your browser sends us a
> technical report: the error message, the place in our code where it
> happened, the app build, the page's route template, your browser's user
> agent string, and a short trail of recent in-app actions recorded as
> request method, route template, and status code. These reports are
> anonymous by design. They carry no account identity, no full URLs, no query
> strings, no form contents, and no request or response bodies, and we do not
> link them to your session. If you choose to send us a problem report
> yourself, we attach your account identity to that report so we can follow
> up with you, and identified reports are deleted with your account. Error
> reports are kept for 90 days; the long-lived statistics we keep about error
> patterns contain no personal data.

Every sentence is load-bearing against the implementation: "anonymous by
design" requires H2's structural split; "no full URLs, no query strings"
requires M3's server-side re-normalization; "deleted with your account"
requires the erasure hook; "contain no personal data" requires M4's
normalized-and-redacted group exemplar. If any of those is cut, the paragraph
must change with it.
