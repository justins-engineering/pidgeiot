use worker::{Env, ScheduleContext, ScheduledEvent, console_error, console_log, event};

use crate::helpers::{
  evaluate_scheduled_alerts, probe_kratos_health, report_billing_meters, sweep_error_retention,
  sweep_pending_tax_ids, sweep_telemetry_history_retention,
};

/// Cron-Trigger entry point (`[triggers] crons`, `wrangler.toml`) for the
/// missing-heartbeat / device-state alert sweep -- absence-of-data
/// conditions (`DeviceState`, `MissingReport`) can't be triggered by an
/// ingest event by definition, since nothing arrives to trigger them.
///
/// Mirrors `queue.rs`'s `queue_consumer` in spirit (a thin
/// `#[event(...)]` entry point that delegates the real work to a
/// `helpers::` function), but returns `()`, not `Result<()>` -- Workers'
/// scheduled-handler glue has no retry/ack concept, so there's nothing to
/// propagate an `Err` to. `evaluate_scheduled_alerts` is itself
/// best-effort/logged throughout; this wrapper just makes sure nothing it
/// returns escapes to crash the invocation.
#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
  console_log!("Scheduled alert sweep firing for cron '{}'", event.cron());

  if let Err(e) = evaluate_scheduled_alerts(&env).await {
    console_error!("Scheduled alert sweep failed: {e}");
  }

  // Kratos readiness probe (helpers/ops_probe.rs) -- rides the same cron
  // rather than its own trigger because the Cloudflare account allows only
  // 5 cron triggers total and dovecote prod+staging already consume 2.
  // Self-gated: a no-op anywhere OPS_ALERT_EMAIL isn't configured
  // (production [vars] only), so staging/dev invocations don't double-probe
  // or double-email. Internally best-effort/logged, same as the sweep above.
  probe_kratos_health(&env).await;

  // Telemetry-history retention sweep (helpers/retention.rs, task #66) --
  // same "ride the existing cron" reasoning as the probe above, and the
  // same crash-proofing as the alert sweep: internally best-effort/logged,
  // so a DB hiccup here doesn't take out the two invocations above it.
  if let Err(e) = sweep_telemetry_history_retention(&env).await {
    console_error!("Telemetry history retention sweep failed: {e}");
  }

  // Error-report retention (helpers/errors.rs) -- same ride-the-cron
  // reasoning and the same crash-proofing as the sweeps above.
  if let Err(e) = sweep_error_retention(&env).await {
    console_error!("Error-report retention sweep failed: {e}");
  }

  // Stripe meter reporter (helpers/usage.rs) -- rides the same cron behind
  // its own ~daily cadence claim (`billing_reporter_state`), for the same
  // 5-cron-trigger account limit the probe above documents. Skips cleanly
  // where STRIPE_SECRET_KEY isn't configured; internally best-effort/logged.
  if let Err(e) = report_billing_meters(&env).await {
    console_error!("Billing meter report failed: {e}");
  }

  // VAT re-check sweep (helpers/business_details.rs) -- the other half of
  // "a VIES outage never blocks a save". A registration we could not settle
  // at save time is stored `pending`, and this is what eventually settles
  // it. Same ride-the-cron reasoning and the same crash-proofing as the
  // sweeps above; internally rate-limited to one attempt per org per hour.
  if let Err(e) = sweep_pending_tax_ids(&env).await {
    console_error!("Pending VAT re-check sweep failed: {e}");
  }
}
