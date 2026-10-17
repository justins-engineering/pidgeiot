\set ON_ERROR_STOP on
-- Migration: a covering (pigeon_id, key, reported_at) index for the telemetry
-- history graph and raw queries (helpers/telemetry.rs), which otherwise fetch
-- every heap row in the range and filter by key afterwards. Replaces the
-- (key) index, which no dovecote statement uses without pigeon_id or a flock
-- join; never query this table by key alone.
--
-- The graphs read the newest rows, and an index-only scan serves those only
-- once a vacuum has marked their pages all-visible. The default insert trigger
-- (20% of the table) leaves about two weeks of rows unmarked, and the covering
-- index is then slower than none. A fixed threshold keeps the unmarked tail
-- under about 13 hours however large the table grows; a scale factor would
-- drift, since the demo pigeon's history is never swept.
--
-- Apply in an environment only AFTER its deployed dovecote carries this
-- change: ensure_telemetry_history_table must check the catalog before its
-- CREATE INDEX (an older one takes a SHARE lock per request, which deadlocks
-- the concurrent build) and must no longer create
-- idx_pigeon_telemetry_history_key (or the next request rebuilds it). For
-- staging that means after its branch includes this change, then with
-- SET ROLE dovecote_staging. Never in one transaction (no -1 /
-- --single-transaction): CONCURRENTLY refuses to run inside a transaction
-- block. Safe to rerun; it stops before the DROP if the new index is not
-- valid, and then says to drop it with DROP INDEX CONCURRENTLY first.
--
--   psql "$DOVECOTE_PSQL_CONNECTION" -f infra/migrations/2026-10-17-history-key-index.sql
--
-- Rollback: before deploying a dovecote older than this change, run
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_pigeon_telemetry_history_key
--     ON pigeon_telemetry_history (key);
-- or its first requests build that index under a lock that blocks inserts,
-- and concurrent ones fail on the name. The covering index can stay; older
-- code ignores it. After such a rollback, optionally
--   ALTER TABLE pigeon_telemetry_history
--     RESET (autovacuum_vacuum_insert_threshold, autovacuum_vacuum_insert_scale_factor);
--
-- Run as the cluster's "application" superuser with SET ROLE so the index
-- comes out app-owned.
SET ROLE dovecote;

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_pigeon_telemetry_history_pigeon_key_reported
  ON pigeon_telemetry_history (pigeon_id, key, reported_at) INCLUDE (value, value_num);

-- IF NOT EXISTS also matches an INVALID index left by a failed build.
DO $$ BEGIN
  IF NOT (SELECT indisvalid FROM pg_index
          WHERE indexrelid = 'idx_pigeon_telemetry_history_pigeon_key_reported'::regclass) THEN
    RAISE EXCEPTION 'covering index is INVALID: DROP INDEX CONCURRENTLY it, then rerun';
  END IF;
END $$;

ALTER TABLE pigeon_telemetry_history
  SET (autovacuum_vacuum_insert_threshold = 10000, autovacuum_vacuum_insert_scale_factor = 0);

DROP INDEX CONCURRENTLY IF EXISTS idx_pigeon_telemetry_history_key;

-- Marks the rows inserted since the last autovacuum now rather than at the
-- next autovacuum pass.
VACUUM pigeon_telemetry_history;

RESET ROLE;
