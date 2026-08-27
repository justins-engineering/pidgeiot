-- Migration: the record behind a marketing-consent tick.
--
-- Article 7(1) puts the burden of demonstrating consent on us, so the
-- checkbox alone is not enough: the trait on the Kratos identity is the
-- current state and the person owns it, and this table is the history,
-- which only dovecote writes (`POST /internal/consent`, called by Kratos's
-- after-registration and after-settings web hooks -- see docs/consent.md).
-- Evidence the subject can edit is not evidence, which is why none of
-- these columns is a trait.
--
-- Append-only by construction: nothing in the codebase issues an UPDATE or
-- a DELETE against it. Current state is derivable (the newest row per
-- identity and purpose), but a current-state column could not show that
-- consent existed *before* it was relied on, which is the question a
-- complaint actually asks.
--
-- No IP or user-agent column, deliberately, matching
-- 2026-08-24-contact-submissions.sql: recording the address someone
-- consented from is itself processing that would need its own basis and
-- its own line in the privacy notice, and the identity id already
-- identifies the person whose consent this is.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_consent_tables` (helpers/consent.rs) runs the same statements
-- lazily, so applying this at deploy time only spares the first
-- registration a DDL round trip.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-27-consent-events.sql
--
-- Run as the cluster's "application" superuser. SET ROLE makes every
-- object dovecote-owned from creation, so the app role can use what this
-- creates without a post-hoc ownership transfer (the ALTER ... OWNER TO
-- lines below stay as idempotent self-healing for any run that skipped
-- this). For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- One row per change of mind. Nothing is written when a flow leaves the
-- trait where it already was, so the rows are the transitions and not a
-- log of saves -- see `capsules::consent::consent_transition` for the rule
-- and dovecote's single INSERT ... WHERE for the concurrency-safe form
-- of it.
CREATE TABLE IF NOT EXISTS consent_events (
  -- Ordering, not just identity. Two events can share a timestamp at the
  -- resolution `now()` gives, and "which came last" has to be answerable
  -- without a tiebreak nobody wrote down.
  seq BIGSERIAL PRIMARY KEY,
  -- The Kratos identity id, the same key flocks.user_id and every DO's
  -- pigeon_acl use. No FK: Kratos owns its own tables in its own schema
  -- and dovecote must not constrain them.
  identity_id UUID NOT NULL,
  -- What was consented to. One value exists today
  -- (capsules::MARKETING_EMAIL_PURPOSE, 'marketing_email'); the column is
  -- here so a second purpose is a new value rather than a new table, and
  -- so a query about marketing consent never has to mean "every row".
  -- Not an enum type, same reasoning as contact_submissions.fleet_size:
  -- adding one stays a code change.
  purpose TEXT NOT NULL,
  -- CHECK rather than a lookup table: two values, and a row that says
  -- neither would be unreadable evidence.
  kind TEXT NOT NULL CHECK (kind IN ('granted', 'withdrawn')),
  -- Which surface the person used. Constrained because each value is a
  -- form whose exact wording we can produce; free text here would let a
  -- row claim a provenance nobody can show. 'import' is unused today and
  -- exists so that a list migrated in from elsewhere stays
  -- distinguishable from consent given to us directly.
  source TEXT NOT NULL CHECK (source IN ('registration', 'settings', 'import')),
  -- The privacy notice in force, as its published "Last updated" date
  -- (capsules::PRIVACY_NOTICE_VERSION). Consent covers the purposes the
  -- notice described, so a row without one cannot show what was agreed
  -- to. Stamped server-side; the web hook does not get to assert it.
  notice_version TEXT NOT NULL,
  -- The Kratos self-service flow this happened in, when the hook context
  -- carried one: a cross-reference into Kratos's own tables for
  -- reconstructing a disputed event. Nullable because it is a
  -- convenience, not the evidence.
  flow_id UUID,
  at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The only read pattern: the newest event for one identity and purpose,
-- which is what decides whether an incoming change is a transition worth
-- recording. `seq DESC` so that lookup is a backwards index scan of one
-- row rather than a sort.
CREATE INDEX IF NOT EXISTS idx_consent_events_identity
  ON consent_events(identity_id, purpose, seq DESC);

-- Account-deletion erasure: unlike contact_submissions (where the row is
-- correspondence the sender addressed to us and only `user_id` is
-- cleared), a consent event is *about* the identity and nothing else, so
-- anonymising it would leave a row that means nothing. Delete it:
--   DELETE FROM consent_events WHERE identity_id = '<id>';
-- Do this only alongside deleting the identity itself. While the account
-- exists the history is what demonstrates the consent we are relying on,
-- and Article 7(1) is the reason to keep it.
--
-- Subject access request -- everything on file about one person's
-- consent, in the order it happened:
--   SELECT kind, source, notice_version, at
--     FROM consent_events WHERE identity_id = '<id>' ORDER BY seq;

-- Ownership transfer -- see the header. Production:
ALTER TABLE consent_events OWNER TO dovecote;
ALTER SEQUENCE consent_events_seq_seq OWNER TO dovecote;
-- Staging (run these lines instead, against the staging database):
-- ALTER TABLE consent_events OWNER TO dovecote_staging;
-- ALTER SEQUENCE consent_events_seq_seq OWNER TO dovecote_staging;

RESET ROLE;
