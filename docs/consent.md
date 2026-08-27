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
| **State** | `traits.marketing_consent.granted` on the Kratos identity | the person, via the registration and settings forms | what they want now |
| **Evidence** | `consent_events` in Postgres | dovecote only, via `POST /internal/consent` | what they chose, when, against which notice |

The reason for the split is that the settings form hands the subject full control of their own
traits. An `at`/`source`/`notice_version` trait would be evidence the person it is evidence
against can rewrite, which is not evidence. So the trait carries exactly one field, `granted`,
and everything that makes the event provable is server-side.

Both halves are declared in `capsules/src/consent.rs` — the label, the helper lines, and the
one rule (`consent_transition`) that decides whether a flow writes a row. They are in the
shared crate so the words on the form and the record behind them cannot move independently.
The notice version every row is stamped with is `capsules::PRIVACY_NOTICE_VERSION`, at the
crate root rather than in this module because the privacy page renders the same constant as
its "Last updated" line: the page and the rows can then never name different notices.

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

**1. `identity.user.schema.json`** — add the trait, matching
`schemas/kratos/identity.user.schema.json` in this repo exactly:

```json
"marketing_consent": {
  "type": "object",
  "properties": {
    "granted": {
      "type": "boolean",
      "title": "Email me occasional product updates about PidgeIoT"
    }
  }
}
```

Additive and optional, so every existing identity stays valid — see the schema-versioning note
below.

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
this is a modest exposure, but it is worth closing: create a static `kratos` group, add
`SupplementaryGroups=kratos` to the unit, and set the config to `root:kratos` 0640. Kratos has
no file-based input for a hook secret and Ory's config loader cannot set a list item from an
environment variable, so there is no way to keep the value out of the config file entirely.

## Deprecating `subscribed`

`subscribed` is the bare boolean this replaces. **Nothing reads it.** A grep across the
workspace finds it only in `schemas/kratos/identity.user.schema.json`; every other match in
`dovecote`, `fancier` and `capsules` is the unrelated word inside a Stripe billing comment.
It has no consumer, no meaning attached to any purpose, and no record behind it.

It is still declared, on purpose. `additionalProperties: false` means a trait that is not
declared is a trait an identity may not carry, so removing it while any identity still holds a
`subscribed` value would make that identity's traits invalid — and Kratos validates traits on
every settings save. Dev holds 22 identities and none carries it; production is a handful and
almost certainly the same, but that is the owner's to confirm, not mine to assume.

Because it is still declared, Kratos still renders it as a form node, which would put two
subscribe-shaped checkboxes on the registration form — the exact opposite of the clarity
Article 7(2) is asking for. So `fancier` hides that one node by name while it remains in the
schema. The trait stays valid; nothing offers it.

**To finish the deprecation** (a small, separate change, once production is checked):

1. Confirm nothing carries it, against the Kratos database:
   ```sql
   SELECT count(*) FROM identities WHERE traits ? 'subscribed';
   ```
2. If that is not zero, strip the key from those identities with the admin API
   (`PATCH /admin/identities/<id>` with a JSON Patch removing `/traits/subscribed`). **Use
   PATCH, never import**: importing identities mints new UUIDs, and `flocks.user_id` and every
   Durable Object's `pigeon_acl` key on the existing ids.
3. Delete `subscribed` from `identity.user.schema.json`, here and on the VPS.
4. Delete the hide-rule in `fancier` in the same change — it exists only to cover this window.

## Kratos schema versioning: why this is an in-place edit

Kratos identifies schemas by id (`identity.schemas[].id`, `user` here) and stores the id each
identity was created against. The two ways to change one are to edit it in place or to publish
a new id and move identities onto it.

**In place is right for this change**, because adding an optional property is not a breaking
change: every existing identity validates against the new schema unaltered, so there is nothing
to migrate. A new schema id would buy nothing and cost a per-identity write, and every write
against Kratos identities is a chance to trip the id-preservation rule above.

A new id earns its keep when a change would make existing identities invalid — removing a
property they carry, tightening a type, adding a `required` entry. Step 3 of the deprecation
above is the only such change in sight, and step 1 is precisely the check that turns it back
into a safe in-place edit: if nothing carries `subscribed`, removing it breaks nothing. Only if
production turns out to hold values that cannot be stripped would `user_v2` plus a per-identity
`schema_id` PATCH be the answer.

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
SELECT id FROM identities WHERE (traits->'marketing_consent'->>'granted')::boolean IS TRUE;
```

Compare that set against the identities whose newest `consent_events` row is `granted`.
Anything in the first set and not the second consented without the record landing. Repair it by
posting to `POST /internal/consent` with `source: import` and the identity id — `import` exists
for exactly this, so a reconstructed row stays distinguishable from one a person's own click
produced.

The reverse gap (a row saying granted, a trait saying otherwise) means a withdrawal was lost,
and it matters more: acting on it would mean mailing someone who opted out. Treat the trait as
authoritative in both directions.

## Divergence from the wording document, for the owner

`consent-wording.md` proposes a single flat boolean trait named `marketing_emails`, and says to
remove `subscribed` in the same edit. This implementation instead uses
`marketing_consent.granted` and keeps `subscribed` declared for now. Neither difference changes
what a person sees or what gets recorded, and both are worth a sentence:

- **`marketing_consent.granted` rather than `marketing_emails`.** The document's objection to an
  object trait is that Kratos renders every property of one as its own form field, which would
  have exposed the `at`/`source`/`notice_version` evidence fields the original design put
  inside it. That objection is fully answered by dropping those three: an object with a single
  property renders as a single checkbox, confirmed live — the registration flow's node list
  contains exactly one `traits.marketing_consent.granted` node. Keeping it an object leaves an
  obvious home for a second consent purpose later without renaming the first. If the owner
  prefers the flat name it is a one-line schema change plus the matching constant, and it is
  cheapest to make now, before any identity carries the trait.
- **`subscribed` kept.** The document's reason for removing it in the same edit is that
  `additionalProperties: false` requires it; that is not so — the flag forbids *undeclared*
  properties, and a declared-but-unused one is valid. Keeping it is what guarantees no existing
  identity can be invalidated by this change, which matters because production's identities
  cannot be inspected from here. The deprecation section above is the path to removing it.

Everything else — the label, the helper line, the withdrawal wording, the unticked default, the
separation from account creation, and the evidence table's shape — follows the document.
