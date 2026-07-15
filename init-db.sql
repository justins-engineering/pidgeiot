CREATE ROLE dovecote WITH LOGIN PASSWORD 'secret';

CREATE DATABASE dovecote OWNER dovecote;

\c dovecote;

-- Reusable Trigger Functions
CREATE OR REPLACE FUNCTION trigger_set_timestamp()
RETURNS TRIGGER AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION trigger_prevent_immutable_updates()
RETURNS TRIGGER AS $$
BEGIN
  IF NEW.id <> OLD.id OR NEW.created_at <> OLD.created_at THEN
    RAISE EXCEPTION 'Cannot mutate immutable columns (id, created_at)';
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- FLOCKS Table (Control Plane)
CREATE TABLE IF NOT EXISTS flocks (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL,
  name TEXT NOT NULL,
  service_plan TEXT NOT NULL DEFAULT 'free',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TRIGGER trigger_flocks_updated_at
  BEFORE UPDATE ON flocks
  FOR EACH ROW
  EXECUTE FUNCTION trigger_set_timestamp();

CREATE TRIGGER trigger_flocks_immutable
  BEFORE UPDATE ON flocks
  FOR EACH ROW
  EXECUTE FUNCTION trigger_prevent_immutable_updates();

-- PIGEONS Table (Data Plane Registry)
-- connector is JSONB to store structured protocol config
-- Timestamps are set by the DO (source of truth) — no defaults or triggers
CREATE TABLE IF NOT EXISTS pigeons (
  id TEXT PRIMARY KEY,
  flock_id UUID NOT NULL REFERENCES flocks(id) ON DELETE CASCADE,
  serial TEXT,
  name TEXT,
  tags TEXT,
  connector JSONB NOT NULL,
  token_expires_at TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '1 year',
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

CREATE TRIGGER trigger_pigeons_immutable
  BEFORE UPDATE ON pigeons
  FOR EACH ROW
  EXECUTE FUNCTION trigger_prevent_immutable_updates();

-- PIGEON ACL Table
CREATE TABLE IF NOT EXISTS pigeon_acl (
  id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  entity_id UUID NOT NULL,
  role TEXT NOT NULL,
  PRIMARY KEY (id, entity_id)
);

-- PIGEON SHADOW Table
-- updated_at is BIGINT (unix epoch) for IoT/SOC compatibility
-- Values come from the DO (source of truth) — no triggers
CREATE TABLE IF NOT EXISTS pigeon_shadow (
  id TEXT PRIMARY KEY REFERENCES pigeons(id) ON DELETE CASCADE,
  target_version INTEGER DEFAULT 0,
  current_version INTEGER DEFAULT 0,
  target_config JSONB DEFAULT '{}',
  current_config JSONB DEFAULT '{}',
  updated_at BIGINT NOT NULL
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_flocks_user_id ON flocks(user_id);
CREATE INDEX IF NOT EXISTS idx_pigeons_flock_id ON pigeons(flock_id);
CREATE INDEX IF NOT EXISTS idx_pigeon_acl_entity_id ON pigeon_acl(entity_id);
CREATE INDEX IF NOT EXISTS idx_pigeon_acl_id ON pigeon_acl(id);
