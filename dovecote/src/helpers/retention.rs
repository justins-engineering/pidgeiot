use time::{Duration, OffsetDateTime};
use tokio_postgres::types::Type;
use worker::{Env, Result, console_error, console_log};

use crate::helpers::demo_pigeon_ids;
use crate::helpers::get_db_client;
use crate::helpers::telemetry::ensure_telemetry_history_table;

/// Retention window for the platform-default telemetry-history store
/// (`pigeon_telemetry_history`) -- task #66. Flat and plan-independent for
/// now, not a placeholder for laziness: production `organizations` has no
/// `plan`/`subscription_status` column at all yet (the billing migration
/// only runs off a webhook that has never fired in prod) and every
/// `flocks.org_id` is NULL, so two of the three hops from a history row to
/// a plan don't exist -- every account today genuinely is free tier, so 7
/// days is its real entitlement. It also matches the only rung with data
/// old enough to exercise: nothing in production is past 30 days yet, so
/// the pricing page's other rungs (30/90/13mo, `fancier/src/views/
/// pricing.rs`) would be untested code with no real row to prove it
/// against until months from now.
const RETENTION_DAYS: i64 = 7;

/// Rows deleted per cron invocation. The sweep rides the existing 5-minute
/// cron (see `sweep_telemetry_history_retention`'s own doc comment), and a
/// Cloudflare cron invocation has a CPU budget a single DELETE must fit
/// inside -- so one run only ever clears a bounded slice of what's
/// overdue, and successive runs converge as long as this comfortably
/// outpaces new rows aging into the window. Measured production growth is
/// ~15,000 rows/day; this batch is more than an order of magnitude above
/// that per run, and at 288 runs/day the sweep has enormous headroom even
/// if a run is occasionally skipped.
const RETENTION_BATCH_LIMIT: i64 = 20_000;

/// Sweeps `pigeon_telemetry_history` for rows past `RETENTION_DAYS`,
/// called from the existing scheduled-event handler (`scheduled.rs`)
/// rather than a second Cron Trigger -- the Cloudflare account allows only
/// 5 cron triggers total and dovecote prod+staging already consume 2 (see
/// `scheduled.rs`'s comment on `probe_kratos_health` for the same
/// constraint). Best-effort/logged, like every other function that rides
/// this cron: a failed sweep must not take down the alert evaluation or
/// health probe that share the same invocation.
///
/// The demo pigeon (`DEMO_PIGEON_IDS`) is excluded from the delete
/// predicate on purpose, not an oversight: it is ~87% of this table, sits
/// on the owner's own free-tier account, and deletion here is one-way --
/// a future longer-range demo chart or a screenshot showing a month of
/// real data needs the history to still exist, and the demo page itself
/// only ever reads a 6-hour window regardless of how much sits behind it.
/// Revisitable, but only ever forward (toward eventually sweeping it too),
/// never used as precedent for a second silent exemption.
pub async fn sweep_telemetry_history_retention(env: &Env) -> Result<()> {
  let client = get_db_client(env).await?;
  ensure_telemetry_history_table(&client).await?;

  let cutoff = OffsetDateTime::now_utc() - Duration::days(RETENTION_DAYS);
  let exclude = demo_pigeon_ids(env);

  // The subselect + `id IN (...)` is Postgres's standard stand-in for
  // `DELETE ... LIMIT`, which doesn't exist -- `id` is `BIGSERIAL PRIMARY
  // KEY`, so the inner scan can use the primary key index rather than a
  // second full predicate evaluation.
  let deleted = client
    .execute_typed(
      "DELETE FROM pigeon_telemetry_history
       WHERE id IN (
         SELECT id FROM pigeon_telemetry_history
         WHERE reported_at < $1
           AND NOT (pigeon_id = ANY($2))
         LIMIT $3
       );",
      &[
        (&cutoff, Type::TIMESTAMPTZ),
        (&exclude, Type::TEXT_ARRAY),
        (&RETENTION_BATCH_LIMIT, Type::INT8),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history retention sweep failed: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  if deleted > 0 {
    console_log!(
      "Telemetry history retention: deleted {deleted} row(s) older than {RETENTION_DAYS}d (batch limit {RETENTION_BATCH_LIMIT})"
    );
  }

  Ok(())
}
