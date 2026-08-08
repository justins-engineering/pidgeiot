-- Migration: organizations + org RBAC (task #12).
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Apply to EXISTING prod + staging databases (dovecote's own
-- runtime `ensure_org_tables` in helpers/orgs.rs runs the same statements
-- lazily, so this is belt-and-suspenders, but applying it explicitly at
-- deploy time avoids the first org request paying the DDL cost and keeps
-- init-db.sql the single documented schema).
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-08-organizations.sql
--
-- Schema body only -- no CREATE ROLE/DATABASE/\c bootstrap (that lives in
-- init-db.sql's first lines and must never be re-run against prod).

CREATE TABLE IF NOT EXISTS organizations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS organization_members (
  org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  user_id UUID NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
  email TEXT,
  invited_by UUID,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (org_id, user_id)
);

CREATE TABLE IF NOT EXISTS organization_invites (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  email TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
  token_hash TEXT NOT NULL UNIQUE,
  expires_at TIMESTAMPTZ NOT NULL,
  created_by UUID NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  accepted_at TIMESTAMPTZ
);

ALTER TABLE flocks ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES organizations(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_flocks_org_id ON flocks(org_id) WHERE org_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_organization_members_user_id ON organization_members(user_id);
CREATE INDEX IF NOT EXISTS idx_organization_invites_org_id ON organization_invites(org_id);
