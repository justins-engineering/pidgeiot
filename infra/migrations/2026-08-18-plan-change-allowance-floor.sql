-- Migration: per-period message-allowance floor for mid-period plan
-- changes. The floor records the highest allowance of any tier the org was
-- entitled to during the period, so a downgrade never converts
-- already-included usage into overage retroactively (see
-- helpers/usage.rs::period_message_allowance).
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_billing_usage_tables` runs the same statement lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-18-plan-change-allowance-floor.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned (an ADD COLUMN on an already-owned table
-- needs no ownership transfer, but the role must be allowed to ALTER it).
-- For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE billing_usage_periods
  ADD COLUMN IF NOT EXISTS allowance_floor_messages BIGINT;

RESET ROLE;
