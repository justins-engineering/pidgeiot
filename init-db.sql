-- Create the second database
CREATE DATABASE dovecote OWNER kratos;

-- Connect to the dovecote database
\c dovecote;

-- 1. Create Reusable Trigger Functions
-- Function to automatically bump the updated_at timestamp
CREATE OR REPLACE FUNCTION trigger_set_timestamp()
RETURNS TRIGGER AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Function to prevent mutation of IDs and creation dates
CREATE OR REPLACE FUNCTION trigger_prevent_immutable_updates()
RETURNS TRIGGER AS $$
BEGIN
  IF NEW.id <> OLD.id OR NEW.created_at <> OLD.created_at THEN
    RAISE EXCEPTION 'Cannot mutate immutable columns (id, created_at)';
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- 2. Create the FLOCKS Table (The Control Plane)
CREATE TABLE IF NOT EXISTS flocks (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL, -- Maps to the Ory Kratos User ID
  name TEXT NOT NULL,
  service_plan TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Apply Flocks Triggers
CREATE TRIGGER trigger_flocks_updated_at
  BEFORE UPDATE ON flocks
  FOR EACH ROW
  EXECUTE FUNCTION trigger_set_timestamp();

CREATE TRIGGER trigger_flocks_immutable
  BEFORE UPDATE ON flocks
  FOR EACH ROW
  EXECUTE FUNCTION trigger_prevent_immutable_updates();

-- 3. Create the PIGEONS Table (The Data Plane Registry)
CREATE TABLE IF NOT EXISTS pigeons (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(), -- Used to route to the Pigeon DO
  flock_id UUID NOT NULL REFERENCES flocks(id) ON DELETE CASCADE,
  name TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Apply Pigeons Triggers
CREATE TRIGGER trigger_pigeons_updated_at
  BEFORE UPDATE ON pigeons
  FOR EACH ROW
  EXECUTE FUNCTION trigger_set_timestamp();

CREATE TRIGGER trigger_pigeons_immutable
  BEFORE UPDATE ON pigeons
  FOR EACH ROW
  EXECUTE FUNCTION trigger_prevent_immutable_updates();

-- 4. Optimize with Indexes
-- YugabyteDB automatically indexes Primary Keys, so we only need to index Foreign Keys.
-- This ensures that querying "All Pigeons for Flock X" or "All Flocks for User Y" is lightning fast.
CREATE INDEX IF NOT EXISTS idx_flocks_user_id ON flocks(user_id);
CREATE INDEX IF NOT EXISTS idx_pigeons_flock_id ON pigeons(flock_id);
