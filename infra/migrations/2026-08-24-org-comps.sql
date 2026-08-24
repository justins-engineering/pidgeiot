-- Migration: complimentary tier grants on organizations, so a specific
-- org can be served a paid tier's entitlements without a subscription --
-- a partner fleet, a design-partner pilot, an account we owe a favour.
--
-- Three columns rather than one because a grant nobody can explain is a
-- grant nobody dares revoke: `comp_note` is why it exists and
-- `comp_granted_at` is when, both of which a future reader needs before
-- they can safely null the first.
--
-- Deliberately no expiry column. A comp is revoked by setting comp_plan
-- back to NULL, and a grant that should end on a date is a subscription
-- with a trial, not a comp -- adding an expiry here would build a second,
-- worse billing engine beside the real one.
--
-- Values in comp_plan are the tier slugs capsules::BillingPlan already
-- parses ('builder', 'growth', 'scale', 'fleet'); an unparseable value
-- resolves to the free tier rather than erroring, so a typo under-serves
-- loudly instead of over-serving silently. 'perch' is accepted and means
-- the free tier, which is what an org gets anyway -- write NULL instead.
--
-- No route grants these. The grant and revoke commands live in
-- docs/infra/org-comps.md and are run by hand against the database; see
-- helpers/usage.rs::served_plan for how the columns are read.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_billing_tables` runs the same statements lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-24-org-comps.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned. For a staging apply, use
-- SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE organizations
  ADD COLUMN IF NOT EXISTS comp_plan TEXT,
  ADD COLUMN IF NOT EXISTS comp_note TEXT,
  ADD COLUMN IF NOT EXISTS comp_granted_at TIMESTAMPTZ;

RESET ROLE;
