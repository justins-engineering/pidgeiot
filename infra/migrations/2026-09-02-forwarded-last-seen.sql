-- Migration: per-pigeon stamp for reports that were forwarded to a
-- customer's own `telemetry_endpoint` rather than stored. Those reports
-- write no `pigeon_telemetry_history` row, so before this the scheduled
-- alert evaluator had no evidence such a pigeon had ever reported and read
-- every one of them as permanently silent -- a MissingReport or DeviceState
-- alert over a forwarding fleet fired constantly and meant nothing. See
-- helpers/telemetry.rs::stamp_forwarded_report (writes it) and
-- helpers/alerts.rs::resolve_pigeon_last_seen (merges it with the shadow
-- and history signals).
--
-- NULL for every existing pigeon, which reads as "no forwarded report seen
-- yet": a forwarding pigeon's next report stamps it, and a pigeon that
-- stores its history normally never uses the column at all.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime `ensure_alert_tables`
-- and `ensure_pigeons_last_forwarded_column` run the same statement lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-09-02-forwarded-last-seen.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned (an ADD COLUMN on an already-owned table
-- needs no ownership transfer, but the role must be allowed to ALTER it).
-- For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE pigeons
  ADD COLUMN IF NOT EXISTS last_forwarded_at TIMESTAMPTZ;

RESET ROLE;
