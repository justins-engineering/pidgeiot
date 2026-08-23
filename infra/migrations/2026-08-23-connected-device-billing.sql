-- Migration: per-pigeon last-billable-activity stamp, so the extra-devices
-- meter bills devices that actually reported in the period rather than
-- every provisioned row. This is what the pricing page has always promised
-- ("the 400 units sitting in a warehouse are free"); see
-- helpers/usage.rs::record_billable_message (writes it) and
-- ::report_device_overage (filters on it).
--
-- NULL for every existing pigeon, which reads as "never reported": the
-- first billable message from each device stamps it. A meter run in the
-- window between this migration and a device's next report therefore
-- undercounts rather than over-bills, which is the safe direction.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_billing_usage_tables` runs the same statement lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-23-connected-device-billing.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned (an ADD COLUMN on an already-owned table
-- needs no ownership transfer, but the role must be allowed to ALTER it).
-- For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE pigeons
  ADD COLUMN IF NOT EXISTS last_billable_activity TIMESTAMPTZ;

RESET ROLE;
