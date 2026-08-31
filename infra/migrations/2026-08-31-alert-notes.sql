-- Migration: an alert definition carries operator notes.
--
-- The notes are free text the operator wrote for whoever reads the
-- notification -- which breaker to check first, a runbook link. They are
-- rendered into the alert email and shown in the dashboard, never
-- interpreted. Bounded by `capsules::MAX_ALERT_NOTES_BYTES` in the routes
-- rather than as a column type, so the number is declared once.
--
-- The same change also lets an alert name several email recipients instead
-- of one. That needs no DDL: recipients live in the existing `channel`
-- JSONB, whose `to` key now holds a list and still parses the single
-- address (or null) written before it was one.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime `ensure_alert_tables`
-- (helpers/alerts.rs) runs the same statement lazily, so applying this at
-- deploy time only spares the first request a DDL round trip.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-31-alert-notes.sql
--
-- Run as the cluster's "application" superuser. SET ROLE keeps every object
-- dovecote-owned. For a staging apply, use SET ROLE dovecote_staging.
SET ROLE dovecote;

ALTER TABLE alert_definitions ADD COLUMN IF NOT EXISTS notes TEXT;

RESET ROLE;
