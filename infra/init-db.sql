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
  -- Last time this pigeon sent a billable message (telemetry, shadow
  -- report-back or log upload, on any transport). NULL until it first
  -- reports, which is exactly what makes a provisioned-but-idle device
  -- free: the extra-devices meter counts only pigeons whose stamp falls
  -- inside the billing period. Written by record_billable_message
  -- (dovecote's helpers/usage.rs), which refreshes it at most every six
  -- hours rather than once per message. Deliberately NOT the DO-owned
  -- updated_at, which also bumps on dashboard writes.
  last_billable_activity TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

-- Idempotent for pre-existing databases that created `pigeons` before these
-- columns existed — mirrors dovecote's own `ALTER TABLE ... ADD COLUMN`
-- fallback for the DO's SQLite schema (see objects/pigeons.rs).
ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS telemetry_endpoint JSONB;
ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS board TEXT;
ALTER TABLE pigeons ADD COLUMN IF NOT EXISTS last_billable_activity TIMESTAMPTZ;

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
  -- IANA zone name the org's emails are stamped in (capsules/src/email.rs).
  -- No CHECK constraint: the valid set is the tz database, which is
  -- revised several times a year -- dovecote validates writes against the
  -- real database instead.
  timezone TEXT NOT NULL DEFAULT 'UTC',
  stripe_customer_id TEXT,
  stripe_subscription_id TEXT,
  plan TEXT NOT NULL DEFAULT 'perch',
  subscription_status TEXT NOT NULL DEFAULT 'none',
  current_period_start TIMESTAMPTZ,
  current_period_end TIMESTAMPTZ,
  cancel_at_period_end BOOLEAN NOT NULL DEFAULT false,
  billing_event_at TIMESTAMPTZ,
  -- Complimentary tier grant: a tier slug this org is served for free,
  -- with no subscription behind it, plus why and when it was granted. A
  -- live subscription outranks it (see helpers/usage.rs::served_plan);
  -- revoking is setting comp_plan back to NULL. Granted by hand only --
  -- docs/infra/org-comps.md, no route writes these.
  comp_plan TEXT,
  comp_note TEXT,
  comp_granted_at TIMESTAMPTZ,
  -- Tax identity of the entity being invoiced. It lives here, and not on
  -- the Kratos identity, because the org is the billing entity: one person
  -- can belong to two orgs with two different registrations, and a tax
  -- field on the identity would face every hobbyist at signup.
  --
  -- `tax_id` is normalized (uppercase, separators stripped) and is NOT a
  -- secret -- a VAT number is on every invoice its owner issues, so no read
  -- path strips it. Logs still never carry it in full.
  --
  -- `tax_id_status` is 'none' | 'pending' | 'validated' | 'invalid' |
  -- 'unverified'. 'pending' is what lets a VIES outage cost the customer
  -- nothing: the number is stored, the answer is owed, and the 5-minute
  -- cron asks again. `tax_id_checked_at` (last attempt) paces that retry;
  -- `tax_id_validated_at` (last confirmation) is what the dashboard shows
  -- and is deliberately not disturbed by an inconclusive re-check.
  business_name TEXT,
  tax_id TEXT,
  tax_id_type TEXT NOT NULL DEFAULT 'none',
  tax_id_status TEXT NOT NULL DEFAULT 'none',
  tax_id_validated_at TIMESTAMPTZ,
  tax_id_checked_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Idempotent for pre-existing databases that created `organizations`
-- before billing existed.
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS timezone TEXT NOT NULL DEFAULT 'UTC';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_subscription_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'perch';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS subscription_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_start TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_end TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS cancel_at_period_end BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS billing_event_at TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS comp_plan TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS comp_note TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS comp_granted_at TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS business_name TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id TEXT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_type TEXT NOT NULL DEFAULT 'none';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_validated_at TIMESTAMPTZ;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_checked_at TIMESTAMPTZ;

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

-- Client error reports, signature-grouped (dovecote's POST /errors,
-- helpers/errors.rs). Groups are the normalized, redacted aggregate and
-- are kept indefinitely; events hold the raw capped detail and age out on
-- a 90-day received_at sweep. `user_id` is populated only for identified
-- manual reports (application/json with a note) -- automatic reports are
-- anonymous by construction, and the CHECK makes that structural. The
-- account-deletion runbook must DELETE FROM error_events WHERE user_id
-- matches the departing identity (same statement as DELETE /errors).
CREATE TABLE IF NOT EXISTS error_groups (
  signature TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  message TEXT NOT NULL,
  location TEXT,
  first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
  first_build TEXT,
  last_build TEXT,
  occurrences BIGINT NOT NULL DEFAULT 0,
  notified_at TIMESTAMPTZ,
  resolved_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS error_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  signature TEXT NOT NULL REFERENCES error_groups(signature) ON DELETE CASCADE,
  client_event_id UUID,
  occurred_at TIMESTAMPTZ NOT NULL,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  user_id UUID,
  message TEXT,
  route TEXT,
  build TEXT,
  user_agent TEXT,
  stack TEXT,
  breadcrumbs JSONB,
  report_note TEXT,
  CONSTRAINT error_events_identity_requires_note CHECK (user_id IS NULL OR report_note IS NOT NULL)
);

-- Public contact-form enquiries (`POST /contact`). The ops notification
-- email is the working surface; this row is what makes an enquiry
-- survive a mail-transport outage, and what makes "did anyone reply?"
-- answerable. No IP column by design: the rate limiter keys on
-- CF-Connecting-IP and discards it, and the sender's own address is the
-- only identifier needed to reply. Account-deletion erasure NULLs
-- `user_id` rather than deleting the row -- the enquiry is business
-- correspondence the sender addressed to us.
CREATE TABLE IF NOT EXISTS contact_submissions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  company TEXT,
  -- capsules::ContactFleetSize wire value, or NULL when unanswered.
  fleet_size TEXT,
  -- Which link opened the form ('fleet' from the pricing page's Fleet
  -- tier), so the funnel is attributable without asking the sender.
  about TEXT,
  message TEXT NOT NULL,
  -- Only when a session happened to be present; resolved server-side,
  -- never trusted from the body (same as error_events).
  user_id UUID,
  -- NULL means the enquiry landed but nobody was told.
  notified_at TIMESTAMPTZ
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
-- (none is reachable from staging/prod today). Written from the session's
-- own `identity.traits` at flock creation, and backfilled opportunistically
-- for older rows (dovecote's helpers/flocks.rs). NULL while neither has
-- happened yet, which `resolve_alert_recipient` (helpers/alerts.rs)
-- degrades on with "no recipient, log and skip".
ALTER TABLE flocks ADD COLUMN IF NOT EXISTS owner_email TEXT;

-- Marketing-consent history. The Kratos identity trait is the current
-- state and the person owns it; these rows are the evidence, written only
-- by dovecote's `POST /internal/consent` and never updated or deleted
-- while the account exists. Column-by-column reasoning, plus the erasure
-- and subject-access statements, live in
-- infra/migrations/2026-08-27-consent-events.sql.
CREATE TABLE IF NOT EXISTS consent_events (
  seq BIGSERIAL PRIMARY KEY,
  -- Kratos identity id. No FK: Kratos owns its own tables.
  identity_id UUID NOT NULL,
  -- capsules::MARKETING_EMAIL_PURPOSE ('marketing_emails') today; a
  -- second purpose is a new value here rather than a new table.
  purpose TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('granted', 'withdrawn')),
  source TEXT NOT NULL CHECK (source IN ('registration', 'settings', 'import')),
  -- The published privacy notice this consent was given against
  -- (capsules::PRIVACY_NOTICE_VERSION), stamped server-side.
  notice_version TEXT NOT NULL,
  flow_id UUID,
  -- The request context, both nullable and both unpopulated today. The
  -- privacy notice discloses addresses and user agents only as transient
  -- web logs kept for debugging and abuse prevention; keeping one against
  -- an identity as consent evidence is a different purpose with a
  -- different retention, so it needs its own line in the notice before
  -- the hook starts sending them. The columns exist so that switching
  -- them on is a config change rather than a migration --
  -- docs/consent.md has the two jsonnet lines it takes.
  ip TEXT,
  user_agent TEXT,
  at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Dashboard preferences, owned by the person rather than the browser.
-- Column-by-column reasoning lives in
-- infra/migrations/2026-08-31-dashboard-state.sql.
CREATE TABLE IF NOT EXISTS dashboard_state (
  -- Kratos identity id. No FK: Kratos owns its own tables.
  user_id UUID NOT NULL,
  scope_key TEXT NOT NULL,
  value JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, scope_key)
);

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
CREATE INDEX IF NOT EXISTS idx_error_events_signature ON error_events(signature);
-- Received (server clock), not occurred (client-claimed): the retention
-- sweep must never trust a client-stamped timestamp.
CREATE INDEX IF NOT EXISTS idx_error_events_received ON error_events(received_at);
-- Partial: the erasure path looks up identified rows only, and almost
-- every row is anonymous.
CREATE INDEX IF NOT EXISTS idx_error_events_user ON error_events(user_id) WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_error_groups_last_seen ON error_groups(last_seen DESC);
-- Unique so a Stripe customer/subscription can never map to two orgs --
-- the webhook applies state by matching on these, and an ambiguous match
-- would bill the wrong tenant.
CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_customer ON organizations(stripe_customer_id) WHERE stripe_customer_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_subscription ON organizations(stripe_subscription_id) WHERE stripe_subscription_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_stripe_webhook_events_unprocessed ON stripe_webhook_events(received_at) WHERE processed_at IS NULL;
-- Partial: the VAT re-check sweep's only query, oldest attempt first. Every
-- settled row would be dead weight here, and settled is the vast majority.
CREATE INDEX IF NOT EXISTS idx_organizations_tax_id_pending ON organizations(tax_id_checked_at) WHERE tax_id_status = 'pending';
CREATE INDEX IF NOT EXISTS idx_contact_submissions_received ON contact_submissions(received_at DESC);
-- Partial: the "landed but never mailed" sweep, and almost every row is
-- notified.
CREATE INDEX IF NOT EXISTS idx_contact_submissions_unnotified ON contact_submissions(received_at) WHERE notified_at IS NULL;
-- The only consent read: newest event for one identity and purpose, which
-- is what decides whether an incoming change is a transition worth
-- recording. DESC so that is a one-row backwards scan, not a sort.
CREATE INDEX IF NOT EXISTS idx_consent_events_identity ON consent_events(identity_id, purpose, seq DESC);
