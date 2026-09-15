# Versioned Terms assent, and the four published legal pages

The assent flow the attorney's memo requires before billing goes live, and the four legal pages the
Terms and the Privacy Policy contractually point at.

**Mechanism in one line.** A signed-in browser cannot reach any dashboard route until dovecote says
the account has a `consent_events` row for `purpose = 'terms_of_service'` stamped with the current
`TERMS_VERSION`; the screen that enforces that is also the screen that writes the row; registration
carries the same checkbox as notice and as a first affirmative act; and checkout refuses to reach
Stripe unless a current-version row is already on file, then records its own row naming the
organization being bound.

**What this needs from Kratos: nothing.** No identity-schema change, no `kratos.yml` change, no new
jsonnet hook, no new Worker secret, nothing for the owner to hand-apply on the VPS.

---

## 1. Why the gate is the mechanism

The memo asks for three things before we rely on the liability cap, the forum clause, the jury
waiver or the incorporated DPA: an affirmative act, reasonable notice, and a server-side record of
version, account and time. It does not say where the act happens, and putting it only at
registration has two costs:

- Registration is the one surface we do not own. Kratos renders it, so a server-enforced tick is an
  identity-schema change (`required` plus `"const": true`), which `docs/consent.md:282-301` names as
  the genuinely breaking case: a new schema id `user_v2` plus a per-identity `PATCH`, never an
  import, because an import re-mints identity UUIDs and `flocks.user_id` and every Durable Object's
  `pigeon_acl` key ride on the current ones. It also renders the trait on the settings profile form
  forever.
- Registration cannot cover existing accounts, and the memo says existing accounts need a path. One
  gate covers both populations with one mechanism.

The second property that falls out: because the gate is the enforcement, the registration checkbox
does not have to be unbypassable. A browser that ignores `inert`, or a script that POSTs to Kratos
directly, still meets the gate on first dashboard entry. Nothing is lost.

---

## 2. The record

### 2.1 Where it lives

`consent_events`, under a new `purpose`. That is what the column was put there for
(`infra/migrations/2026-08-27-consent-events.sql:51-57`: "adding one stays a code change"),
`purpose` carries no CHECK, and the existing `(identity_id, purpose, seq DESC)` index already serves
a second purpose with no schema change. A separate table would duplicate nine columns, a lazy
ensure, a migration and an erasure statement to buy a name.

```
purpose        = 'terms_of_service'      (capsules::consent::TERMS_OF_SERVICE_PURPOSE)
kind           = 'granted'               (an assent is never withdrawn; a later version supersedes)
notice_version = capsules::TERMS_VERSION (stamped server-side, never from the caller)
source         = 'gate' | 'checkout'     (two new ConsentSource variants)
identity_id    = the Kratos identity resolved from the session cookie
at             = the column default now()
ip, user_agent = NULL, as on a marketing row: the notice describes neither for this record
org_id         = the organization being bound, on a checkout row only (new nullable column)
flow_id        = always NULL (no Kratos flow stands behind either surface)
```

Version, account and time, which is what the memo asks for, are all written by the server. The
client supplies no version, no timestamp and no source, so no row can misdescribe the surface it
came from.

`org_id` is kept. The settled Terms extract a representation that an identity-only row
cannot reconstruct: "If you subscribe on behalf of a separate business or public body, you confirm
that you are authorized to bind it" (terms-of-service-final-2026-09-11.md:13). An abandoned checkout
leaves no Stripe object to recover it from.

### 2.2 Two schema changes, both idempotent, one of them dangerous

`source` is CHECK-constrained in three places (`infra/init-db.sql:493`,
`infra/migrations/2026-08-27-consent-events.sql:66`, `dovecote/src/helpers/consent.rs:39`), and
`CREATE TABLE IF NOT EXISTS` is inert against a table that already exists. Without an explicit
`ALTER`, the first `gate` insert fails at runtime on staging and production while passing every
local test against a fresh database. This is the one change in the task that can strand every
signed-in user.

The repair must be conditional, because a blind DROP/ADD on every isolate boot takes an
ACCESS EXCLUSIVE lock and revalidates the table, and it must discover the constraint's real name
rather than assume the Postgres default:

```sql
ALTER TABLE consent_events ADD COLUMN IF NOT EXISTS org_id UUID;

-- Widens the source CHECK once. The loop finds nothing after the first run,
-- so a warm isolate never takes the table's exclusive lock, and the real
-- constraint name is discovered rather than assumed.
DO $$
DECLARE c record;
BEGIN
  FOR c IN
    SELECT conname FROM pg_constraint
     WHERE conrelid = 'consent_events'::regclass AND contype = 'c'
       AND pg_get_constraintdef(oid) LIKE '%source%'
       AND pg_get_constraintdef(oid) NOT LIKE '%gate%'
  LOOP
    EXECUTE format('ALTER TABLE consent_events DROP CONSTRAINT %I', c.conname);
    EXECUTE 'ALTER TABLE consent_events ADD CONSTRAINT consent_events_source_check
             CHECK (source IN (''registration'',''settings'',''import'',''gate'',''checkout''))';
  END LOOP;
END $$;
```

The same two statements go into the new migration, into `infra/init-db.sql`, and into
`ensure_consent_tables` (`dovecote/src/helpers/consent.rs:31-56`), whose own comment requires it to
stay statement-equivalent with the migration. Belt (migration at deploy) and suspenders (the lazy
ensure heals a database the migration was not run against).

### 2.3 The writer

`record_consent_event` is left alone. The marketing path keeps hard-coding its own purpose and its
own version, which is the property that makes a terms row impossible to stamp with the privacy date,
and leaving it untouched means nothing on the live marketing path has to be re-verified. A sibling
in the same module does the same job for assent:

```rust
/// Appends a Terms assent. Purpose and version are hard-coded here for the
/// same reason the marketing writer hard-codes its own: a caller that could
/// pass either pair could stamp a terms row with the privacy date.
pub async fn record_terms_assent(
  client: &Client,
  identity_id: Uuid,
  source: ConsentSource,
  org_id: Option<Uuid>,
) -> Result<Option<i64>>
```

Two statements, picked by a `match` on `source`:

- `ConsentSource::Checkout` always appends. A checkout assent names an organization and carries the
  authority-to-bind representation, so it is a distinct act and must never be suppressed by an
  earlier gate row for the same version.
- everything else appends only when no `granted` row exists yet for this identity, purpose and
  version:

```sql
INSERT INTO consent_events
  (identity_id, purpose, kind, source, notice_version, org_id)
SELECT $1, $2, 'granted', $3, $4, $5, $6, $7
 WHERE NOT EXISTS (
   SELECT 1 FROM consent_events e
    WHERE e.identity_id = $1 AND e.purpose = $2
      AND e.notice_version = $4 AND e.kind = 'granted')
RETURNING seq;
```

Decision and write are one statement, for the reason the existing writer gives: two tabs clicking
Accept at the same moment would otherwise both read "nothing on file" and both append.

`capsules::consent::consent_transition` is **not** reused and the existing
`WHERE $3 <> COALESCE(...)` predicate is **not** touched. That predicate suppresses a second
`granted` row for the same identity and purpose regardless of version, which is exactly the shape of
assent to a new version; threading a version-scoped bind through it would be a mode flag on the live
marketing path.

The `clamp` closure that bounds `ip` and `user_agent` is lifted out of `record_consent_event` into a
private `clamp_context`, so the marketing writer keeps one copy of the bound if the notice ever
covers the columns for assent rows too.

### 2.4 The reader

```rust
/// The account's most recent Terms assent, or `None` if it has never given one.
///
/// Anchored on `now()`: Hyperdrive will not cache a statement carrying a
/// volatile function, and this read gates a screen the user has just cleared.
/// Without it the gate reappears for up to a minute after a successful accept.
pub async fn load_terms_assent(client: &Client, identity_id: &Uuid)
  -> Result<Option<(String, OffsetDateTime)>>
```

```sql
SELECT notice_version, at, now() AS read_at
  FROM consent_events
 WHERE identity_id = $1 AND purpose = $2 AND kind = 'granted'
 ORDER BY seq DESC LIMIT 1;
```

`read_at` is never read back; it is there to defeat the query cache, the same device
`load_dashboard_state` (`dovecote/src/helpers/dashboard_state.rs:45-55`) and
`load_org_billing_state` use, and load-bearing for the same reason. `[env.dev]`'s Hyperdrive binding
is a `localConnectionString` with no query cache, so dev cannot reproduce the failure this prevents.
It is a staging check, not a local one.

No `*Row` twin is needed: dovecote reads these columns with typed getters through `query_typed`,
not through serde.

---

## 3. The two routes

Both sit on the public router beside the existing dashboard routes, both authenticate with
`require_auth_session` (`dovecote/src/lib.rs:252`), both build their own `build_cors(&ctx.env,
&req)` and `.with_cors(&cors)` every response, both use `let ... else { return ... }` and no `?`.

### `GET /account/terms`
**Auth:** session. Answers `capsules::consent::TermsAssentStatus`:

```json
{"current_version": "2026-09-14", "accepted_version": "2026-09-04",
 "accepted_at": "2026-09-04T11:02:31Z"}
```

`current_version` rides the wire rather than being read from fancier's own compiled constant,
because fancier and dovecote deploy separately: a gate keyed on fancier's constant would ask for
assent to a version dovecote would not stamp. `is_current()` is a capsules method with a unit test.

### `POST /account/terms`
**Auth:** session. Empty body, deliberately: `ConsentSource`'s own doc says each value names a
surface whose wording we can produce, and letting a client name its surface would let it lie about
one. Records `source: 'gate'`, `org_id: NULL`, and answers the same `TermsAssentStatus` the GET
would now return, built from the write rather than from a re-read. That is the client half of the
Hyperdrive rule: a mutation's own 2xx body is the only read guaranteed to reflect the write, so
fancier never refetches to confirm a save.

**Neither route ever answers 401 for anything but a session that no longer resolves.** 401 is the
cross-cutting sign-out signal (`fancier/src/api/helpers.rs:11,62`), so a 401 from a new route for
any other reason silently signs dashboard users out. Everything else is 400, 403, 409 or 500.

---

## 4. The three surfaces

### 4.1 First dashboard entry: the gate

The gate goes in `AuthGuard`'s `Authenticated` arm (`fancier/src/lib.rs:182-193`). That is one choke
point covering `/dashboard`, `/flocks`, both pigeon routes, `/orgs`, `/orgs/:id`, `/session` and
`/settings`, and it is SSG-safe by construction: the prerender pass never leaves
`AuthState::Pending`, so the prerendered auth-gated pages still contain exactly "Verifying
session..." and nothing about the Terms.

```rust
#[component]
fn AuthGuard() -> Element {
  let session = use_context::<Session>();
  // Hoisted with the navigator for the same reason it is: a hook called from
  // only one arm would shift this scope's hook indices as the state resolves.
  let nav = use_navigator();
  let assent = use_signal(|| None::<TermsAssentStatus>);

  use_resource(move || async move {
    if (session.state)() == AuthState::Authenticated && assent.read().is_none() {
      if let Some(status) = api::terms::status().await {
        assent.set(Some(status));
      }
    }
  });

  match (session.state)() {
    AuthState::Authenticated => {
      // Unknown renders the app. A status we could not read must never be
      // the thing that locks an account out of its own fleet.
      let blocked = assent.read().as_ref().is_some_and(|s| !s.is_current());
      if blocked { rsx! { TermsGate { assent } } } else { rsx! { Outlet::<Route> {} } }
    }
    // the two existing arms unchanged
  }
}
```

That is the whole client state: one `Signal<Option<TermsAssentStatus>>` in the layout. Nothing in
`LocalSession`, nothing in context, nothing in `localStorage`. The resource lives in the layout
rather than in the gate component so it fires once per sign-in, not once per route change.

`TermsGate` is a full-screen panel, not a modal: what changed and why it is being asked; `Link`s to
`/terms/`, `/privacy/`, `/dpa/` and `/subprocessors/`, all public routes outside `AuthGuard` and so
reachable while gated; the required checkbox carrying `capsules::consent::TERMS_ASSENT_LABEL`; an
"Agree and continue" button disabled until it is ticked; and `OryLogOut` so someone who declines can
leave. Not dismissable, no "later". On a 2xx it sets `assent` from the response body and the guard
renders the `Outlet` on the next render. On a failure it shows an inline error and stays up; nothing
is cached, so a retry is a fresh POST.

**No route is exempt, `/settings` included.** Accepting is one click and needs no password, so a
completed account recovery landing on `/settings` through `kratos_settings_handoff`
(`fancier/src/helpers/session_start.rs:84-101`) meets the gate, accepts, and continues to the
password form with the URL unchanged. The clock to watch is Kratos's `privileged_session_max_age`,
15 minutes: the gate's own copy asks the person to read four documents, and every link leaves the
gate, so someone who reads them all meets a re-authentication prompt on the password form. Exempting
the settings handoff specifically is the escape hatch, on the condition `is_settings_handoff`
already computes.

### 4.2 Registration: notice plus a first affirmative act

`RegisterFlow`'s `Some(Ok(res))` arm already proves fancier markup with real `Link`s can sit beside
the Kratos form (`fancier/src/views/register.rs:92-99`). The notice goes above `FormBuilder`, and
the form is made non-interactive until the box is ticked:

```rust
let terms_ok = use_signal(|| false);
// inside the Some(Ok(res)) arm:
let credential_step = creates_the_identity(res.ui.nodes.iter().map(|node| node.group));
TermsNotice { accepted: credential_step.then_some(terms_ok) }
div {
  // `inert` is the whole enforcement, and it is allowed to be only a
  // browser behaviour: the account meets the gate on its first dashboard
  // entry either way.
  "inert": (credential_step && !terms_ok()).then_some(""),
  class: if credential_step && !terms_ok() { "opacity-60" } else { "" },
  FormBuilder { ui: *res.ui.to_owned() }
}
```

No Kratos node, no trait, no schema change, no `checkbox_helper` arm, no jsonnet. The tick writes
nothing: there is no identity to key a row on at the moment it is made, and the row the product
relies on is the gate's, written against an authenticated session with the server's own clock,
address or user agent. Registration's job here is notice at the moment of account creation and an
affirmative first act.

SSG-safe for free: the arm renders only after `register.rs:12`'s `use_resource` resolves, and the
prerendered page is the `None` arm ("Loading registration flow..."). No query param is read, so
neither hydration trap applies.

Registration is two-step (`schemas/kratos/kratos.yml:139-152`) and the step transition is a full
page load, so a tick on the first step is gone by the second. The notice renders on both steps and
the box appears on the credential step only, which is the step that creates the account and the one
whose nodes fall outside the `default` and `profile` groups. A new account is therefore asked once
here and once at the gate.
A `localStorage` handoff that posted the assent once the session was adopted would remove that
second ask; it is not built, because it adds a key, a TTL, a failure branch and a client-claimed
source to save one click, and the gate copy reads as confirmation rather than as a repeat.

### 4.3 Checkout: refuse without a record, then record the entity

Server side, in `POST /orgs/:org_id/billing/checkout` (`dovecote/src/lib.rs:4760`), after the
manager check and before `load_org_billing_state`, on the Postgres client the handler already has
open. Placing it there keeps every Stripe call downstream, so a refusal happens before any Stripe
Customer or Session object exists.

1. `load_terms_assent` for `auth.user_id`. Resolved and not current: refuse **409** with a body
   naming the version required, because the caller is authorised and the state is wrong; never 401,
   which would sign the tab out. A read that failed is a **500**: "your state is wrong" is a claim
   we cannot make when we could not read the state. We do not take money against terms we cannot
   show were accepted.
2. Current: `record_terms_assent(..., ConsentSource::Checkout, Some(org_id))`, then
   continue. One INSERT on the open client, no extra round trip.
3. `auth.user_id` that will not parse as a UUID is a 500, not a 400: a session that resolved and
   whose id will not parse is our bug, matching `dovecote/src/lib.rs:3738`.

Client side, **a notice line beside the purchase button and no second checkbox**, rather than a
client-supplied `accepted_terms` bool the server would have to trust. One line above each of the
three call sites, rendered from one capsules constant:

> Subscribing accepts the [Terms of Service](/terms/) dated {TERMS_VERSION}, including the
> [Data Processing Agreement](/dpa/) they incorporate.

- `fancier/src/views/pricing.rs:136`, the single-org direct path.
- `fancier/src/views/pricing.rs:258`, the `UpgradeOrgCreate` modal, whose one-field design
  (`pricing.rs:179-186`) is preserved: a notice line is not a field.
- `fancier/src/views/org.rs:768`, the org page's upgrade button, the only live route to checkout
  while `BILLING_LIVE = false`.

`capsules::BillingCheckoutRequest` is unchanged, so this commit does not span three crates.
`api::billing::checkout` already calls `fetch_json_any_status`, so a 409 is distinguishable by
status with no new plumbing: it renders the server's message inline beside the button, telling the
person to reload to review the new Terms. Reloading brings the gate up, because the gate's resource
refetches on a fresh sign-in state. No shared signal, no new state.

Stripe's own `consent_collection[terms_of_service]` is not added: it records on
Stripe's session object rather than in our database, reaches us only through the webhook after
completion, and would change the test-pinned parameter set
(`dovecote/src/helpers/stripe_api.rs:630-655`).

---

## 5. Versions, and the "Last updated" line

### 5.1 Two constants

```rust
/// The date the published Terms of Service last changed, ISO 8601 so it sorts.
///
/// It feeds the Terms page's own "Last updated" line, the DPA and sub-processor
/// list published with them, and every Terms assent we record. Bumping it is
/// the decision that a version needs fresh assent: every account is asked again
/// on its next sign-in, so a wording fix that needs no re-assent must not move it.
pub const TERMS_VERSION: &str = "2026-09-XX";
```

In `capsules/src/lib.rs` directly under `PRIVACY_NOTICE_VERSION` (`:89`), matching that constant's
stated reason: one string that the page renders and the row stamps, in the crate both other crates
depend on, so the two cannot name different documents. **No wrangler var**: a Worker var reaches
dovecote only, and the page could then disagree with the rows, which is the exact failure the
constant exists to prevent.

One constant covers Terms, DPA and sub-processor list. The DPA forms part of the Terms and the
sub-processor list forms part of the DPA, and all three ship in one deployment. The Privacy Policy
renders `PRIVACY_NOTICE_VERSION`, bumped to the same deploy date. If the
DPA ever changes on its own under its Section 12.2 while the Terms stand still, split then: a
`DPA_VERSION` constant and nothing else moves. Adding it now gives the owner two dates to keep in
step at every deploy and buys nothing.

A capsules unit test asserts both constants match `^\d{4}-\d{2}-\d{2}$`. No such test exists today
and a malformed value would ship silently into every row.

### 5.2 One token, two substituters

Each of the four in-repo documents carries the literal token `{{LAST_UPDATED}}` on the line the
settled documents spell `Last updated: [the date these terms deploy]`. One token in all four beats
rendering a constant into two and hand-editing the other two, which would leave the DPA's date as an
unchecked hand edit at every deploy.

- **HTML page.** `helpers/legal_doc.rs::render(src, version)` does one `str::replace` before
  parsing, inside the `use_memo` that already runs once per mount. Terms, DPA and sub-processors
  pass `TERMS_VERSION`; Privacy passes `PRIVACY_NOTICE_VERSION`.
- **Markdown variant.** `fancier/scripts/build-release.sh` reads the constants out of
  `capsules/src/lib.rs` and substitutes during the copy, in the same block that already copies
  `docs/api.md` to `api-reference/index.md` (`build-release.sh:196-205`):

```sh
read_const() {  # $1 = constant name
  local v
  v="$(sed -n "s/^pub const $1: \&str = \"\\(.*\\)\";\$/\\1/p" ../capsules/src/lib.rs)"
  [ -n "$v" ] || { echo "build-release: $1 not found in capsules/src/lib.rs" >&2; exit 1; }
  printf '%s' "$v"
}
```

A missing constant fails the build rather than publishing a page with a raw token on it, and a final
grep gate fails the build if `{{` survives in any published `.md`. The python stub loop later in the
same script skips any `index.md` that already exists, so these four keep their real prose the way
`/` and `/api-reference/` already do.

One fancier test pins it: for each of the four `include_str!`'d documents the token occurs exactly
once, and `render`'s output contains no `{{` at all. That catches a dropped token, a duplicated one,
and a renderer that stops substituting.

### 5.3 Deploy ordering, which the constant forces

fancier carries the pages. dovecote carries `current_version` and stamps the rows. On a version bump
they go in one order: **fancier first, then dovecote.**

If dovecote goes first it answers `current_version: v2` while `/terms/` still renders v1, the gate
fires, and every row written in that window says the account accepted v2 while being shown v1. That
is a false record, and it is the one outcome this whole stream exists to prevent. The other order is
benign: the pages show v2 for a few minutes while dovecote still considers v1 current, so no gate
fires and no row is written against text nobody could read. The rule lives in `docs/legal/README.md`
beside the two constants, and in `docs/consent.md`'s version-bump runbook.

---

## 6. What an attacker-free failure looks like

Nobody is attacking this. The failures that matter are a Postgres blip, a query cache, a dead
isolate, a double click and a half-finished deploy. The rule: **a lost record fails closed for
reliance and open for the product.** We may lose the right to invoke the cap against one account for
one session. We may not lock an account out of its own fleet.

| Failure | Behaviour | Why |
| --- | --- | --- |
| The API calls a version current that this build does not publish | The panel says the Terms are being published and offers no accept button | Half a deploy. Accepting would write a row naming text this page is not showing |
| `GET /account/terms` has not answered yet | The guard shows its "Verifying session..." placeholder | A gate that appears over a dashboard already on screen is not a gate |
| `GET /account/terms` fails (network, 500, table missing) | `assent` stays `None` but the read is marked done, the guard renders the `Outlet`, the dashboard works | An unreadable status is not evidence that assent is missing. The next sign-in asks again. |
| `GET /account/terms` returns a stale `accepted_version` | Cannot happen: the statement carries `now()` and Hyperdrive will not cache it | The documented failure mode is the gate reappearing after a successful accept |
| `POST /account/terms` fails | Panel stays up with an inline error, nothing cached, a retry is a fresh POST | The person is held out of the dashboard but not out of `/terms/`, `/privacy/` or sign-out |
| Two tabs accept at once | One row: the `NOT EXISTS` predicate is inside the INSERT, not a read-then-write | Same reasoning as the existing marketing writer |
| An account reaches the dashboard with no row because the read failed | Product works, no row exists, we do not rely on the cap for that session | Exactly the trade the memo's "before relying on" language permits |
| Checkout's assent write fails | 500 before any Stripe object exists: no subscription, no charge | The opposite call on purpose. Proceeding would create a paying customer with no record. |
| Checkout with no current-version row | 409 naming the version, before any Stripe call | We do not take money against terms we cannot show were accepted |
| `source` CHECK not widened on production | The first accept 500s; the lazy `ensure_consent_tables` widens it on that same request path and the retry succeeds | The only failure that could strand every user, which is why it has a migration, a lazy ensure and a staging rehearsal |
| `inert` unsupported in some browser | The registration form is usable without the tick; the gate catches the account | This is the point of gate-first |
| `KRATOS_HOOK_SECRET` unset in an environment | Irrelevant to this stream | Nothing here goes through a Kratos hook. It still matters for the marketing rows (section 9.3). |
| dovecote deployed before fancier on a bump | The gate would ask for a version whose text is not published | Prevented by the deploy order, section 5.3 |
| A bug in `is_current()` walls off every account | Redeploy the previous fancier version: the gate is client-side and dovecote needs no change | The kill switch, section 11 step 8 |

The one thing that must never happen quietly is a row saying someone accepted a document they were
not shown. Three controls prevent it: the version is stamped server-side and never accepted from a
caller; the gate compares against the server's `current_version`, not fancier's compiled one; and
fancier deploys before dovecote.

---

## 7. The four legal pages

### 7.1 Sources and renderer

Four in-repo copies under `docs/legal/`: `terms.md`, `privacy.md`, `dpa.md`, `subprocessors.md`.
Repo root `docs/` rather than `fancier/assets/`, matching `docs/api.md`: these are published
documents counsel and the business folder both reference, and `build-release.sh` already copies from
`../docs/`. `docs/legal/README.md` records where each came from, that the copy is one way (business
folder to repo, never back), the two constants, the deploy order and the rollback. There is no
byte-comparison test, because the business copies are the attorney's working drafts and are meant to
move ahead of the published ones.

Renderer: a new `fancier/src/helpers/legal_doc.rs` with one function.

```rust
/// Renders a published legal document, substituting the "Last updated" token.
/// Headings get GitHub-rule ids so a clause can be linked to; tables get the
/// horizontal-scroll wrapper. No `<details>` folding and no route scraping:
/// those are API-reference affordances, and collapsible clauses on a contract
/// are wrong.
pub fn render(src: &str, version: &str) -> String
```

It reuses `api_doc`'s `slugify` (already `pub`) and raises `Slugger` to `pub(crate)` rather than
copying them, and uses `pulldown_cmark` with `ENABLE_TABLES | ENABLE_STRIKETHROUGH |
ENABLE_FOOTNOTES` (already a fancier dependency; tables are an `Options` flag, not a cargo feature,
so no dependency change). It does not call `api_doc::render`, which wraps every H2 in
`<details open class="api-surface">`.

No contents sidebar and no fragment-click JavaScript. The four documents contain no in-document
`](#...)` links, so the capture-phase anchor handling `api_reference.rs` needs has nothing to do
here, and with no sticky element these pages do not need the `main:has(...) { overflow-x: clip; }`
fix either.

One shared view component:

```rust
#[component]
fn LegalDocument(section_id: &'static str, title: &'static str,
                 version: &'static str, source: &'static str) -> Element
```

renders `section { id: section_id }`, the H1, a provenance line matching `api_reference.rs:253-255`,
and `div { id: "legal-md", dangerous_inner_html: "{MARKDOWN_STYLE}{body}" }`. The "Last updated"
line comes from the document body itself now that the token is substituted there, so the page shell
does not print a second one.

`fancier/src/views/legal.rs` holds it plus `TermsPage`, `PrivacyPage`, `DpaPage` and
`SubprocessorsPage`, three lines each. `views/terms.rs` and `views/privacy.rs` are deleted, and
their `LegalSection`, `StorageItem` and `RetentionRow` components go with them.

Section ids: the outer `<section>` ids stay page-prefixed kebab-case per the convention
(`terms-of-service`, `privacy-policy`, `data-processing-agreement`, `subprocessors-list`). Heading
ids inside the body now come from the slug rule, so the seventeen hand-written `privacy-*` ids
become their GitHub slugs. Nothing in the repo links to them (grep returns zero inbound hits) and no
alias map is built; `docs/legal/README.md` records that those anchors retire. The Annex I/II/III
H1s in the DPA are demoted to H2 so each page
has one `<h1>`; no clause text and no section number moves.

### 7.2 The places that must stay in sync

Adding `/dpa/` and `/subprocessors/` touches every one of them, in the order a build would notice a
miss:

1. `fancier/src/lib.rs` route table: `#[route("/dpa/")] DpaPage {}` and
   `#[route("/subprocessors/")] SubprocessorsPage {}`, in the public block after `#[end_layout]`,
   trailing-slash form like every other public page.
2. `fancier/src/views/mod.rs`: `mod legal;` plus the four re-exports.
3. `fancier/src/helpers/page_meta.rs::public_routes()`: two entries, or
   `public_and_app_routes_cover_the_same_ground_as_the_json` fails.
4. `fancier/page-meta.json`: two entries inside the SEO bands (title 60 characters or fewer,
   description 120 to 160, or the release build exits).
5. `fancier/wrangler.toml` `run_worker_first`: `"/dpa"`, `"/dpa/"`, `"/subprocessors"`,
   `"/subprocessors/"`, both slash forms as the list's own comment requires.
6. `fancier/public/_headers`: a `rel="alternate" type="text/markdown"` Link block per new route,
   beside the existing `/privacy/` and `/terms/` blocks at `:87-90`.
7. `fancier/public/llms.txt`: a new `## Legal` section listing all four with their markdown
   variants. No legal page is listed there today.
8. `fancier/scripts/build-release.sh`: the four copies and the token substitution.
9. `fancier/src/lib.rs` router tests: two `both_forms!` lines.
10. `docs/seo-audit-guide.md:23`'s unenforced `PAGES` list.

`static_routes` needs nothing: `Route::static_routes()` drops only routes with dynamic segments, so
two new trailing-slash routes prerender automatically. That is worth stating because it is the one
place on this list that looks like it should need an edit and does not.

### 7.3 Tests

In `fancier/src/views/legal.rs`, modelled on `every_negotiable_route_place_knows_the_stories`
(`views/stories.rs:795-841`), reading each config file through `include_str!`:

- `no_draft_markers_or_bracketed_flags_publish`: none of the four documents contains `DRAFT`,
  `[OWNER` or `[LAWYER`. Three lines, and it is the only automated guard against the worst outcome
  available in this task, which is publishing the attorney's working draft to `pidgeiot.com/dpa/`.
- `every_legal_document_carries_exactly_one_last_updated_token`, plus `render` output containing no
  `{{`.
- `published_links_point_at_routes_that_exist`: every `https://pidgeiot.com/<path>` link in the four
  documents parses through `Route::from_str` to something that is not `PageNotFound`. Four settled
  sentences point at `/dpa/` and `/subprocessors/`; this is what stops the Terms' own contractual
  pointers shipping as 404s, and incorporation by reference depends on it.
- `every_negotiable_route_place_agrees`: generalised from the stories test to **every key in
  `page-meta.json`**: four new routes at once is when that gap bites, and it covers the existing
  fifteen routes for free. It keeps the stories globs as the one documented exception.
- In capsules: `version_constants_are_iso_dates`, and `is_current()`'s unit test.

All fancier tests run under `cargo test -p fancier --target x86_64-unknown-linux-gnu`; the crate's
config pins wasm32 and a wasm test binary cannot execute.

---

## 8. The interim `/dpa/` and `/subprocessors/` content

`docs/legal/dpa.md` is `24-eu-paperwork/dpa.md` with the edits map-dpa-content specifies;
`docs/legal/subprocessors.md` is `subprocessors.md` with its own. The business-folder copies are not
edited: they stay the attorney's working drafts, and her reviewed version replaces the published one
later under the DPA's own Section 12.2.

### 8.1 Mechanical, applied in the content commit

- The DRAFT block at `dpa.md:3` becomes one sentence: "**Status:** Counsel's substantive review of
  this document is pending; any change it produces is made under Section 12.2."
- `dpa.md:7`, the LAWYER note citing a superseded Terms memo, is deleted with one of its blank
  lines. Nothing replaces it.
- The `Last updated: {{LAST_UPDATED}}` line is added after the H1.
- Section 3.4 is rewritten so the no-training sentence reads "does not use it to train models for
  any purpose other than providing the Service to that Customer", gains the aggregated-statistics
  permission in the settled Terms' own words, and carves out an optional program the Customer
  affirmatively enrolls in. Section 4.1's instruction list gains enrollment as an instruction route.
  Without both halves the Terms' opt-in hook is dead for personal data.
- Section 10 is replaced with the provider-only cap that tracks the settled Terms: mutual exclusion
  of indirect damages, the Provider's total liability counted with the Agreement as one cap, and all
  four carve-outs (fees owed; gross negligence, fraud or willful misconduct; anything unlimitable;
  Article 82 and Clause 12 rights). It points at the Agreement's cap rather than restating a number.
- Section 11 loses ", by clicking through in the dashboard": no such surface exists or is planned,
  and the settled Terms name two routes.
- All 36 bracketed flags come out. The three structural ones are resolved rather than deleted: the
  backup-retention sentence ends at "under their normal rotation", which is what the settled Privacy
  Policy publishes; the OVH and useSend assurance bullets are filled in or removed rather than left
  as a colon with nothing after it.
- Notice recipients in 5.2, 6.2, 12.2 and 12.4 gain the settled Terms' fallback: "or, if your
  account is not associated with an organization, to the email address on your account". A free-tier
  customer can own devices with no organization.
- The four absolute claims about error reports are restated in the settled Policy's words
  ("de-identified by design", "no direct account identifier", exemplars grouped by error signature).
  The "device credentials are asymmetric" sentence in 9.6 is restated to cover the CoAP and MQTT
  pre-shared keys the platform stores. MQTT is added to A.3 and B.2; the string does not appear in
  the draft at all.
- Telemetry-history retention is restated as "by the plan the organization is served at". Flocks are
  added to the self-service deletion list. Device deletion says its telemetry history goes with it.
  Account deletion adds saved dashboard graphs and the product-update consent record.
- Annex II A.4's "Staging environments sit behind Cloudflare Access" is corrected or deleted: the
  gate is armed per deploy by a var and is inert without an Access application.
- Annex III's "available on request until that page is live" is replaced by a pointer to the
  published list and its date.
- **Section numbers are not changed.** The settled Terms cite "Section 8 of the Data Processing
  Agreement" by number. Insert, never renumber.
- Sub-processor list: the status line, the verification methodology paragraph, the Worker secret
  name and its contents, the local filesystem paths and evidence hashes, the repo path citations,
  all 14 flags, the two "for a lawyer's eye" analyses and the editorial instruction in the Resend
  row all come out. The useSend row is corrected: the edge provider's Email Service is the primary
  transport and useSend the fallback, which is what the code does and what the settled Policy says.
  The log-retention figure resolves to the 7 days the settled Policy publishes.

The DPA-versus-Privacy-Policy reconciliation above (error reports, device credentials, MQTT,
retention, notice fallback) is what keeps the two documents in one deployment from contradicting
each other, which is the class of defect memo item I.2 is about.

### 8.2 Not mechanical

One item blocks `/subprocessors/` on substance: the useSend row has no stated processing location
and no transfer mechanism, while DPA 6.4 warrants flow-down and 9.5 says the published list states
each vendor's mechanism. Publishing a contractual annex that names a live vendor with neither
publishes the gap to every customer and to any supervisory authority. It is the one decision that
blocks the page, and it is the owner's; the deploy checklist carries it.

The other eight open content questions publish under the conservative reading already in the draft,
and counsel's version replaces them under the DPA's own Section 12.2.

---

## 9. Kratos

### 9.1 Nothing changes

No `schemas/kratos/kratos.yml` change, no `schemas/kratos/identity.user.schema.json` change, no new
jsonnet hook body, no new Worker secret, nothing for the owner to hand-apply on the VPS.
`docs/consent.md:145-236`, the by-hand production apply, is untouched, and so is
`KRATOS_HOOK_SECRET` on both Workers. This is the largest operability win available in the task and
it is taken deliberately: the registration checkbox is fancier's own markup and the record is
written by an authenticated dovecote route.

### 9.2 What Kratos-side enforcement would cost

If the owner wants Kratos to enforce the tick server-side:

1. `schemas/kratos/identity.user.schema.json`: a `terms_accepted` boolean trait with `"const": true`
   added to `required`. `required` alone is not enough: JSON Schema `required` tests presence, not
   truth, and an unticked HTML checkbox is simply absent from the POST.
2. A new schema id. `docs/consent.md:282-301` names "adding a `required` entry nothing carries" as
   the one genuinely breaking schema change: `user_v2` registered alongside `user` in
   `identity.schemas`, and a per-identity `PATCH /admin/identities/<id>` moving each existing
   identity onto it and setting the trait. **Never an import**: an import mints new identity UUIDs.
3. `capsules::consent`: a `TERMS_ACCEPT_LABEL` constant plus a `schema_title_matches_label`-style
   test, because the checkbox label becomes the schema's `title`.
4. `ory_form_builder.rs`: a `checkbox_helper` arm for the new node name, and a decision about what
   the trait does on the settings profile form. The retired-node filter is not usable, because
   hiding a node is also what clears it.
5. If assent were also recorded by a Kratos hook rather than by our route, the per-method hook rule
   applies: Kratos picks ONE after-hook list per registration method and does not merge, so the
   entry must be listed under `registration.after.password.hooks` as well as the global list, and it
   must be first in the password list because `show_verification_ui` redirects. It would also
   inherit `response.ignore: true`, which is right for marketing (the trait still carries the
   choice) and wrong for assent (the row is the only evidence).

Owner's hand-apply for that variant: stop `kratos.service`, edit `/opt/kratos/kratos.yml` and the
identity schema file it points at (keeping the file 0640 under the static `kratos-conf` group, never
a group named `kratos`), `kratos validate`, `systemctl start kratos`, then the per-identity PATCH
loop against `127.0.0.1:4434` over loopback. That is the work this design exists to avoid.

### 9.3 Prerequisite queries, inherited rather than created

Three answers the owner should collect before the branch lands. None blocks this design; the third
decides whether the privacy archive is worth writing.

```sh
# On the VPS: has production Kratos ever received the consent hook and the trait?
grep -c 'consent-registration.jsonnet' /opt/kratos/kratos.yml   # expect 2 (global + password)
ls -l /opt/kratos/hooks/
# Locally: is the hook secret set on both Workers?
cd dovecote && bunx wrangler secret list
cd dovecote && bunx wrangler secret list --env staging
# Against the database: how many consent rows exist, against which versions?
psql "$DOVECOTE_PSQL_CONNECTION" -c \
  "SELECT purpose, notice_version, count(*) FROM consent_events GROUP BY 1,2 ORDER BY 1,2;"
```

If the first two come back empty, the marketing consent machinery is dev-only; Terms assent is
unaffected either way, because nothing here goes through Kratos.

---

## 10. Documentation changes

**`docs/api.md`** is test-enforced by `fancier/src/helpers/api_doc.rs`, so all of this lands in the
same commit as the routes or `cargo test -p fancier` fails: a `### Terms assent` group at the end of
`## Dashboard API` with two H4s spelled exactly `` `GET /account/terms` `` and
`` `POST /account/terms` ``, an `**Auth:** session` line under each, two "Routes at a glance" rows
at the matching index with a "what it does" cell longer than 10 characters, the checkout route's new
409, and a `## Type reference` bullet for `TermsAssentStatus` naming `TERMS_VERSION` as the stamped
version.

**`docs/consent.md`** is written as marketing consent end to end today. It gains a `## Terms assent`
section covering the gate and the checkout row as the two writing surfaces, why the transition rule
is not reused and what predicate replaces it, the two new `source` values and the conditional CHECK
repair, the `org_id` column, the retention difference against marketing rows, the version-bump
runbook with the deploy order, and the fact that no Kratos configuration is involved. The erasure
and subject-access statements gain `purpose` in their projection so they stay meaningful across two
purposes.

**`docs/legal/README.md`** (new): where each document came from, the one-way copy rule, the two
constants, the deploy order, the owner runbook of section 11 and the rollback.

---

## 11. The owner's runbook

Consolidated here because the owner is the person executing it. Nothing in it is an agent action.

### Deploy checklist: what the owner decides first

Everything the implementation could settle for itself, it settled, and the reasoning sits with the
mechanism it belongs to. What is left is owner and counsel work, and none of it is an engineering
call.

- **The useSend sub-processor row. BLOCKING.** The in-repo copy states that row as the deployed
  configuration stands, so the published list is accurate to what runs; what it states is a vendor
  with no stated processing location and no transfer mechanism, against the DPA's own 6.4 and 9.5.
  Each way out is an owner action taken before deploy -- remove useSend from the deployed
  configuration (the edge provider's Email Service is already the primary transport), obtain a DPA
  from the vendor, or switch the fallback rail -- and the row is then edited to match what was done.
- **The two constants' values.** `TERMS_VERSION` and `PRIVACY_NOTICE_VERSION` to the deploy date,
  one commit on the branch. The privacy constant is `2026-09-04` in the code while the settled
  policy is dated later, so leaving it would make the page and the existing rows disagree.
- **Whether the superseded privacy text is archived.** `docs/legal/archive/privacy-2026-09-04.md`
  exists so a row stamped with the old version can still be resolved back to the words on screen;
  the row-count query in section 9.3 decides whether production holds any such row.
- **The retention row for the assent record**, in the settled Privacy Policy, plus the sentence
  that we keep one. Until counsel adds them, every assent row is deleted with the identity and
  carries no address or user agent, which is what the published policy describes.
- **The eight open DPA content questions.** They publish under the conservative reading already in
  the draft, with the status line saying counsel's review is pending, and her version replaces
  them under the DPA's own Section 12.2. `map-dpa-content.md` sets each out.
- **Blocking existing accounts on day one, rather than 30 days' notice first.** The revised Terms
  promise 30 days' email notice for material changes to existing customers; a clause cannot govern
  its own adoption, and the superseded Terms promise only notice via the site, which publishing the
  page satisfies. There is no paying customer and the only thing blocked is the dashboard. A future
  bump should use an effective-date constant and a banner during the notice window rather than a
  wall on day one.
- **`BILLING_LIVE`.** Unchanged at `false` unless the owner says otherwise. It gates only the
  pricing CTA: the org page is already a live route to checkout, so the checkout notice and the 409
  ship regardless.

### The steps

1. **Set the constants**, one commit on the branch, once the decisions above are answered.
2. **Prerequisite queries**, section 9.3.
3. **Staging database.** `infra/migrations/2026-08-27-consent-events.sql` first, then
   `infra/migrations/2026-09-14-terms-assent.sql`, each
   `psql "$DOVECOTE_PSQL_CONNECTION_STAGING" -f ...` with its `SET ROLE` line edited to
   `dovecote_staging`. The 2026-09-14 file alters a table the 2026-08-27 file creates, and section
   9.3's first query is what says whether any deployed database ever received it; both are
   idempotent. Then `\d consent_events` to confirm the widened CHECK and the `org_id` column.
4. **Staging deploy, fancier first:** `cd fancier && bunx wrangler deploy --env staging`, then
   `cd dovecote && bunx wrangler deploy --env staging`.
5. **Staging smoke, signed in.** Sign in with the existing staging test identity: the gate appears,
   accept, and the dashboard renders without a refetch. **Reload within 30 seconds**, which is
   inside the Hyperdrive stale window, and confirm the gate does not return. A reload after the
   window expires proves nothing, which is the whole point of the `now()` anchor. Then:
   `SELECT purpose, notice_version, source, org_id, at
      FROM consent_events ORDER BY seq DESC LIMIT 3;`
6. **Production database:** the same two applies against `DOVECOTE_PSQL_CONNECTION`.
7. **Production deploy, fancier first, then dovecote.** Never the other order, section 5.3.
8. **Rollback, if the gate walls everyone off.** Redeploy the previous fancier version: the gate is
   client-side and dovecote needs no change, so the dashboard comes back without touching the
   database or the rows already written. Note it before you need it at 2am.
9. **Nothing on the Kratos VPS.**

### Live verification, before this is called done

Green builds are not evidence. Dev stack via `docker-compose -f infra/docker-compose.yml up -d` from
the main checkout, dovecote on 8787, the built artifact served by `wrangler dev` on port 8790 (not
`dx serve`, which does not hydrate; do not touch 4455).

1. Register a fresh account. The first step shows the notice with no box and submits normally; the
   password step shows the box and is inert until it is ticked. Tick it, register, reach the
   dashboard, meet the gate, accept. Expect exactly one `terms_of_service` row, `source = gate`,
   the current version, a NULL `ip` and `user_agent`.
2. Accept again (reload, click again): no second row, 200 either way.
3. **Bump `TERMS_VERSION` locally, restart, sign in.** Expect the gate again and a second row
   against the new version. This is the only check that exercises the version-scoped predicate, and
   the whole mechanism fails silently if that scoping is wrong.
4. **Stop Postgres. Sign in.** Expect the dashboard to render (fail open) and checkout to refuse
   (fail closed). **Stop dovecote. Register.** Expect the signup to complete and the gate on first
   sign-in. This is the only check that proves the fail-open branch, which is the only thing
   between a database blip and a total dashboard lockout.
5. Checkout end to end against the Stripe sandbox: the 409 with no row, the session and the
   `checkout` row carrying `org_id` with one.
6. `curl` each of `/terms/`, `/privacy/`, `/dpa/`, `/subprocessors/` with no JS: real prose in the
   HTML, the substituted date, no `{{LAST_UPDATED}}` anywhere.
7. `curl -H 'Accept: text/markdown'` each: `text/markdown; charset=utf-8`, `Vary: Accept`,
   `x-markdown-tokens`, the substituted date in the body. Repeat with a browser Accept string and
   confirm HTML comes back. `curl -I` each for the `_headers` Link block, and the no-slash form's
   307.
8. Confirm `public/dashboard/index.html` still contains only "Verifying session..." and no Terms
   wording, no version string and no account state.
9. Playwright headless Chromium hydration pass, zero console errors and zero page errors on `/`,
   `/terms/`, `/privacy/`, `/dpa/`, `/subprocessors/`, `/registration` and `/dashboard/`.
10. Both themes, and a 400px viewport with the PNGs actually opened: the global `main` overflow clip
    makes a `scrollWidth` check read green while text is cut off. The sub-processor tables are the
    risk, 70 table lines through `table-scroll` on a phone.
11. Then the staging pass in step 5 of the runbook, because dev's Hyperdrive has no query cache and
    a defect of the read-after-write class passes every local test and appears first on staging.

---

## 12. Risks

- **The Hyperdrive cache is the failure a green local run cannot catch.** The `now()` anchor on
  `load_terms_assent` is the only thing between this design and a gate that reappears for a minute
  after every accept, and `[env.dev]` cannot reproduce it. The staging probe must be inside the
  stale window, not after it.
- **The `source` CHECK on production.** Covered by a migration and by the lazy ensure, but it is
  still the one change that can strand every signed-in user, and it is invisible to every local test
  against a fresh database.
- **The gate is a single point of blocking for the whole dashboard.** It fails open on an unreadable
  status, which is the right call, but a bug that makes `is_current()` return `false` for everyone
  would wall off every account at once. The rollback in runbook step 8 is the answer, and the
  fail-open branch must be tested by killing Postgres, not assumed.
- **Two deploys, one version.** The pages and the rows are stamped by two separately deployed
  binaries. The ordering rule holds only if whoever bumps the constant follows it, and nothing
  enforces it mechanically.
- **The Privacy Policy does not yet describe this record.** The code takes the defaults that keep
  the deployment inside what the policy says: no address, no user agent, and erasure
  with the identity. What that costs is the evidence in exactly the case it was kept for, a
  dispute with someone who has since deleted their account, so one retention row in the settled
  policy is still worth asking counsel for.
- **The DPA we publish is ours, not counsel's.** Section 8's edits are engineering applying a
  content map to a legal document. The status line says the review is pending and the change clause
  covers replacement, but the interim text is text a customer can rely on from the day it deploys.
- **The second ask after registration** reads as a bug to a user who has just ticked a box. The
  `localStorage` handoff in section 4.2 is the escape hatch, and until it exists the gate copy has
  to do real work to explain itself.
- **Bundle size.** The four documents add roughly 108 KB of markdown `include_str!`'d into the wasm,
  against 455 KB already baked in; deleting the `terms.rs` and `privacy.rs` rsx recovers some of it.
  `/dpa/` prerenders to a large HTML file. Worth measuring once.
- **Heading ids change on `/privacy/`.** Seventeen shipped ids become their GitHub slugs. Nothing
  in-repo links to them, but the convention treats a shipped id as a public URL surface.
