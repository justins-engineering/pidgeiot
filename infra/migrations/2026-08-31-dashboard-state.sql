-- Migration: per-account dashboard preferences (`GET`/`PUT`/`DELETE
-- /dashboard-state/:scope_key`, see docs/api.md). Until now the dashboard's
-- saved telemetry graphs lived only in the browser's own localStorage, so a
-- browser set to clear site data on close destroyed them.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_dashboard_state_table` (helpers/dashboard_state.rs) runs the same
-- statement lazily, so applying this at deploy time only spares the first
-- request a DDL round trip.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-31-dashboard-state.sql
--
-- Run as the cluster's "application" superuser. SET ROLE makes every
-- object dovecote-owned from creation, so the app role can use what this
-- creates without a post-hoc ownership transfer (the ALTER ... OWNER TO
-- line below stays as idempotent self-healing for any run that skipped
-- this). For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- One row per (account, scope). The value is opaque: the platform stores
-- and returns the document verbatim and never reads inside it, which is
-- what lets a new widget claim a key without a migration. JSONB rather
-- than TEXT so the column itself refuses anything that is not JSON.
--
-- `user_id` is the Kratos identity, not an organization: a saved graph is
-- how one person chose to look at a fleet, not a fact about the fleet.
-- Account-deletion erasure is a plain delete, since every row here is that
-- person's own preference and nobody else's record:
--   DELETE FROM dashboard_state WHERE user_id = '<id>';
--
-- The primary key is the only index the table needs -- the point read and
-- the per-account key count are both served by it -- and the caps on key
-- length, key count and document size are enforced in the route
-- (capsules::dashboard_state) rather than as column types, so the numbers
-- are declared once.
CREATE TABLE IF NOT EXISTS dashboard_state (
  user_id UUID NOT NULL,
  scope_key TEXT NOT NULL,
  value JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, scope_key)
);

-- Ownership transfer -- see the header. Production:
ALTER TABLE dashboard_state OWNER TO dovecote;
-- Staging (run this line instead, against the staging database):
-- ALTER TABLE dashboard_state OWNER TO dovecote_staging;

RESET ROLE;
