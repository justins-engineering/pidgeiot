use worker::{Env, ScheduleContext, ScheduledEvent, console_error, console_log, event};

use crate::helpers::evaluate_scheduled_alerts;

/// Cron-Trigger entry point (`[triggers] crons`, `wrangler.toml`, task
/// #38) for the missing-heartbeat / device-state alert sweep
/// `docs/design/alerts-triggers.md` §2.4 calls out as needing a genuinely
/// separate scheduled evaluator -- absence-of-data conditions
/// (`DeviceState`, `MissingReport`) can't be triggered by an ingest event
/// by definition, since nothing arrives to trigger them.
///
/// Mirrors `queue.rs`'s `queue_consumer` in spirit (a thin
/// `#[event(...)]` entry point that delegates the real work to a
/// `helpers::` function), but this handler's signature returns `()`, not
/// `Result<()>` -- Workers' scheduled-handler glue has no retry/ack
/// concept the way a queue message does, so there's nothing to propagate
/// an `Err` to. `evaluate_scheduled_alerts` is itself best-effort/logged
/// throughout; this wrapper's only remaining job is making sure nothing it
/// returns escapes to crash the invocation, same "never fail the caller"
/// convention every other best-effort sync in this codebase already
/// follows (root CLAUDE.md).
#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
  console_log!("Scheduled alert sweep firing for cron '{}'", event.cron());

  if let Err(e) = evaluate_scheduled_alerts(&env).await {
    console_error!("Scheduled alert sweep failed: {e}");
  }
}
