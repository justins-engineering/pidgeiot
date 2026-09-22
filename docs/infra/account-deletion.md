# Account deletion

The documented account-deletion procedure the DPA's Annex II G.3 refers to,
and the one the Privacy Policy's retention table is measured against. It is
implemented by [`scripts/purge-identities.py`](../../scripts/purge-identities.py),
which is the only supported way to run it.

Two modes share one spine:

- **fixture** — removing an account a test run created. The default.
- **erasure** — a person's Article 17 request. Same order, one difference in
  the consent sweep, plus a DSAR log line. See [Erasure mode](#erasure-mode);
  it is refused at runtime today, and why is the point of that section.

## Why the order matters more than the steps

A Kratos identity is not the account. `flocks.user_id`, every `pigeon_acl`
row, `alert_definitions.user_id`, `organization_members.user_id`,
`consent_events.identity_id`, `dashboard_state.user_id` and the rest are
plain UUID columns with **no foreign key** to Kratos — Kratos owns its own
tables, so nothing cascades from an identity delete. Deleting the identity
first therefore orphans every one of those rows.

One of those orphans cannot be repaired at all. A pigeon's authoritative
state lives in its own Durable Object, and the only thing that empties one is
`DELETE /pigeons/:pigeon_id`, whose DO-side handler is gated by `is_owner`
against that pigeon's own DO-resident access list. There is no admin route
and no internal purge route. So the account's own live session is the only
key to its own devices, and deleting the identity throws that key away
permanently: the Durable Object keeps its rows, its shadow, its telemetry and
its device credential, stays addressable by id, and becomes unmeterable and
unpausable because `check_ingest_fuse` allows anything with no Postgres
mirror row to meter against.

Hence: **capture, then delete through the product as the account, then sweep
what no route reaches, then prove the rows are gone, then the identity.**
Proving it afterwards would be reporting a problem one step too late to fix.

## The order

1. **Classify and refuse.** The keep list — `code@jes.contact`,
   `justin@jes.contact` and the two standing staging fixtures — is refused
   under every flag. An address that is not ours and does not name itself a
   fixture needs `--allow-external REASON`. The mail-catcher recipient is
   held unless `--include-held`.
2. **Inventory**, by direct `psql` against each database, before anything is
   deleted. Pigeon ids are Durable Object addresses and dashboard-state keys
   are built from pigeon and flock ids; neither is recoverable once the
   Postgres rows are gone. Written to `inventory/<id>.json` in the run
   directory.
3. **Refuse** if rows exist in an environment the run did not select. One
   Kratos serves staging and production, so an account's identity can be
   production-side while its fleet is staging rows.
4. **Mint a session** — an admin recovery link, spent once against the
   loopback listener. It queues no mail and needs no password. One mint
   covers both API hosts, and a resumed run always mints a fresh one: the
   token is single-use.
5. **Delete through the product's own routes**, in this order, because each
   step is a precondition of the next: pigeons (the only thing that empties a
   Durable Object) → flocks, which must be empty → alerts → `DELETE /errors`
   → one `DELETE /dashboard-state/:scope_key` per captured key → orgs. For an
   org the caller does not own, leave it with
   `DELETE /orgs/:org_id/members/:own_user_id`. For one they solely own,
   revoke its pending invites and delete it once it owns no flocks. An org
   with other members is **reported and held**: transferring it names a
   successor, which is the owner's decision and not a script's. So is an org
   owning more flocks than the identity created — `POST /flocks/:id/transfer`
   leaves `flocks.user_id` as provenance, so a transferred flock is the org's
   and needs its own owner before the org can go.
6. **Sweep what no route reaches**, one transaction per database:
   `consent_events`; `contact_submissions.user_id` set to NULL, because the
   row is correspondence the sender addressed to us; `billing_usage_periods`
   and `billing_meter_reports`; residual `organization_members` and
   `organization_invites` rows on `user_id`/`email`/`invited_by`/`created_by`
   — `created_by` is `NOT NULL`, so a spent invite row is deleted rather than
   detached; `pigeon_acl` mirror rows; anything the API leg missed in
   `error_events` and `dashboard_state`; and `flocks.owner_email`.
7. **Prove the rows are gone, before the identity.** Direct `psql` only —
   one `SELECT count(*)` per identity-keyed column the sweep or the routes
   were responsible for, plus the deleted orgs' own rows and the deleted
   pigeons' mirror rows, all zero. Never a list route: Hyperdrive's ~60 s
   query cache makes the natural list-write-list check poison its own
   result. A non-zero count stops that account **with its identity intact**,
   because the session is the only key to anything still keyed on it.
8. **Revoke sessions, delete the identity, confirm 404.**

## Running it

```sh
scripts/purge-identities.py --env both --candidates-from identities.json      # read this
scripts/purge-identities.py --env both --candidates-from identities.json --apply
```

Dry run is what happens without `--apply`, which asks for the plan's ready
count typed back on a terminal. `--env both` is the only sequence that can
finish an account with rows in each database: rows in an environment the run
did not select are a hold, so `--env staging` and `--env prod` each hold
whatever the other still holds. One Kratos serves both, so either leg
deletes the production identity at the end of its own run.

Each run writes `plan.json`, `inventory/<id>.json`, `state.json` and
`log.ndjson` under `--run-dir` (a path under `/tmp` by default). `--resume
RUNDIR` continues an interrupted run: it re-derives what is left by `psql`,
skips completed steps and mints a fresh session. Every step is a delete by
id, so re-running is safe — a missing target counts as done.

Connection strings are read by name (`DOVECOTE_PSQL_CONNECTION`,
`STAGING_PSQL_CONNECTION`, `VPS_SSH`) from the environment or `secrets.env`,
and decomposed into `PG*` variables so they never reach `ps`. Both deployed
strings name the same managed instance, so the run refuses two environments
that open the same database. A recovery link reaches the remote shell in a
file that shell writes itself, so no one-time token lands in either
machine's `ps`.

`--absent-ok REASON` is needed to sweep a uuid Kratos no longer knows:
`pigeon_acl.entity_id` holds organization ids as well as identity ids, so an
org id pasted into the list would otherwise strip every grant that org
confers. The run holds any id that turns out to name an `organizations` row.

## What the procedure does not reach

Seven things survive it. Each needs a deliberate decision rather than a flag.

- **Stripe.** Deleting an organization does not touch its customer or
  subscription; the script never calls Stripe and refuses an org carrying
  either id unless `--orphan-stripe REASON` says the subscription is already
  cancelled. Every id it saw is written to `stripe-to-cancel.txt` for the
  owner's own step, with the right key — the sandbox subscriptions live under
  a key that is not in `secrets.env` at all.
- **An access-list grant inside a Durable Object belonging to a pigeon that
  survives.** The ACL write paths are upsert-only and there is no delete
  route, so the Postgres mirror row can be swept but the DO's own copy cannot
  be cleared short of deleting and recreating that pigeon. The plan prints
  one line per such grant.
- **R2 firmware bytes.** Objects are content-addressed at
  `firmware/<sha256>.bin` and shared across flocks; no route deletes them, and
  the Privacy Policy says as much. Removing one is safe only once no
  `flock_firmware` row in that environment still names that hash.
- **Kratos `courier_messages`.** The table keys on the recipient address, not
  on an identity, so every verification and recovery mail ever queued for the
  address survives the identity. Removing those is a direct write against the
  Kratos DSN.
- **An address inside somebody else's alert.** `alert_definitions.channel`
  carries the recipient list (`{"Email":{"to":[…]}}`), so an alert belonging
  to an account that stays can name an address that has just been deleted.
  The plan prints one line per such definition; nothing edits it, because
  editing another account's alert changes who it notifies. A real erasure has
  to decide per definition, and `PUT /alerts/:id` is the route.
- **A Durable Object whose Postgres row was already gone.** The procedure
  reaches a pigeon through its mirror row, so a pigeon deleted by hand in an
  earlier era of cleanup — `DELETE FROM pigeons` without the route — leaves an
  object this run cannot see, let alone wipe. The dev store holds dozens of
  them against a handful of live pigeons; staging and production may hold
  their own.
  Enumerating them means the Cloudflare API's namespace listing,
  `GET /accounts/:account_id/workers/durable_objects/namespaces/:id/objects`,
  which answers `id` and `hasStoredData` per object; it has not been run
  against this account, so treat the shape as documented rather than proven.
  Clearing one needs a dovecote route that does not exist: every path into a
  pigeon's DO is gated by that pigeon's own access list, and the object no
  longer has one.
- **A pigeon's stored log dictionary in R2.** `DELETE /pigeons/:pigeon_id`
  removes `log-dictionaries/<pigeon_id>.json` best-effort and logs a failure
  rather than failing the request, so a failed R2 delete leaves the object
  behind. It is unreachable — every log-dictionary route re-checks the access
  list first — but it is still bytes.

## Erasure mode

`--mode erasure --dsar-log PATH --dsar-ref REF` differs from fixture mode in
exactly two ways: the consent sweep takes every `consent_events` row except
the `terms_of_service` ones — excluding rather than naming what goes, so a
purpose added later is erased by default — and one
`<utc>\t<identity id>\t<email>\t<request ref>` line is appended to the DSAR
log — which is then the only surviving link between a retained row and a
person.

**It is written and refused at runtime.** Keeping the acceptance record past
deletion is what Article 17(3)(e) permits and what counsel recommended at the
2026-09-22 close, and it is the rule a real deletion should follow: the
record is the evidence that the liability cap, the forum clause and the
incorporated DPA were agreed to, and it is exactly the account whose deletion
is disputed whose record is worth having. But the published text says the
opposite today — the Privacy Policy's retention table says the record is
"deleted with your account", and Annex II G.3 lists it among what account
deletion removes. Taking the mode live means moving both documents first,
with the 30-day notice the Policy owes on a material change, and then
dropping the refusal. Until then fixture mode is the only live path and a
real erasure removes both purposes.
