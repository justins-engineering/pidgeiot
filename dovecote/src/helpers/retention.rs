use capsules::BillingPlan;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Env, Result, console_error, console_log};

use crate::helpers::demo_pigeon_ids;
use crate::helpers::get_db_client;
use crate::helpers::telemetry::ensure_telemetry_history_table;
use crate::helpers::usage::served_plan;

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

/// The rung a personal account is swept at, and the one every account
/// without a subscription or a grant falls back to.
const FREE_RETENTION_RUNG: &str = "7 days";

/// One organization's rung, as a Postgres interval literal.
///
/// The ladder is the pricing page's published one
/// (`fancier/src/views/pricing.rs`, the tier cards): 7 days free, 30 on
/// Builder, 90 on Growth, 13 months on Scale/Fleet -- thirteen months
/// rather than twelve so a year-over-year comparison has both ends of its
/// window.
///
/// Which rung applies comes from `served_plan`, the same resolution the
/// entitlement gates and the meter reporter use, because the Privacy
/// Policy publishes retention "by the plan the organization is served at":
/// a complimentary organization keeps its granted tier's window with no
/// subscription behind it, and a suspended paid one falls to the free
/// tier's. Reading the plan column alone would grant a cancelled org its
/// paid retention forever; reading the subscription status alone would
/// sweep a comped org at seven days. An unparseable plan resolves to the
/// free tier there, which is deliberate: deleting at the shortest window
/// is the one direction that can't be repaired later, but an unknown plan
/// string can only come from our own provisioning, and serving it
/// indefinite retention would hide the defect instead.
fn retention_rung(
  org_plan: Option<&str>,
  org_status: Option<&str>,
  comp_plan: Option<&str>,
) -> &'static str {
  match served_plan(org_plan, org_status, comp_plan).plan {
    BillingPlan::Perch => FREE_RETENTION_RUNG,
    BillingPlan::Builder => "30 days",
    BillingPlan::Growth => "90 days",
    BillingPlan::Scale | BillingPlan::Fleet => "13 months",
  }
}

/// Every organization's rung, as the parallel id/interval arrays the sweep
/// joins on. Resolving in Rust rather than in the DELETE's own CASE is
/// what lets the rung come from `served_plan` instead of a second copy of
/// the entitlement order; one row per organization is small enough to
/// snapshot on every run.
async fn org_retention_rungs(client: &Client) -> Result<(Vec<Uuid>, Vec<&'static str>)> {
  let rows = client
    .query_typed(
      "SELECT id, plan, subscription_status, comp_plan FROM organizations;",
      &[],
    )
    .await
    .map_err(|e| {
      console_error!("Telemetry history retention org load failed: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })?;

  let mut ids = Vec::with_capacity(rows.len());
  let mut rungs = Vec::with_capacity(rows.len());
  for row in &rows {
    let plan: String = row.get("plan");
    let status: String = row.get("subscription_status");
    let comp_plan: Option<String> = row.get("comp_plan");
    ids.push(row.get::<_, Uuid>("id"));
    rungs.push(retention_rung(
      Some(&plan),
      Some(&status),
      comp_plan.as_deref(),
    ));
  }

  Ok((ids, rungs))
}

/// Sweeps `pigeon_telemetry_history` for rows past their account's
/// retention rung, called from the existing scheduled-event handler
/// (`scheduled.rs`) rather than a second Cron Trigger -- the Cloudflare
/// account allows only 5 cron triggers total and dovecote prod+staging
/// already consume 2 (see `scheduled.rs`'s comment on `probe_kratos_health`
/// for the same constraint). Best-effort/logged, like every other function
/// that rides this cron: a failed sweep must not take down the alert
/// evaluation or health probe that share the same invocation.
///
/// A flock with no organization is a personal account and takes the free
/// rung. A flock whose organization is missing from the rung snapshot
/// waits for the next run instead: that read can be served from
/// Hyperdrive's query cache, so an organization created in the last minute
/// may not be in it, and deleting on a stale absence is one-way.
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
  // The rung snapshot reads the org billing columns and would otherwise
  // fail (not fail-open -- the whole sweep errors) in a database the
  // billing migration hasn't reached; bootstrapping both here makes deploy
  // order irrelevant, and heals the tally/fuse paths' fail-open
  // undercounting in the same stroke, within one cron interval of any
  // deploy.
  crate::helpers::ensure_billing_tables(&client).await?;
  crate::helpers::ensure_billing_usage_tables(&client).await?;

  let exclude = demo_pigeon_ids(env);
  let (org_ids, org_rungs) = org_retention_rungs(&client).await?;

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
         LEFT JOIN unnest($3::uuid[], $4::text[]) AS r(org_id, rung)
           ON r.org_id = f.org_id
         WHERE NOT (h.pigeon_id = ANY($1))
           AND (f.org_id IS NULL OR r.rung IS NOT NULL)
           AND h.reported_at < now() - COALESCE(r.rung, $5)::interval
         LIMIT $2
       );",
      &[
        (&exclude, Type::TEXT_ARRAY),
        (&RETENTION_BATCH_LIMIT, Type::INT8),
        (&org_ids, Type::UUID_ARRAY),
        (&org_rungs, Type::TEXT_ARRAY),
        (&FREE_RETENTION_RUNG, Type::TEXT),
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn comped_org_keeps_its_granted_rung() {
    // No subscription, so the plan column is the free tier and only the
    // grant says otherwise.
    assert_eq!(
      retention_rung(Some("perch"), Some("none"), Some("growth")),
      "90 days"
    );
    assert_eq!(
      retention_rung(Some("perch"), Some("none"), Some("fleet")),
      "13 months"
    );
  }

  #[test]
  fn suspended_paid_org_falls_to_the_free_rung() {
    // The plan column deliberately remembers the tier through a lapse.
    assert_eq!(
      retention_rung(Some("growth"), Some("canceled"), None),
      FREE_RETENTION_RUNG
    );
    assert_eq!(
      retention_rung(Some("scale"), Some("unpaid"), None),
      FREE_RETENTION_RUNG
    );
  }

  #[test]
  fn live_subscription_outranks_a_grant() {
    assert_eq!(
      retention_rung(Some("builder"), Some("active"), Some("fleet")),
      "30 days"
    );
  }
}
