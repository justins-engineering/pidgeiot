-- Migration: an organization's own timezone, so the emails we send about
-- it are stamped in the local time its members work in rather than only in
-- UTC. See capsules/src/email.rs (formatting) and
-- dovecote/src/helpers/timezone.rs (the database that answers it).
--
-- One zone per ORGANIZATION, not per person: an alert about a device is
-- about a place, and everybody looking at that fleet reasons in the same
-- wall clock. It also keeps the timezone database on the server, where one
-- copy serves everyone, instead of in every dashboard download.
--
-- TEXT holding an IANA zone name ('America/New_York'). No CHECK constraint:
-- the valid set is the tz database, which is revised several times a year,
-- and a constraint written today would start refusing legitimate zones the
-- moment the database moves. dovecote validates every write against the
-- real database instead, and an unresolvable stored value falls back to
-- UTC at send time rather than failing the send.
--
-- Every existing org gets 'UTC', which is exactly what its emails said
-- before this column existed, so nothing changes until somebody chooses a
-- zone.
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime `ensure_org_tables`
-- runs the same statement lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-26-org-timezone.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned. For a staging apply, use
-- SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE organizations
  ADD COLUMN IF NOT EXISTS timezone TEXT NOT NULL DEFAULT 'UTC';

RESET ROLE;
