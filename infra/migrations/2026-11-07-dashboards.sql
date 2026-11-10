-- Migration: custom dashboards (`/dashboards` routes, see docs/api.md's "Dashboards" section).
--
-- Idempotent -- safe to run repeatedly against an already-migrated database. dovecote's runtime
-- `ensure_dashboards_table` (helpers/dashboards.rs) runs the same statements, so applying this
-- at deploy time only spares the first request a DDL round trip. Apply it before running
-- scripts/purge-identities.py against a database, which reads this table.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-11-07-dashboards.sql
--
-- Run as the cluster's "application" superuser. SET ROLE makes every object dovecote-owned from
-- creation. For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

-- A dashboard belongs to exactly one person (`user_id`) or one organization (`org_id`), never
-- both: unlike a flock, which keeps its creator's `user_id` once it moves to an org, the CHECK
-- makes the two exclusive. The layout is validated in the route (capsules::validate_layout) and
-- holds only references, never telemetry.
--
-- Erasure: a personal row goes with its account; an org row goes with its org through the
-- cascade and outlives the person who made it, so `created_by` is nullable provenance that
-- scripts/purge-identities.py sets NULL.
--
-- `revision` is the optimistic lock two editors of one org dashboard share: a save names the
-- revision it loaded and loses to any save made since.
CREATE TABLE IF NOT EXISTS dashboards (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID,
  org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
  created_by UUID,
  name TEXT NOT NULL,
  layout JSONB NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK ((user_id IS NULL) <> (org_id IS NULL))
);
CREATE INDEX IF NOT EXISTS idx_dashboards_user ON dashboards(user_id) WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_dashboards_org ON dashboards(org_id) WHERE org_id IS NOT NULL;

-- Ownership transfer -- see the header. Production:
ALTER TABLE dashboards OWNER TO dovecote;
-- Staging (run this line instead, against the staging database):
-- ALTER TABLE dashboards OWNER TO dovecote_staging;

RESET ROLE;
