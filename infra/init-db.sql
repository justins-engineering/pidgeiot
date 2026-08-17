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
  -- User-definable GreptimeDB/InfluxDB forwarding target — NULL when unset
  -- (the common case). Mirrors the DO's own `pigeons.telemetry_endpoint`
  -- column; see capsules::TelemetryEndpoint.
  telemetry_endpoint JSONB,
  -- This pigeon's own Zephyr CONFIG_BOARD_TARGET string, e.g.
  -- "circuitdojo_feather/nrf9160/ns" -- NULL until an operator tags it (at
  -- provisioning or via update). Mirrors the DO's own
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

-- PIGEON TELEMETRY HISTORY Table
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

-- FLOCK FIRMWARE Table
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
  -- The Zephyr CONFIG_BOARD_TARGET this image was built for -- required at
  -- upload time going forward (see dovecote's POST
  -- /flocks/:flock_id/firmware), NULL only for rows uploaded before this
  -- column existed. Compared against pigeons.board before a shadow
  -- firmware assignment is accepted; see capsules::FirmwareImage::board.
  board TEXT,
  uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (flock_id, sha256)
);

-- Idempotent for pre-existing databases that created `flock_firmware`
-- before this column existed.
ALTER TABLE flock_firmware ADD COLUMN IF NOT EXISTS board TEXT;

-- ALERT DEFINITIONS Table
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

-- ALERT STATE Table
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

-- ORGANIZATIONS -- shared-org access for teams (individual
-- Kratos accounts, org-level RBAC, membership-row revocation; no literal
-- shared accounts, no Ory Keto). See capsules::Organization/OrgRole and
-- dovecote's helpers/orgs.rs. A flock is EXACTLY one of user-owned
-- (org_id NULL) or org-owned (org_id set); org-owned flocks' pigeons also
-- carry a pigeon_acl row whose entity_id IS the org id.
-- Billing hangs off the org rather than the flock: an org is the only
-- entity that survives a change of individual owner and can hold a team's
-- payment relationship. Everything Stripe owns lives in these columns;
-- usage aggregation stays in our own tables, with Stripe's meter as a
-- reporting sink. `billing_event_at` is the Stripe event timestamp that
-- last wrote this row -- Stripe delivers events unordered, so an older
-- event must not overwrite a newer one (see dovecote's
-- helpers/billing.rs::apply_subscription). See capsules::BillingPlan /
-- SubscriptionStatus for the vocabularies `plan` and
-- `subscription_status` hold.
CREATE TABLE IF NOT EXISTS organizations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  stripe_customer_id TEXT,
  stripe_subscription_id TEXT,
  plan TEXT NOT NULL DEFAULT 'perch',
  subscription_status TEXT NOT NULL DEFAULT 'none',
  current_period_start TIMESTAMPTZ,
  current_period_end TIMESTAMPTZ,
  cancel_at_period_end BOOLEAN NOT NULL DEFAULT false,
  billing_event_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Idempotent for pre-existing databases that created `organizations`
-- before billing existed.
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_subscription_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'perch';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS subscription_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_start TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_end TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS cancel_at_period_end BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS billing_event_at TIMESTAMPTZ;

-- Webhook idempotency. Stripe retries a delivery for up to three days and
-- can send the same event more than once even after a 2xx, so every
-- delivery is recorded here before anything is applied and `processed_at`
-- is set only once the apply succeeded -- a delivery that dies mid-apply
-- is therefore retried rather than being suppressed by its own claim.
-- `redelivery_count` is incremented by the claiming upsert, making a
-- repeatedly-failing event visible without a separate log trawl.
CREATE TABLE IF NOT EXISTS stripe_webhook_events (
  event_id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  event_created TIMESTAMPTZ NOT NULL,
  livemode BOOLEAN NOT NULL DEFAULT false,
  api_version TEXT,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  processed_at TIMESTAMPTZ,
  redelivery_count INTEGER NOT NULL DEFAULT 0
);

-- Billable-message usage aggregation -- one row per billing account per
-- billing period. Our Postgres is the source of truth for usage; Stripe's
-- meters are a reporting sink fed from these rows (dovecote's
-- helpers/usage.rs). The account is either an organization (org-owned
-- flocks) or a bare user (personal flocks -- no org, no subscription,
-- always free tier). Period anchoring: the org's Stripe current_period
-- bounds while a live subscription covers now(); calendar month otherwise.
-- warned_at/paused_at are the free-tier fuse's once-per-period bookkeeping
-- (80% warning email sent / allowance first crossed), claimed atomically so
-- concurrent queue consumers can't double-send.
CREATE TABLE IF NOT EXISTS billing_usage_periods (
  owner_kind TEXT NOT NULL CHECK (owner_kind IN ('org', 'user')),
  owner_id UUID NOT NULL,
  period_start TIMESTAMPTZ NOT NULL,
  period_end TIMESTAMPTZ NOT NULL,
  billable_messages BIGINT NOT NULL DEFAULT 0,
  warned_at TIMESTAMPTZ,
  paused_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (owner_kind, owner_id, period_start)
);

-- Claimed rollups reported to Stripe's billing meters (dovecote's
-- helpers/usage.rs reporter). A row is a claim first and a delivery record
-- second: claimed rows count toward "already reported" whether or not the
-- POST landed, so a transient Stripe failure undercounts (logged) rather
-- than ever double-billing. `meter` is our internal name; Stripe's own
-- event_name is resolved at run time from the price catalog.
CREATE TABLE IF NOT EXISTS billing_meter_reports (
  org_id UUID NOT NULL,
  period_start TIMESTAMPTZ NOT NULL,
  report_day DATE NOT NULL,
  meter TEXT NOT NULL CHECK (meter IN ('messages', 'devices')),
  quantity BIGINT NOT NULL,
  stripe_identifier TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  posted_at TIMESTAMPTZ,
  PRIMARY KEY (org_id, period_start, report_day, meter)
);

-- Cadence gate for the meter reporter, which rides the existing 5-minute
-- cron: one row, claimed once per ~day.
CREATE TABLE IF NOT EXISTS billing_reporter_state (
  id SMALLINT PRIMARY KEY,
  last_run_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS organization_members (
  org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  user_id UUID NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
  -- Denormalized at join time (same convention as flocks.owner_email) so
  -- the dashboard can show who a member is without a Kratos admin-API call
  -- from the edge.
  email TEXT,
  -- Inviting user's Kratos id (NULL for the founding owner) -- the
  -- per-person audit trail.
  invited_by UUID,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (org_id, user_id)
);

-- App-level invites (self-hosted Kratos has no B2B invite flow). Only the
-- sha256 hex hash of the invite token is ever stored -- the cleartext
-- token appears exactly once, in the create response / invite email.
-- Single-use (accepted_at set on consumption), short expiry.
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

-- Org-owned flock marker. ON DELETE SET NULL is a safety net only -- org
-- deletion is refused while any org-owned flock exists.
ALTER TABLE flocks ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES organizations(id) ON DELETE SET NULL;

-- Denormalized flock-owner email -- needed to resolve an alert
-- notification's recipient without a Kratos admin-API call from the edge
-- (none is reachable from staging/prod today). NULL until a follow-up
-- wires `require_auth`/`create_user_flock` to populate it from the
-- session's own `identity.traits` (already fetched, currently discarded,
-- on every authenticated request) -- see docs/design/alerts-triggers.md
-- §3.4 and dovecote's helpers/alerts.rs::resolve_alert_recipient, which
-- already reads this column and degrades to "no recipient, log and skip"
-- until it's populated.
ALTER TABLE flocks ADD COLUMN IF NOT EXISTS owner_email TEXT;

-- Indexes
CREATE INDEX IF NOT EXISTS idx_flocks_user_id ON flocks(user_id);
CREATE INDEX IF NOT EXISTS idx_flocks_org_id ON flocks(org_id) WHERE org_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_organization_members_user_id ON organization_members(user_id);
CREATE INDEX IF NOT EXISTS idx_organization_invites_org_id ON organization_invites(org_id);
CREATE INDEX IF NOT EXISTS idx_alert_definitions_pigeon ON alert_definitions(pigeon_id) WHERE pigeon_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_alert_definitions_flock ON alert_definitions(flock_id) WHERE flock_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_alert_definitions_user_id ON alert_definitions(user_id);
CREATE INDEX IF NOT EXISTS idx_flock_firmware_flock_id ON flock_firmware(flock_id);
CREATE INDEX IF NOT EXISTS idx_pigeons_flock_id ON pigeons(flock_id);
CREATE INDEX IF NOT EXISTS idx_pigeon_acl_entity_id ON pigeon_acl(entity_id);
CREATE INDEX IF NOT EXISTS idx_pigeon_acl_id ON pigeon_acl(id);
CREATE INDEX IF NOT EXISTS idx_pigeon_telemetry_history_pigeon_reported ON pigeon_telemetry_history(pigeon_id, reported_at);
CREATE INDEX IF NOT EXISTS idx_pigeon_telemetry_history_key ON pigeon_telemetry_history(key);
-- Unique so a Stripe customer/subscription can never map to two orgs --
-- the webhook applies state by matching on these, and an ambiguous match
-- would bill the wrong tenant.
CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_customer ON organizations(stripe_customer_id) WHERE stripe_customer_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_subscription ON organizations(stripe_subscription_id) WHERE stripe_subscription_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_stripe_webhook_events_unprocessed ON stripe_webhook_events(received_at) WHERE processed_at IS NULL;
