-- Migration: storage for the public contact form (`POST /contact`, see
-- docs/api.md). The notification email is the working surface, but it is
-- best-effort like every other send in this codebase, so the row is what
-- guarantees an enquiry is never lost to a Resend outage -- and it is what
-- makes "did anyone ever reply?" answerable.
--
-- No IP address column, deliberately. The rate limiter keys on
-- CF-Connecting-IP and discards it (same as `POST /errors`), and the
-- privacy policy promises no tracking: the sender's own email address is
-- the only identifier needed to reply.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_contact_table` (helpers/contact.rs) runs the same statements
-- lazily, so applying this at deploy time only spares the first
-- submission a DDL round trip.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-24-contact-submissions.sql
--
-- Run as the cluster's "application" superuser. SET ROLE makes every
-- object dovecote-owned from creation, so the app role can use what this
-- creates without a post-hoc ownership transfer (the ALTER ... OWNER TO
-- line below stays as idempotent self-healing for any run that skipped
-- this). For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- One row per submission. Field lengths are enforced in the route
-- (capsules::contact::validate) rather than as column types: the caps are
-- shared with the form, and a VARCHAR(n) here would be a second
-- declaration of the same number that nothing keeps in step.
--
-- `user_id` is populated only when a Kratos session happened to be present
-- (the form is public and usually reached logged-out); it is resolved
-- server-side and never trusted from the body, same as `POST /feedback`.
-- Account-deletion erasure: the manual runbook should clear it with
--   UPDATE contact_submissions SET user_id = NULL WHERE user_id = '<id>';
-- rather than deleting the row, since the enquiry itself is business
-- correspondence the sender addressed to us.
CREATE TABLE IF NOT EXISTS contact_submissions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  company TEXT,
  -- capsules::ContactFleetSize wire value, or NULL when unanswered. Kept
  -- as TEXT rather than an enum type so adding a band is a code change
  -- only; an unknown value here would fail deserialization at the route
  -- long before it could reach this column.
  fleet_size TEXT,
  -- Which link opened the form ('fleet' from the pricing page's Fleet
  -- tier), so the sales funnel is attributable without asking the sender.
  about TEXT,
  message TEXT NOT NULL,
  user_id UUID,
  -- Stamped after the ops email is handed to the transport. NULL on a row
  -- means the enquiry landed but nobody was told, which is exactly the
  -- state worth being able to query for.
  notified_at TIMESTAMPTZ
);

-- The only read pattern: newest first, and the "stored but never mailed"
-- sweep. Partial index because almost every row is notified.
CREATE INDEX IF NOT EXISTS idx_contact_submissions_received ON contact_submissions(received_at DESC);
CREATE INDEX IF NOT EXISTS idx_contact_submissions_unnotified ON contact_submissions(received_at) WHERE notified_at IS NULL;

-- Ownership transfer -- see the header. Production:
ALTER TABLE contact_submissions OWNER TO dovecote;
-- Staging (run this line instead, against the staging database):
-- ALTER TABLE contact_submissions OWNER TO dovecote_staging;

RESET ROLE;
