-- Migration: the rule builder's versions. A builder version stores the model it was made from
-- beside the module dovecote wrote from it, and its origin is 'builder'. The assistant's model
-- name already holds the column `model`, hence `builder_model`.
--
-- Idempotent: safe to run against an already-migrated database. dovecote's runtime
-- `ensure_rule_tables` runs the same statements when the column is missing, and a test holds this
-- file, init-db.sql and that function's text together. Run after 2026-11-03-rules.sql.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-11-07-rule-builder.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything created comes out
-- app-owned. For a staging apply, use SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE rule_versions ADD COLUMN IF NOT EXISTS builder_model JSONB;
ALTER TABLE rule_versions DROP CONSTRAINT IF EXISTS rule_versions_origin_check;
ALTER TABLE rule_versions ADD CONSTRAINT rule_versions_origin_check
  CHECK (origin IN ('hand', 'assistant', 'assistant_edited', 'builder'));

RESET ROLE;
