# Marketing consent: the trait, the record, and how they are wired

Two things have to describe the same event: the words a person reads when they choose to
receive marketing email, and the record that shows they chose it. GDPR Article 7(1) puts the
burden of demonstrating consent on us, so a tick with nothing behind it is not consent we can
rely on.

This was built before there were users to backfill, which is the only time it is cheap.

The wording itself, and the reasoning behind each string, is owner-facing and lives in
`pidgeiot-business/eu-paperwork-2026-08/consent-wording.md`. This file is the engineering half:
what exists, why it is shaped this way, and what the owner has to apply by hand.

## The split

| | Where | Who writes it | What it is |
|---|---|---|---|
| **State** | `traits.marketing_emails` on the Kratos identity | the person, via the registration and settings forms | what they want now |
| **Evidence** | `consent_events` in Postgres | dovecote only, via `POST /internal/consent` | what they chose, when, against which notice |

The reason for the split is that the settings form hands the subject full control of their own
traits. An `at`/`source`/`notice_version` trait would be evidence the person it is evidence
against can rewrite, which is not evidence — and Kratos renders every property of an object
trait as its own form input (the existing `name` object is why settings shows First Name and
Last Name separately), so those three would have arrived as three editable boxes. The trait is
therefore one bare boolean, and everything that makes the event provable is server-side.

Both halves are declared in `capsules/src/consent.rs` — the label, the helper lines, and the
one rule (`consent_transition`) that decides whether a flow writes a row. They are in the
shared crate so the words on the form and the record behind them cannot move independently.
The notice version every row is stamped with is `capsules::PRIVACY_NOTICE_VERSION`, at the
crate root rather than in this module because the privacy page renders the same constant as
its "Last updated" line: the page and the rows can then never name different notices.

### What a row holds, and what it deliberately does not

`seq`, `identity_id`, `purpose` (`marketing_emails`, named after the trait so the two are
obviously the same thing), `kind` (`granted`/`withdrawn`), `source`
(`registration`/`settings`/`import`), `notice_version`, `flow_id`, and `at`.

There are also `ip` and `user_agent` columns, and **they are left empty**. The privacy notice
discloses addresses and user agents only as transient web logs kept "for debugging and abuse
prevention"; keeping one against an identity as consent evidence is a different purpose with a
different retention, so it needs its own line in the notice before the hook starts sending
them. The columns exist so that switching them on is a config change rather than a migration:
add two lines to each `.jsonnet` (they are written out in a comment there), and dovecote
already stores and truncates what arrives.

### Only transitions are recorded

A registration with the box left alone writes nothing: there is no consent to evidence, and a
"withdrawn" row for someone who never granted would be a fiction. A settings save that leaves
the trait where it was writes nothing either.

Recording every save instead would bury the two events that matter in a pile of rows saying
nothing changed — and each of those rows would carry the notice version in force at the time,
making it look as though consent had been re-given against a notice the person may never have
been shown.

The rule is one line of Rust (`capsules::consent::consent_transition`, unit-tested) and one
`WHERE` clause in dovecote's single `INSERT ... SELECT`. It is one statement rather than a read
followed by a write because two settings saves in flight would otherwise both see the old state
and both append.

## The route

`POST /internal/consent` — full reference in `docs/api.md`. Service-internal, like the PSK
lookup: the only legitimate caller is our own Kratos.

- **Secret:** `KRATOS_HOOK_SECRET`, a Worker secret, presented in an `X-Kratos-Hook-Secret`
  header. Its own header rather than `Authorization` because Kratos's `api_key` hook auth sends
  a bare value, not a `Bearer` credential. Compared in constant time, checked before the body
  is read.
- **Refuses with 403, never 401.** A 401 from this API is the dashboard's sign-out signal
  (`fancier`'s `dispatch()`), so a misconfigured hook secret must not be able to sign anyone
  out.
- **Fails closed.** An unset or blank secret refuses every call. This route writes evidence for
  a legal claim; a deploy that forgot the secret should record nothing rather than record
  whatever it is told.
- **The notice version and the timestamp are stamped server-side**, never taken from the body.
  Kratos has no idea which notice was on screen, and a caller-supplied version is an assertion
  rather than a record.

### Setting the secret

```sh
# production
cd dovecote && bunx wrangler secret put KRATOS_HOOK_SECRET
# staging
cd dovecote && bunx wrangler secret put KRATOS_HOOK_SECRET --env staging
```

Dev reads it from the gitignored `dovecote/.dev.vars`, where it holds an obviously-dev literal
(`dev-only-consent-hook-secret`) that matches the one in `schemas/kratos/kratos.yml`. Generate
the real ones with something like `openssl rand -base64 32`, and give staging and production
different values. Whatever value an environment's Worker holds, that environment's Kratos
config must hold the same string.

## Kratos configuration

Two hooks, both `response.ignore: true`.

That flag is the significant choice. Ignoring the response means Kratos fires the call
asynchronously and a failure never reaches the person: signup and settings still succeed if
dovecote is down. What it costs is a lost row in exactly that case. It is the right trade
because a consent record must not be able to fail a signup, and because the loss is
recoverable — the trait still carries the person's choice, so the reconciliation below can find
and repair any gap. The opposite setting would mean a backend outage takes registration with
it. Verified live: with dovecote unreachable, Kratos retried three times, logged
`Webhook request failed but the error was ignored`, and the registration completed normally.

### Dev (in-repo, already applied)

`schemas/kratos/kratos.yml` carries both hooks, and `schemas/kratos/hooks/*.jsonnet` the two
bodies. `infra/docker-compose.yml` gained `extra_hosts: host.docker.internal:host-gateway` on
the `kratos` service, because the hook posts to `wrangler dev` running on the host rather than
in the compose network.

One caveat when exercising consent locally: the documented dev command binds
`wrangler dev --ip 127.0.0.1`, which a container cannot reach. Start it on an address the
bridge can see for the duration:

```sh
cd dovecote && bunx wrangler dev --ip 172.17.0.1 --port 8787 --env dev
```

(`172.17.0.1` is the docker0 gateway — reachable from containers, unlike loopback, and unlike
`0.0.0.0` it does not expose the dev API to the rest of the network.) Leaving the default
loopback bind is harmless the rest of the time: the hook fails, the failure is ignored, and
registration works.

**Where the hook goes matters, and the obvious placement silently does nothing.** Kratos picks
*one* hook list per registration method: the method's own if it has one, the global
`registration.after.hooks` otherwise — it does not merge them. A web hook configured only in
the global list was simply absent from a password registration's executor chain, with no error
anywhere. So the config lists it in both places (a YAML anchor, so there is one copy of the
text): the global entry covers passkey and code registrations, and `password`, which already
had its own list, carries it too. It is first in that list because `show_verification_ui`
redirects, and a hook after a redirecting one is a hook that may not run.

The settings hook hangs off `settings.after.profile.hooks` rather than the flow-wide list:
`profile` is the only method that can move a trait, so a password or TOTP change firing it
would be a round trip that can only ever conclude nothing changed.

### Production — for the owner to apply on the VPS

Production Kratos is the systemd binary described in `docs/infra/kratos-systemd-migration.md`;
its config is not in this repo. Three files change under `/opt/kratos/`.

**1. `identity.user.schema.json`** — add the trait and delete `subscribed`, matching
`schemas/kratos/identity.user.schema.json` in this repo exactly:

```json
"marketing_emails": {
  "type": "boolean",
  "title": "Email me occasional product updates about PidgeIoT"
}
```

Adding an optional property breaks nothing, and removing `subscribed` breaks nothing either —
both verified on the dev stack, see the section on it below.

**2. `/opt/kratos/hooks/`** — copy `schemas/kratos/hooks/consent-registration.jsonnet` and
`consent-settings.jsonnet` from this repo, unchanged.

**3. `kratos.yml`** — add the blocks below. `<SECRET>` is the same string put into the Worker
secret above.

```yaml
selfservice:
  flows:
    registration:
      after:
        # Global list: covers every method that has no list of its own.
        hooks: &consent_registration_hook
          - hook: web_hook
            config:
              url: https://api.pidgeiot.com/internal/consent
              method: POST
              body: file:///opt/kratos/hooks/consent-registration.jsonnet
              response:
                ignore: true
              auth:
                type: api_key
                config:
                  name: X-Kratos-Hook-Secret
                  value: <SECRET>
                  in: header
        password:
          # Kratos does not merge the global list with a method's own, so
          # the hook has to be repeated here. Keep whatever `session` /
          # `show_verification_ui` entries production already has, and keep
          # the web hook FIRST.
          hooks:
            - *consent_registration_hook_entry
            - hook: session
            - hook: show_verification_ui

    settings:
      after:
        profile:
          hooks:
            - hook: web_hook
              config:
                url: https://api.pidgeiot.com/internal/consent
                method: POST
                body: file:///opt/kratos/hooks/consent-settings.jsonnet
                response:
                  ignore: true
                auth:
                  type: api_key
                  config:
                    name: X-Kratos-Hook-Secret
                    value: <SECRET>
                    in: header
```

(The in-repo dev config writes the anchor as `- &consent_registration_hook_entry` on the first
list item; copy that form if using the alias, or just write the block out twice.)

Then `sudo systemctl restart kratos` and confirm `http://127.0.0.1:4433/health/ready` answers
200. Staging shares production Kratos, so this one apply covers both dashboards; point the URL
at `https://api.pidgeiot.com` (production dovecote) and accept that a staging registration
records a row through production dovecote, or add a second Kratos config if that matters later.

**File permissions.** `/opt/kratos/kratos.yml` is currently root:root 0644, and the unit runs
under `DynamicUser=yes`, whose ephemeral uid can only read world-readable files. Putting
`<SECRET>` in that file therefore makes it readable by any local account. The secret only
grants "write a consent row for an identity id you name" — no reads, no dashboard access — so
this is a modest exposure, but it is worth closing: create a static `kratos-conf` group, add
`SupplementaryGroups=kratos-conf` to the unit, and set the config to `root:kratos-conf` 0640.
The group must NOT be named `kratos`: `DynamicUser=yes` allocates a dynamic user and group named
after the unit, and a static namesake group collides with that allocation, failing the start
with `217/USER` (observed live). Kratos has
no file-based input for a hook secret and Ory's config loader cannot set a list item from an
environment variable, so there is no way to keep the value out of the config file entirely.

## Removing `subscribed`, and the node that outlives it

`subscribed` is the bare boolean this replaces. **Nothing read it.** A grep across the
workspace found it only in `schemas/kratos/identity.user.schema.json`; every other match in
`dovecote`, `fancier` and `capsules` is the unrelated word inside a Stripe billing comment.
It had no consumer, no purpose attached to it, and no record behind it. It is gone from the
schema in the same edit that adds `marketing_emails`, because declaring both would put two
subscribe-shaped checkboxes on the registration form — the opposite of the clarity Article
7(2) asks for.

The obvious worry is that `additionalProperties: false` makes an identity whose *stored* traits
still carry `subscribed` invalid, and Kratos revalidates traits on every settings save. **It
does not.** Verified on the dev stack rather than reasoned about, which is worth doing because
the actual behaviour has a surprise in it:

- An identity whose stored traits carried `subscribed: true` **signed in normally** and its
  settings flow **rendered** under the new schema.
- Its **settings save succeeded** (303), and the stale key **dropped out of storage** — the
  profile method writes the submitted traits object wholesale, and the form no longer carries
  that field, so saving is what cleans it up.
- **But Kratos still renders a node for it.** The profile form is built from the schema *and*
  the identity's stored traits, so the flow came back with a `traits.subscribed` checkbox
  carrying the stored value and — having no schema entry to take a title from — **no label at
  all**. `InputCheckBoxNode` falls back to the node name, so that person would see a ticked box
  labelled `traits.subscribed`.

So `fancier` hides the node by name (`is_retired_node`), and hiding it is also what clears it:
the form posts every trait except that one, and the next profile save drops the key. The rule
also covers `traits.marketing_consent.granted`, the object trait that was briefly on `main`
before being flattened — no production identity ever saw it, so that line can go sooner.

**To finish the cleanup**, once the counts below are zero everywhere:

```sql
SELECT count(*) FROM identities WHERE traits ? 'subscribed';
SELECT count(*) FROM identities WHERE traits ? 'marketing_consent';
```

Then delete `is_retired_node` and its call site. Nothing else is left to do: the schema entry
is already gone. If a stubborn identity needs the key removed without waiting for its owner to
save, use `PATCH /admin/identities/<id>` with a JSON Patch removing `/traits/subscribed` —
**PATCH, never import**, because importing mints new UUIDs and `flocks.user_id` and every
Durable Object's `pigeon_acl` key on the existing ids.

## Kratos schema versioning: why this is an in-place edit

Kratos identifies schemas by id (`identity.schemas[].id`, `user` here) and stores the id each
identity was created against. The two ways to change one are to edit it in place or to publish
a new id and move identities onto it.

**In place is right for this change**, because adding an optional property is not a breaking
change: every existing identity validates against the new schema unaltered, so there is nothing
to migrate. A new schema id would buy nothing and cost a per-identity write, and every write
against Kratos identities is a chance to trip the id-preservation rule above.

Removing `subscribed` in the same edit looks like the breaking case, and is the reason to
check rather than assume. It is not: an identity carrying the stale key still signs in, still
renders its settings form, and still saves it — the section above has the evidence. So this
stays one in-place edit of the `user` schema.

A new id earns its keep when a change really would make existing identities unusable —
tightening a type under stored values, or adding a `required` entry nothing carries. Neither is
in sight. If one ever is, the answer is `user_v2` plus a per-identity `schema_id` **PATCH**,
never an import.

## Reading the current state

**The dashboard reads the trait, and there is no new route for it.** The settings page already
renders the checkbox from the Kratos settings flow, with its current value, through the same
`ory_form_builder` as every other trait — that *is* the read path, and it is the same control
that performs the withdrawal, which is what makes withdrawal as easy as granting (Article
7(3)).

The `consent_events` table is deliberately not exposed to the dashboard. It is the evidence,
its audience is us and a regulator, and putting it on a subject-editable surface invites
exactly the confusion the split above exists to avoid.

Subject access request — everything on file about one person's consent:

```sql
SELECT kind, source, notice_version, at
  FROM consent_events WHERE identity_id = '<id>' ORDER BY seq;
```

Account-deletion erasure — delete the rows rather than anonymise them (a consent event is
*about* the identity and nothing else, so a row with the id removed means nothing), and only
alongside deleting the identity itself:

```sql
DELETE FROM consent_events WHERE identity_id = '<id>';
```

Both statements are repeated in the migration header, which is where the erasure runbook
already looks.

## Reconciling a lost row

Because the hooks ignore failures, a dovecote outage can leave a person whose trait says
`granted: true` with no `granted` row, or no row at all. The trait and the table disagreeing is
detectable, and the trait is the person's actual choice:

```sql
-- against the Kratos database: who currently wants marketing email
SELECT id FROM identities WHERE (traits->>'marketing_emails')::boolean IS TRUE;
```

Compare that set against the identities whose newest `consent_events` row is `granted`.
Anything in the first set and not the second consented without the record landing. Repair it by
posting to `POST /internal/consent` with `source: import` and the identity id — `import` exists
for exactly this, so a reconstructed row stays distinguishable from one a person's own click
produced.

The reverse gap (a row saying granted, a trait saying otherwise) means a withdrawal was lost,
and it matters more: acting on it would mean mailing someone who opted out. Treat the trait as
authoritative in both directions.

## Relationship to the wording document

Every string a person reads — the label, the helper line, the settings withdrawal text and the
list of email that keeps arriving — is verbatim from
`pidgeiot-business/eu-paperwork-2026-08/consent-wording.md`, and the shape follows its
correction to `phases.md`: one flat boolean trait, `subscribed` replaced rather than kept
alongside, and the evidence in a backend-write-only append-only table.

One thing that document leaves open is settled here rather than by it: it offers the request's
IP and user agent "if we want them". The columns exist and stay empty, because the privacy
notice does not yet cover keeping either as consent evidence. See the section on what a row
holds.
