-- Migration: business identity + tax registration on organizations, so an
-- invoice can be made out to the legal entity being billed rather than to
-- whichever person happened to click subscribe.
--
-- These live on `organizations` and not on the Kratos identity because the
-- org IS the billing entity: it already holds stripe_customer_id, it
-- survives a change of individual owner, and one person can belong to two
-- orgs with two different registrations. A tax id on the identity could
-- not express that, and it would put a VAT field in front of every
-- hobbyist at signup.
--
-- Six columns:
--   business_name        the registered legal name, if it differs from the
--                        org's display name (it usually does)
--   tax_id               the identifier, normalized -- uppercase, no
--                        separators. NOT a secret: a VAT number is printed
--                        on every invoice its owner issues, so nothing
--                        strips it on read. Logs still never carry it in
--                        full (capsules::tax_id_log_label).
--   tax_id_type          'none' | 'eu_vat' | 'other'. Only 'eu_vat' has an
--                        authority to check against.
--   tax_id_status        'none' | 'pending' | 'validated' | 'invalid' |
--                        'unverified'. 'pending' is the load-bearing one:
--                        VIES is famously flaky, and an outage must never
--                        block a customer from saving their own VAT
--                        number, so a lookup we could not complete stores
--                        the number and leaves this pending for the
--                        scheduled sweep to retry.
--   tax_id_validated_at  when VIES last CONFIRMED the number. Left alone by
--                        an inconclusive re-check, because "confirmed on
--                        the 3rd, unreachable since" is the truth.
--   tax_id_checked_at    when we last ASKED, answer or not. This is what
--                        paces the retry sweep; without it the 5-minute
--                        cron would re-ask VIES about every pending org
--                        every five minutes forever.
--
-- No CHECK constraints on the two enum columns, unlike
-- organization_members.role and like the billing columns beside them: the
-- values are written only from a Rust enum's own as_str(), never from free
-- text, and a CHECK would force every future variant to land in the
-- database strictly before the Worker that writes it -- the deploy ordering
-- the lazy-DDL pattern exists to avoid. capsules parses these permissively
-- (an unreadable status reads as 'pending', which is retried, rather than
-- as 'validated', which would be a claim we cannot support).
--
-- Idempotent -- safe to run repeatedly against an already-migrated
-- database. Belt-and-suspenders: dovecote's runtime
-- `ensure_business_details_columns` runs the same statements lazily.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-08-24-org-business-details.sql
--
-- Run as the cluster's "application" superuser with SET ROLE so anything
-- created comes out app-owned. For a staging apply, use
-- SET ROLE dovecote_staging instead.
SET ROLE dovecote;

ALTER TABLE organizations
  ADD COLUMN IF NOT EXISTS business_name TEXT,
  ADD COLUMN IF NOT EXISTS tax_id TEXT,
  ADD COLUMN IF NOT EXISTS tax_id_type TEXT NOT NULL DEFAULT 'none',
  ADD COLUMN IF NOT EXISTS tax_id_status TEXT NOT NULL DEFAULT 'none',
  ADD COLUMN IF NOT EXISTS tax_id_validated_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS tax_id_checked_at TIMESTAMPTZ;

-- The retry sweep's only query: oldest unanswered lookups first. Partial,
-- because every settled row is dead weight in this index and settled is
-- the overwhelming majority.
CREATE INDEX IF NOT EXISTS idx_organizations_tax_id_pending
  ON organizations(tax_id_checked_at)
  WHERE tax_id_status = 'pending';

RESET ROLE;
