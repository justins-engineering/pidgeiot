-- Migration: per-pigeon hold on alert evaluation. An operator suspends one
-- pigeon (a sign unplugged for the day) and every alert definition skips
-- it, on ingest and in the scheduled sweep, without pausing a flock-scoped
-- definition for the flock's other pigeons. See
-- helpers/pigeons.rs::update_pigeon_suspension_pg_db (writes it, mirroring
-- the pigeon's own Durable Object row) and helpers/alerts.rs (reads it in
-- the ingest-time definition lookup and the sweep's scope resolution).
--
-- NULL for every existing pigeon, which reads as "live": nothing changes
-- for a fleet until an operator suspends something.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime `ensure_alert_tables`
-- and `ensure_pigeons_suspended_column` run the same statement lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-09-02-pigeon-suspension.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned (an ADD COLUMN on an already-owned table
-- needs no ownership transfer, but the role must be allowed to ALTER it).
-- For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE pigeons
  ADD COLUMN IF NOT EXISTS suspended_at TIMESTAMPTZ;

RESET ROLE;
