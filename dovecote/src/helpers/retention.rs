use tokio_postgres::types::Type;
use worker::{Env, Result, console_error, console_log};

use crate::helpers::demo_pigeon_ids;
use crate::helpers::get_db_client;
use crate::helpers::telemetry::ensure_telemetry_history_table;

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

/// Sweeps `pigeon_telemetry_history` for rows past their account's
/// retention rung, called from the existing scheduled-event handler
/// (`scheduled.rs`) rather than a second Cron Trigger -- the Cloudflare
/// account allows only 5 cron triggers total and dovecote prod+staging
/// already consume 2 (see `scheduled.rs`'s comment on `probe_kratos_health`
/// for the same constraint). Best-effort/logged, like every other function
/// that rides this cron: a failed sweep must not take down the alert
/// evaluation or health probe that share the same invocation.
///
/// The rungs are the pricing page's published ladder
/// (`fancier/src/views/pricing.rs`, the tier cards): 7 days free,
/// 30 on Builder, 90 on Growth, 13 months on Scale/Fleet -- thirteen
/// months rather than twelve so a year-over-year comparison has both ends
/// of its window. The rung is resolved per row THROUGH the entitlement
/// order: subscription status first (must be one of the statuses
/// `capsules::SubscriptionStatus::is_entitled` accepts -- keep the SQL
/// list in sync with it), and only then the plan column. Reading `plan`
/// alone would grant a cancelled org its paid retention forever. A
/// non-entitled or org-less account takes the free rung; an entitled org
/// with an unrecognized plan value ALSO falls to the free rung via the
/// CASE's ELSE -- deleting at the shortest window is the one direction
/// that can't be repaired later, but an unknown plan string can only come
/// from our own provisioning, and serving it indefinite retention would
/// hide the defect instead.
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
  // The rung CASE reads the org billing columns and would otherwise fail
  // (not fail-open -- the whole sweep errors) in a database the billing
  // migration hasn't reached; bootstrapping both here makes deploy order
  // irrelevant, and heals the tally/fuse paths' fail-open undercounting in
  // the same stroke, within one cron interval of any deploy.
  crate::helpers::ensure_billing_tables(&client).await?;
  crate::helpers::ensure_billing_usage_tables(&client).await?;

  let exclude = demo_pigeon_ids(env);

  // The subselect + `id IN (...)` is Postgres's standard stand-in for
  // `DELETE ... LIMIT`, which doesn't exist -- `id` is `BIGSERIAL PRIMARY
  // KEY`, so the inner scan can use the primary key index rather than a
  // second full predicate evaluation.
  let deleted = client
    .execute_typed(
      "DELETE FROM pigeon_telemetry_history
       WHERE id IN (
         SELECT h.id
         FROM pigeon_telemetry_history h
         JOIN pigeons p ON p.id = h.pigeon_id
         JOIN flocks f ON f.id = p.flock_id
         LEFT JOIN organizations o ON o.id = f.org_id
         WHERE NOT (h.pigeon_id = ANY($1))
           AND h.reported_at < now() - CASE
             WHEN o.id IS NULL
               OR o.subscription_status NOT IN ('trialing', 'active', 'past_due')
               THEN interval '7 days'
             WHEN o.plan = 'builder' THEN interval '30 days'
             WHEN o.plan = 'growth' THEN interval '90 days'
             WHEN o.plan IN ('scale', 'fleet') THEN interval '13 months'
             ELSE interval '7 days'
           END
         LIMIT $2
       );",
      &[
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
      "Telemetry history retention: deleted {deleted} row(s) past their tier's rung (batch limit {RETENTION_BATCH_LIMIT})"
    );
  }

  Ok(())
}
