-- Migration: per-pigeon stamp of the last accepted device contact on any
-- transport (every authenticated device route, every WebSocket frame that
-- parses, and any NIDD uplink from the claimed device, stored or not; see
-- docs/api.md's GET /pigeons/:pigeon_id). Before this the flock list, the
-- pigeon page and the device-state alert each judged "seen" from different
-- evidence, and a device that only polled its shadow read Offline while it
-- was alive.
--
-- NULL until the pigeon's first contact after deploy; the pigeon's own
-- Durable Object holds the authoritative copy and seeds it from its older
-- evidence on its next wake. Written at most once a minute per pigeon by
-- dovecote's helpers/pigeons.rs::stamp_last_seen (the gateway after the DO
-- answers, or the telemetry queue consumer for WebSocket frames); read by
-- helpers/alerts.rs::resolve_pigeon_last_seen for the DeviceState sweep.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. dovecote's runtime `ensure_alert_tables`
-- runs the same statement before the sweep reads the column. Run it
-- before deploying: until the column exists every stamp fails and is logged.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-10-16-last-seen.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned. For a staging apply, use SET ROLE
-- dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE pigeons
  ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ;

RESET ROLE;
