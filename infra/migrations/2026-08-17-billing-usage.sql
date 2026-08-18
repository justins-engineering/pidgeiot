-- Migration: billable-message usage aggregation, per billing account and
-- billing period. Our Postgres is the source of truth for usage; Stripe's
-- meters are a reporting sink fed from these rows.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Apply to EXISTING prod + staging databases (dovecote's own
-- runtime `ensure_billing_usage_tables` in helpers/usage.rs runs the same
-- statements lazily from the cron path, so this is belt-and-suspenders,
-- but applying it explicitly at deploy time keeps the hot ingest paths --
-- which deliberately never run DDL -- from undercounting until the first
-- cron invocation).
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-17-billing-usage.sql
--
-- Run as the cluster's "application" superuser. SET ROLE makes every object
-- dovecote-owned from creation, so the app role can use what this creates
-- without a post-hoc ownership transfer (the ALTER ... OWNER TO lines below
-- stay as idempotent self-healing for any run that skipped this). For a
-- staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- Schema body only -- no CREATE ROLE/DATABASE/\c bootstrap (that lives in
-- init-db.sql's first lines and must never be re-run against prod).
--
-- OWNERSHIP, and this is not optional: the owner applies migrations as the
-- Crunchy Bridge cluster owner, so anything CREATEd here comes out owned by
-- that role and is unusable by the app role Hyperdrive connects as. Run the
-- ALTER TABLEs at the bottom of this file in the same session, with the
-- role matching the database: `dovecote` for production, `dovecote_staging`
-- for staging.

-- One row per billing account per billing period. The account is either an
-- organization (org-owned flocks) or a bare user (personal flocks, which
-- have no org and therefore no subscription -- always free tier). Period
-- anchoring: the org's own Stripe current_period bounds while a live
-- subscription covers now(); calendar month otherwise.
CREATE TABLE IF NOT EXISTS billing_usage_periods (
  owner_kind TEXT NOT NULL CHECK (owner_kind IN ('org', 'user')),
  owner_id UUID NOT NULL,
  period_start TIMESTAMPTZ NOT NULL,
  period_end TIMESTAMPTZ NOT NULL,
  billable_messages BIGINT NOT NULL DEFAULT 0,
  -- Free-tier fuse bookkeeping: set once per period when the 80% warning
  -- email went out / when usage first crossed the allowance. Both are
  -- claimed atomically (UPDATE ... WHERE ... IS NULL RETURNING) so
  -- concurrent queue consumers can't double-send or double-stamp.
  warned_at TIMESTAMPTZ,
  paused_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (owner_kind, owner_id, period_start)
);

-- Claimed rollups reported to Stripe's billing meters. A row is a claim
-- first and a delivery record second: the reporter inserts the day's delta
-- here, then marks posted_at as it hands the figure to Stripe -- claimed
-- rows count toward "already reported" whether or not the POST landed, so
-- a transient Stripe failure undercounts (logged) rather than ever
-- double-billing. `meter` is our internal name ('messages' | 'devices');
-- Stripe's own event_name is resolved at run time from the price catalog.
CREATE TABLE IF NOT EXISTS billing_meter_reports (
  org_id UUID NOT NULL,
  period_start TIMESTAMPTZ NOT NULL,
  report_day DATE NOT NULL,
  meter TEXT NOT NULL CHECK (meter IN ('messages', 'devices')),
  quantity BIGINT NOT NULL,
  stripe_identifier TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  posted_at TIMESTAMPTZ,
  PRIMARY KEY (org_id, period_start, report_day, meter)
);

-- Cadence gate for the reporter, which rides the existing 5-minute cron:
-- one row, claimed once per ~day (see helpers/usage.rs::claim_reporter_run).
CREATE TABLE IF NOT EXISTS billing_reporter_state (
  id SMALLINT PRIMARY KEY,
  last_run_at TIMESTAMPTZ NOT NULL
);

-- Ownership transfer -- see the header. Production:
ALTER TABLE billing_usage_periods OWNER TO dovecote;
ALTER TABLE billing_meter_reports OWNER TO dovecote;
ALTER TABLE billing_reporter_state OWNER TO dovecote;
-- Staging (run these lines instead, against the staging database):
-- ALTER TABLE billing_usage_periods OWNER TO dovecote_staging;
-- ALTER TABLE billing_meter_reports OWNER TO dovecote_staging;
-- ALTER TABLE billing_reporter_state OWNER TO dovecote_staging;

RESET ROLE;
