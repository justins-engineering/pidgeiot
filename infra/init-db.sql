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
  -- User-definable GreptimeDB/InfluxDB forwarding target (task #18) —
  -- NULL when unset (the common case). Mirrors the DO's own
  -- `pigeons.telemetry_endpoint` column; see capsules::TelemetryEndpoint.
  telemetry_endpoint JSONB,
  -- This pigeon's own Zephyr CONFIG_BOARD_TARGET string (task #20, phase
  -- 1), e.g. "circuitdojo_feather/nrf9160/ns" -- NULL until an operator
  -- tags it (at provisioning or via update). Mirrors the DO's own
  -- `pigeons.board` column; see capsules::Pigeon::board. Enforced against
  -- flock_firmware.board (below) by dovecote's
  -- check_firmware_board_compat before a firmware shadow assignment is
  -- accepted.
  board TEXT,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

-- Idempotent for pre-existing databases that created `pigeons` before these
-- columns existed — mirrors dovecote's own `ALTER TABLE ... ADD COLUMN`
-- fallback for the DO's SQLite schema (see objects/pigeons.rs).
ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS telemetry_endpoint JSONB;
ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS board TEXT;

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

-- PIGEON TELEMETRY HISTORY Table (task #18)
-- Written by the queue consumer alongside the DO's own latest-value-per-key
-- upsert (`pigeon_telemetry` in the DO's SQLite) -- this is the append-only
-- time-series counterpart, queried by GET /pigeons/:id/telemetry/history and
-- GET /flocks/:id/telemetry/history. Only written when the pigeon has no
-- user-defined telemetry_endpoint configured; when one is set, the consumer
-- forwards to it instead (see dovecote's queue.rs).
CREATE TABLE IF NOT EXISTS pigeon_telemetry_history (
  id BIGSERIAL PRIMARY KEY,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  value_num DOUBLE PRECISION,
  reported_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- FLOCK FIRMWARE Table (task #23)
-- Firmware images are shared across every pigeon in a flock (same hardware
-- fleet) rather than duplicated per-pigeon, so this catalog lives here
-- rather than in each pigeon's own DO (which also can't hold MB-sized
-- blobs -- see dovecote's CLAUDE.md). The actual binary lives in R2,
-- content-addressed by sha256 (key `firmware/<sha256>.bin`); this table is
-- metadata + per-flock visibility only. A pigeon's *assigned* firmware is a
-- separate, per-pigeon concern living in that pigeon's own shadow
-- (pigeon_shadow.target_config.firmware), not here.
CREATE TABLE IF NOT EXISTS flock_firmware (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  flock_id UUID NOT NULL REFERENCES flocks(id) ON DELETE CASCADE,
  version TEXT NOT NULL,
  size BIGINT NOT NULL,
  sha256 TEXT NOT NULL,
  -- The Zephyr CONFIG_BOARD_TARGET this image was built for (task #20,
  -- phase 1) -- required at upload time going forward (see dovecote's
  -- POST /flocks/:flock_id/firmware), NULL only for rows uploaded before
  -- this column existed. Compared against pigeons.board before a shadow
  -- firmware assignment is accepted; see capsules::FirmwareImage::board.
  board TEXT,
  uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (flock_id, sha256)
);

-- Idempotent for pre-existing databases that created `flock_firmware`
-- before this column existed.
ALTER TABLE flock_firmware ADD COLUMN IF NOT EXISTS board TEXT;

-- ALERT DEFINITIONS Table (task #32)
-- Postgres-only, not DO-mirrored -- same reasoning already applied to
-- flock_firmware above: this is dashboard-authored config with no
-- device-facing counterpart, and a flock-scoped alert has no DO to live in
-- at all (flocks have none). condition/channel are JSONB (not columns per
-- condition-type field), matching the existing polymorphic-config
-- convention this file already uses for pigeons.connector/
-- pigeons.telemetry_endpoint. Exactly one of flock_id/pigeon_id is set,
-- enforced by the CHECK constraint below (mirrors AlertScope being an
-- enum, not two independent optional fields, in capsules).
CREATE TABLE IF NOT EXISTS alert_definitions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL,
  flock_id UUID REFERENCES flocks(id) ON DELETE CASCADE,
  pigeon_id TEXT REFERENCES pigeons(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  condition JSONB NOT NULL,
  severity TEXT NOT NULL DEFAULT 'warning',
  channel JSONB NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT alert_definitions_scope_check CHECK (
    (flock_id IS NOT NULL AND pigeon_id IS NULL) OR
    (flock_id IS NULL AND pigeon_id IS NOT NULL)
  )
);

CREATE TRIGGER trigger_alert_definitions_updated_at
  BEFORE UPDATE ON alert_definitions
  FOR EACH ROW
  EXECUTE FUNCTION trigger_set_timestamp();

-- ALERT STATE Table (task #32)
-- Debounce/hysteresis + fired-state tracking (see capsules::AlertState) --
-- one row per (alert_definition_id, pigeon_id), not per definition, since a
-- flock-scoped alert fires/clears independently per pigeon it applies to.
-- Written/read entirely by dovecote's check_telemetry_alerts evaluator
-- (helpers/alerts.rs), no dashboard route reads/writes this directly today.
CREATE TABLE IF NOT EXISTS alert_state (
  alert_definition_id UUID NOT NULL REFERENCES alert_definitions(id) ON DELETE CASCADE,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'ok',
  first_true_at TIMESTAMPTZ,
  last_notified_at TIMESTAMPTZ,
  PRIMARY KEY (alert_definition_id, pigeon_id)
);

-- Denormalized flock-owner email (task #32, design doc §3.4) -- needed to
-- resolve an alert notification's recipient without a Kratos admin-API call
-- from the edge (none is reachable from staging/prod today). NULL until a
-- follow-up wires `require_auth`/`create_user_flock` to populate it from
-- the session's own `identity.traits` (already fetched, currently
-- discarded, on every authenticated request) -- see
-- docs/design/alerts-triggers.md §3.4 and dovecote's
-- helpers/alerts.rs::resolve_alert_recipient, which already reads this
-- column and degrades to "no recipient, log and skip" until it's populated.
ALTER TABLE flocks ADD COLUMN IF NOT EXISTS owner_email TEXT;

-- Indexes
CREATE INDEX IF NOT EXISTS idx_flocks_user_id ON flocks(user_id);
CREATE INDEX IF NOT EXISTS idx_alert_definitions_pigeon ON alert_definitions(pigeon_id) WHERE pigeon_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_alert_definitions_flock ON alert_definitions(flock_id) WHERE flock_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_alert_definitions_user_id ON alert_definitions(user_id);
CREATE INDEX IF NOT EXISTS idx_flock_firmware_flock_id ON flock_firmware(flock_id);
CREATE INDEX IF NOT EXISTS idx_pigeons_flock_id ON pigeons(flock_id);
CREATE INDEX IF NOT EXISTS idx_pigeon_acl_entity_id ON pigeon_acl(entity_id);
CREATE INDEX IF NOT EXISTS idx_pigeon_acl_id ON pigeon_acl(id);
CREATE INDEX IF NOT EXISTS idx_pigeon_telemetry_history_pigeon_reported ON pigeon_telemetry_history(pigeon_id, reported_at);
CREATE INDEX IF NOT EXISTS idx_pigeon_telemetry_history_key ON pigeon_telemetry_history(key);
