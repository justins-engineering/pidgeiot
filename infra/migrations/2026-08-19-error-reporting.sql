-- Migration: client error-report storage, signature-grouped. Two tables
-- because a group and an occurrence want different lifetimes: groups are
-- small and kept indefinitely (first-seen history is the most useful thing
-- here), events age out on a 90-day sweep.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Apply to EXISTING prod + staging databases (dovecote's own
-- runtime `ensure_error_tables` in helpers/errors.rs runs the same
-- statements lazily, so this is belt-and-suspenders, but applying it
-- explicitly at deploy time spares the first ingest the DDL round trip).
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-19-error-reporting.sql
--
-- Run as the cluster's "application" superuser. SET ROLE makes every object
-- dovecote-owned from creation, so the app role can use what this creates
-- without a post-hoc ownership transfer (the ALTER ... OWNER TO lines below
-- stay as idempotent self-healing for any run that skipped this). For a
-- staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- One row per error signature (a truncated SHA-256 over kind + normalized
-- message + location, computed server-side only). `message` is the
-- NORMALIZED, redacted exemplar -- ids, emails, and token-shaped runs
-- replaced with placeholders -- which is what makes keeping groups forever
-- compatible with the privacy policy. The raw capped message lives only on
-- the 90-day event rows.
CREATE TABLE IF NOT EXISTS error_groups (
  signature TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  message TEXT NOT NULL,
  location TEXT,
  first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
  first_build TEXT,
  last_build TEXT,
  occurrences BIGINT NOT NULL DEFAULT 0,
  -- Claimed atomically (UPDATE ... WHERE notified_at IS NULL ... RETURNING)
  -- under a global per-hour budget, so a flood of crafted new signatures
  -- cannot become a mail flood -- see helpers/errors.rs.
  notified_at TIMESTAMPTZ,
  -- Set by hand (or by the future support panel) to mark a group closed.
  resolved_at TIMESTAMPTZ
);

-- One row per report. `user_id` is populated ONLY for identified manual
-- reports (application/json with a note, session resolved server-side);
-- automatic reports are anonymous by construction -- their handler never
-- resolves a session. The CHECK makes that policy structural. Retention
-- keys on `received_at` (server clock) because `occurred_at` is
-- client-claimed and a future-stamped row must still age out.
--
-- Account-deletion erasure: the manual account-deletion runbook (and any
-- future automated flow) must remove identified rows --
--   DELETE FROM error_events WHERE user_id = '<kratos identity id>';
-- (same statement the authenticated DELETE /errors route runs).
CREATE TABLE IF NOT EXISTS error_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  signature TEXT NOT NULL REFERENCES error_groups(signature) ON DELETE CASCADE,
  -- Client-minted correlation id shown on the crash screen; a follow-up
  -- note carries the same id so the user's words join their crash. A hint,
  -- not a key -- deliberately not UNIQUE, since ids are attacker-reusable
  -- and notes attach alongside rather than overwrite.
  client_event_id UUID,
  occurred_at TIMESTAMPTZ NOT NULL,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  user_id UUID,
  message TEXT,
  route TEXT,
  build TEXT,
  user_agent TEXT,
  stack TEXT,
  breadcrumbs JSONB,
  report_note TEXT,
  CONSTRAINT error_events_identity_requires_note CHECK (user_id IS NULL OR report_note IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_error_events_signature ON error_events(signature);
CREATE INDEX IF NOT EXISTS idx_error_events_received ON error_events(received_at);
-- Partial: the erasure path looks up identified rows only, and almost
-- every row is anonymous.
CREATE INDEX IF NOT EXISTS idx_error_events_user ON error_events(user_id) WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_error_groups_last_seen ON error_groups(last_seen DESC);

-- Ownership transfer -- see the header. Production:
ALTER TABLE error_groups OWNER TO dovecote;
ALTER TABLE error_events OWNER TO dovecote;
-- Staging (run these lines instead, against the staging database):
-- ALTER TABLE error_groups OWNER TO dovecote_staging;
-- ALTER TABLE error_events OWNER TO dovecote_staging;

RESET ROLE;
