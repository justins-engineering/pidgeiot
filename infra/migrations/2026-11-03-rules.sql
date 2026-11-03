-- Migration: the tables behind user-authored rules (dovecote's helpers/rule_store.rs and
-- helpers/rules.rs). Source lives here rather than in R2 because a version row and its source
-- belong in one transaction, and Cloudflare keeps no versions of a user Worker.
--
-- Idempotent: safe to run against an already-migrated database. dovecote's runtime
-- `ensure_rule_tables` runs the same statements when a table or index is missing, and a test
-- holds this file, init-db.sql and that function's text together.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-11-03-rules.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything created comes out
-- app-owned. For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

CREATE TABLE IF NOT EXISTS rules (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  flock_id UUID NOT NULL REFERENCES flocks(id) ON DELETE CASCADE,
  created_by UUID NOT NULL,
  name TEXT NOT NULL,
  recipients JSONB NOT NULL DEFAULT '[]',
  enabled BOOLEAN NOT NULL DEFAULT false,
  deployed_version INTEGER,
  pending_version INTEGER,
  paused_reason TEXT,
  blocked_at TIMESTAMPTZ,
  blocked_reason TEXT,
  fail_count INTEGER NOT NULL DEFAULT 0,
  fail_since TIMESTAMPTZ,
  watch_until TIMESTAMPTZ,
  mail_hour TIMESTAMPTZ,
  mail_in_hour INTEGER NOT NULL DEFAULT 0,
  mail_day DATE,
  mail_in_day INTEGER NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- The dispatch gate joins a pigeon's flock to its rules on every batch.
CREATE INDEX IF NOT EXISTS idx_rules_flock_id ON rules(flock_id);
CREATE TABLE IF NOT EXISTS rule_versions (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  language TEXT NOT NULL DEFAULT 'js' CHECK (language = 'js'),
  source TEXT NOT NULL CHECK (octet_length(source) <= 65536),
  sha256 TEXT NOT NULL,
  contract INTEGER NOT NULL,
  origin TEXT NOT NULL CHECK (origin IN ('hand', 'assistant', 'assistant_edited')),
  based_on INTEGER,
  repair_depth SMALLINT NOT NULL DEFAULT 0,
  prompt TEXT,
  model TEXT,
  input_tokens INTEGER,
  output_tokens INTEGER,
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  tested_sha256 TEXT,
  tested_contract INTEGER,
  tested_at TIMESTAMPTZ,
  test_summary JSONB,
  created_by UUID NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (rule_id, version)
);
-- scope is '' for the flock document, else a pigeon id.
CREATE TABLE IF NOT EXISTS rule_state (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  scope TEXT NOT NULL,
  doc JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (rule_id, scope)
);
-- decided_at is the last reading of the batch that changed the flag, so an older batch never
-- undoes a newer one.
CREATE TABLE IF NOT EXISTS rule_alert_state (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  scope TEXT NOT NULL,
  key TEXT NOT NULL,
  firing BOOLEAN NOT NULL,
  decided_at TIMESTAMPTZ NOT NULL,
  changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_sent_at TIMESTAMPTZ,
  PRIMARY KEY (rule_id, scope, key)
);
CREATE TABLE IF NOT EXISTS rule_values (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  value_num DOUBLE PRECISION,
  at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (rule_id, pigeon_id, key, at)
);
-- Keyed on the batch so a redelivered batch keeps the first delivery's row.
CREATE TABLE IF NOT EXISTS rule_runs (
  rule_id UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  pigeon_id TEXT NOT NULL REFERENCES pigeons(id) ON DELETE CASCADE,
  batch_id TEXT NOT NULL,
  status TEXT NOT NULL,
  readings INTEGER NOT NULL DEFAULT 0,
  alerts_sent INTEGER NOT NULL DEFAULT 0,
  alerts_suppressed INTEGER NOT NULL DEFAULT 0,
  values_written INTEGER NOT NULL DEFAULT 0,
  state_changed BOOLEAN NOT NULL DEFAULT false,
  wall_ms INTEGER,
  error TEXT,
  logs JSONB,
  ran_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (rule_id, pigeon_id, batch_id)
);
CREATE INDEX IF NOT EXISTS idx_rule_runs_rule_ran ON rule_runs(rule_id, ran_at);
-- What is true of a flock's script rather than of one rule, and its deploy lease.
CREATE TABLE IF NOT EXISTS rule_bundles (
  flock_id UUID PRIMARY KEY REFERENCES flocks(id) ON DELETE CASCADE,
  contract INTEGER NOT NULL,
  account_tag TEXT NOT NULL,
  uploaded_at TIMESTAMPTZ,
  deploy_until TIMESTAMPTZ NOT NULL DEFAULT 'epoch'
);
-- scope is an account uuid or 'platform'. No foreign key: deleting a rule or a flock refunds
-- no assistant allowance and no mail.
CREATE TABLE IF NOT EXISTS rule_usage (
  scope TEXT NOT NULL,
  day DATE NOT NULL,
  assistant_turns INTEGER NOT NULL DEFAULT 0,
  input_tokens BIGINT NOT NULL DEFAULT 0,
  output_tokens BIGINT NOT NULL DEFAULT 0,
  cache_read_tokens BIGINT NOT NULL DEFAULT 0,
  cache_write_tokens BIGINT NOT NULL DEFAULT 0,
  cost_micro_usd BIGINT NOT NULL DEFAULT 0,
  mail_sent INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (scope, day)
);

RESET ROLE;
