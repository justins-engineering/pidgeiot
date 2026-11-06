-- Migration: flock-level config. A flock carries one JSON object, its layer, that every member
-- pigeon inherits under its own config one top-level key at a time (docs/api.md, PUT
-- /flocks/:flock_id/config). The layer and its version live on the flocks row; each member's
-- Durable Object holds the copy it last took, and pigeon_shadow.flock_version mirrors which
-- version that is, for the flock page's "N of M on this version".
--
-- Existing rows take an empty layer at version 0, which changes nothing for any pigeon until its
-- flock is given a layer.
--
-- Idempotent -- safe to run repeatedly against an already-migrated database. dovecote's runtime
-- `ensure_flock_config_columns` runs the same statements when a column is missing. Run it before
-- deploying:
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-11-06-flock-config.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything created comes out
-- app-owned. For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE flocks
  ADD COLUMN IF NOT EXISTS target_config JSONB NOT NULL DEFAULT '{}';
ALTER TABLE flocks
  ADD COLUMN IF NOT EXISTS config_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pigeon_shadow
  ADD COLUMN IF NOT EXISTS flock_version INTEGER NOT NULL DEFAULT 0;

RESET ROLE;
