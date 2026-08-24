# Complimentary tiers: granting an organization a paid tier for free

Some organizations should ride a paid tier's entitlements without paying: a
partner fleet, a design-partner pilot, an account we owe a favour. That is a
**comp**, and it is granted by hand against the database.

There is no route, no admin page, and no self-service surface — deliberately.
A comp is worth real money and is granted a handful of times a year; an HTTP
endpoint that can hand out free Fleet tiers is an attack surface that would
exist permanently to serve an action taken almost never. Same reasoning as
the incident-comms KV values: rare owner action, documented command, nothing
reachable from the internet.

## What a comp does

`organizations` carries three nullable columns:

| Column | Meaning |
|---|---|
| `comp_plan` | The tier slug being granted: `builder`, `growth`, `scale`, `fleet` |
| `comp_note` | Why it exists, in a sentence. Not optional in practice — see below |
| `comp_granted_at` | When it was granted |

`helpers/usage.rs::served_plan` resolves a tier in the order **subscription,
then comp, then free**, and everything downstream — the entitlement gates,
the ingest fuse, the billing overview — reads that one answer. The rules that
follow from it:

- **A live subscription outranks a comp**, even a richer one. A comp is a
  floor for an account that is not paying, not a discount on one that is. A
  grant left on an org that later subscribes is inert; revoking it then
  changes nobody's invoice. It starts serving again only if the subscription
  lapses, which is the safety net a comp is for.
- **A comped org is never billed.** It has no subscription to put an overage
  line on, and the meter reporter skips it explicitly before anything reaches
  Stripe.
- **It is still bounded**, at the granted tier: the ingest fuse pauses it at
  that tier's message allowance, and its device count is a hard cap rather
  than the start of per-device billing. A grant with no meter behind it would
  be an unbounded one.
- **An unreadable value is no grant.** A typo in `comp_plan`, or the value
  `perch`, resolves to the free tier — under-serving loudly rather than
  over-serving silently. Check the org page after granting.

`comp_note` is never exposed over the API. It is our own record, and the
reason it matters is revocation: a grant nobody can explain is a grant nobody
dares remove. Write the sentence you would want to read in two years.

There is **no expiry column**, on purpose. Revoking is setting `comp_plan`
back to `NULL`, and a grant that should end on a date is a subscription with
a trial — an expiry here would be a second, worse billing engine beside the
real one.

## Granting

Run against the production database as the owner. `DOVECOTE_PSQL_CONNECTION`
is in the gitignored `secrets.env`; read it from the environment rather than
pasting it.

```sh
psql "$DOVECOTE_PSQL_CONNECTION" -c "
  UPDATE organizations
  SET comp_plan = '<tier-slug>',
      comp_note = '<why, in a sentence>',
      comp_granted_at = now(),
      updated_at = now()
  WHERE id = '<org-uuid>';"
```

`UPDATE 1` is the confirmation. `UPDATE 0` means the id is wrong — check it
before assuming the grant landed.

Find the org id first if you only have a name:

```sh
psql "$DOVECOTE_PSQL_CONNECTION" -c \
  "SELECT id, name FROM organizations WHERE name ILIKE '%<fragment>%';"
```

## Revoking

```sh
psql "$DOVECOTE_PSQL_CONNECTION" -c "
  UPDATE organizations
  SET comp_plan = NULL,
      comp_note = NULL,
      comp_granted_at = NULL,
      updated_at = now()
  WHERE id = '<org-uuid>';"
```

The org drops to the free tier immediately (or to its subscription, if it has
one). Anything it holds above the free tier's limits keeps working — the
gates only ever refuse *growth*, so no device, seat, alert or organization
disappears. What changes is that the next one is refused, and the ingest fuse
starts measuring against 300 K instead of the granted allowance.

## Listing what is granted

```sh
psql "$DOVECOTE_PSQL_CONNECTION" -c "
  SELECT id, name, comp_plan, comp_granted_at, comp_note
  FROM organizations WHERE comp_plan IS NOT NULL ORDER BY comp_granted_at;"
```

Worth running occasionally. A comp has no expiry, so this list only shrinks
when somebody looks at it.

## Verifying a grant took

The org's own billing panel shows `Complimentary (Builder)` in place of the
plan badge, with a line saying the entitlements are granted rather than
billed.

**Give it a minute.** Hyperdrive runs a ~60 s query cache with no
configuration, so `GET /orgs/:id/billing` can keep answering from before the
`UPDATE` for up to a minute — and refreshing the page repeatedly is exactly
what keeps a stale answer warm. The `UPDATE 1` from psql is the authoritative
confirmation that the write landed; the dashboard is confirmation that it is
being served. Never re-issue the grant on the strength of a stale read.

## Staging

Identical, against `STAGING_PSQL_CONNECTION`. Staging has its own database,
so a comp there is invisible to production.
