-- Migration: Stripe billing state on organizations + webhook idempotency.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Apply to EXISTING prod + staging databases (dovecote's own
-- runtime `ensure_billing_tables` in helpers/billing.rs runs the same
-- statements lazily, so this is belt-and-suspenders, but applying it
-- explicitly at deploy time avoids the first webhook paying the DDL cost
-- and keeps init-db.sql the single documented schema).
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-12-billing.sql
--
-- Schema body only -- no CREATE ROLE/DATABASE/\c bootstrap (that lives in
-- init-db.sql's first lines and must never be re-run against prod).
--
-- Run as the cluster's "application" superuser. SET ROLE makes every object
-- dovecote-owned from creation, so the app role can use what this creates
-- without a post-hoc ownership transfer (the ALTER ... OWNER TO lines below
-- stay as idempotent self-healing for any run that skipped this). For a
-- staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- OWNERSHIP, and this is not optional: the owner applies migrations as the
-- Crunchy Bridge cluster owner, so anything CREATEd here comes out owned by
-- that role and is unusable by the app role Hyperdrive connects as. The
-- organizations migration hit exactly this and every org request failed
-- until ownership was transferred. Run the ALTER TABLE at the bottom of
-- this file in the same session, with the role matching the database:
-- `dovecote` for production, `dovecote_staging` for staging.

ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_subscription_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'perch';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS subscription_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_start TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_end TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS cancel_at_period_end BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS billing_event_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS stripe_webhook_events (
  event_id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  event_created TIMESTAMPTZ NOT NULL,
  livemode BOOLEAN NOT NULL DEFAULT false,
  api_version TEXT,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  processed_at TIMESTAMPTZ,
  redelivery_count INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_customer
  ON organizations(stripe_customer_id) WHERE stripe_customer_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_subscription
  ON organizations(stripe_subscription_id) WHERE stripe_subscription_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_stripe_webhook_events_unprocessed
  ON stripe_webhook_events(received_at) WHERE processed_at IS NULL;

-- Ownership transfer -- see the header. Production:
ALTER TABLE stripe_webhook_events OWNER TO dovecote;
-- Staging (run this line instead, against the staging database):
-- ALTER TABLE stripe_webhook_events OWNER TO dovecote_staging;

RESET ROLE;
