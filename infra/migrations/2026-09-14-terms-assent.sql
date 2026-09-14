-- Migration: the record behind a Terms of Service assent.
--
-- The attorney's memo asks for a server-side record of the Terms version, the
-- account and the acceptance time before we rely on the liability cap, the
-- forum clause, the jury waiver or the incorporated DPA. That record is a
-- second `purpose` in consent_events ('terms_of_service'), written by
-- dovecote's `POST /account/terms` and by the checkout route -- see
-- docs/consent.md and docs/design/terms-assent-and-legal-pages.md.
--
-- Two changes, because the table already exists everywhere. `source` is
-- CHECK-constrained and CREATE TABLE IF NOT EXISTS is inert against a table
-- that is already there, so without the widening below the first 'gate' insert
-- fails at runtime on staging and production while passing every local test
-- against a fresh database. `org_id` records which entity a checkout assent
-- bound, which is the one fact the identity alone cannot reconstruct after an
-- abandoned checkout.
--
-- The widening is conditional rather than a blind DROP/ADD: a DROP/ADD on
-- every run takes an ACCESS EXCLUSIVE lock and revalidates the table, and the
-- constraint's real name is discovered rather than assumed. Idempotent -- safe
-- to run repeatedly; the second run does nothing at all.
--
-- Must stay statement-equivalent with dovecote's runtime
-- `ensure_consent_tables` (helpers/consent.rs) and with the same block in
-- infra/init-db.sql. Belt (this file at deploy) and suspenders (the lazy
-- ensure heals a database this was not run against).
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-09-14-terms-assent.sql
--
-- Run as the cluster's "application" superuser. SET ROLE keeps every object
-- dovecote-owned; for a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

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

-- Subject access request, now that two purposes share the table -- project
-- the purpose or the answer runs them together:
--   SELECT purpose, kind, source, notice_version, at
--     FROM consent_events WHERE identity_id = '<id>' ORDER BY seq;
--
-- Account-deletion erasure: marketing rows go with the identity. A Terms
-- assent is evidence of a contract and is kept under Article 17(3)(e), minus
-- its request context:
--   DELETE FROM consent_events
--    WHERE identity_id = '<id>' AND purpose <> 'terms_of_service';
--   UPDATE consent_events SET ip = NULL, user_agent = NULL
--    WHERE identity_id = '<id>' AND purpose = 'terms_of_service';

RESET ROLE;
