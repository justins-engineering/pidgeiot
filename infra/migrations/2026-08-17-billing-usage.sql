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

-- Ownership transfer -- see the header. Production:
ALTER TABLE billing_usage_periods OWNER TO dovecote;
-- Staging (run this line instead, against the staging database):
-- ALTER TABLE billing_usage_periods OWNER TO dovecote_staging;
