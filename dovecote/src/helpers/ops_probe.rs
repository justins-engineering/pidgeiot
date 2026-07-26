use time::OffsetDateTime;
use tokio_postgres::{Client, types::Type};
use worker::{Delay, Env, Fetch, Method, Request, Result, console_error, console_log};

use super::alerts::send_via_resend;
use super::hyperdrive::get_db_client;

/// Which row in `ops_health_state` this probe owns. A constant rather than a
/// parameter because Kratos is currently the only off-edge service worth
/// probing from the Worker -- if a second one appears, generalize the public
/// entry point instead of duplicating this module.
const SERVICE: &str = "kratos";

/// Ops-notification recipient (`OPS_ALERT_EMAIL` var). Deliberately the
/// probe's ONLY enable switch: the var is set in production's `[vars]` block
/// alone, so staging/dev cron invocations (which fire the same `#[event
/// (scheduled)]` handler every 5 minutes) never probe, never write state,
/// and never email. One knob, not a separate "enabled" flag that could
/// drift out of sync with the recipient.
fn ops_alert_email(env: &Env) -> Option<String> {
  env
    .var("OPS_ALERT_EMAIL")
    .ok()
    .map(|v| v.to_string())
    .filter(|s| !s.trim().is_empty())
}

/// One GET against Kratos's public readiness endpoint, reusing the
/// `KRATOS_BROWSER_URL` var the Worker already holds for session
/// validation -- the probe watches the exact origin real logins depend on,
/// through the same Tunnel path a browser takes, not some side channel.
async fn kratos_ready_once(base: &str) -> bool {
  let url = format!("{}/health/ready", base.trim_end_matches('/'));
  let Ok(req) = Request::new(&url, Method::Get) else {
    return false;
  };
  match Fetch::Request(req).send().await {
    Ok(resp) => resp.status_code() == 200,
    Err(_) => false,
  }
}

/// Probe Kratos's readiness from the 5-minute Cron Trigger and email
/// `OPS_ALERT_EMAIL` on state TRANSITIONS only (healthy -> down, down ->
/// recovered), tracked in the `ops_health_state` Postgres table -- so an
/// outage produces one "DOWN" email and one "recovered" email, not a page
/// every 5 minutes. A single failed GET is retried once after 3s before
/// counting as down, so a lone network blip doesn't flap the state.
///
/// Everything here is best-effort/logged, never an error to the caller --
/// same "never fail the scheduled invocation" convention as
/// `evaluate_scheduled_alerts` (see `scheduled.rs`). Postgres being
/// unreachable is intentionally NOT reported through this path: the alert
/// sweep right before this call already screams into the logs about that,
/// and this probe's own email delivery would likely be the least of the
/// platform's problems.
pub async fn probe_kratos_health(env: &Env) {
  let Some(recipient) = ops_alert_email(env) else {
    return;
  };
  let Ok(base) = env.var("KRATOS_BROWSER_URL") else {
    console_error!("ops probe: KRATOS_BROWSER_URL unset -- cannot probe");
    return;
  };
  let base = base.to_string();

  let mut healthy = kratos_ready_once(&base).await;
  if !healthy {
    Delay::from(std::time::Duration::from_secs(3)).await;
    healthy = kratos_ready_once(&base).await;
  }

  let client = match get_db_client(env).await {
    Ok(c) => c,
    Err(e) => {
      console_error!("ops probe: db open failed, state not recorded: {e}");
      return;
    }
  };

  if let Err(e) = record_and_notify(env, &client, &recipient, healthy).await {
    console_error!("ops probe: state/notify step failed: {e}");
  }
}

async fn ensure_ops_health_table(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS ops_health_state (
         service TEXT PRIMARY KEY,
         healthy BOOLEAN NOT NULL,
         since TIMESTAMPTZ NOT NULL DEFAULT now(),
         updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
       );",
    )
    .await
    .map_err(|e| {
      console_error!("ops_health_state bootstrap error: {e}");
      worker::Error::RustError("Internal Server Error".into())
    })
}

async fn record_and_notify(
  env: &Env,
  client: &Client,
  recipient: &str,
  healthy: bool,
) -> Result<()> {
  ensure_ops_health_table(client).await?;

  let prior = client
    .query_typed(
      "SELECT healthy, since FROM ops_health_state WHERE service = $1;",
      &[(&SERVICE, Type::TEXT)],
    )
    .await
    .map_err(|e| worker::Error::RustError(format!("ops state read: {e}")))?;

  let transition = match prior.first() {
    None => {
      client
        .query_typed(
          "INSERT INTO ops_health_state (service, healthy) VALUES ($1, $2);",
          &[(&SERVICE, Type::TEXT), (&healthy, Type::BOOL)],
        )
        .await
        .map_err(|e| worker::Error::RustError(format!("ops state insert: {e}")))?;
      // First-ever observation: only noteworthy if the service is already
      // down (a healthy first row is just bootstrap, not a transition).
      (!healthy).then_some(None)
    }
    Some(row) => {
      let was_healthy: bool = row.get("healthy");
      let since: OffsetDateTime = row.get("since");
      if was_healthy == healthy {
        client
          .query_typed(
            "UPDATE ops_health_state SET updated_at = now() WHERE service = $1;",
            &[(&SERVICE, Type::TEXT)],
          )
          .await
          .map_err(|e| worker::Error::RustError(format!("ops state touch: {e}")))?;
        None
      } else {
        client
          .query_typed(
            "UPDATE ops_health_state
             SET healthy = $2, since = now(), updated_at = now()
             WHERE service = $1;",
            &[(&SERVICE, Type::TEXT), (&healthy, Type::BOOL)],
          )
          .await
          .map_err(|e| worker::Error::RustError(format!("ops state update: {e}")))?;
        Some(Some(since))
      }
    }
  };

  let Some(prior_since) = transition else {
    return Ok(());
  };

  let (subject, text) = if healthy {
    let downtime = prior_since
      .map(|s| {
        let mins = (OffsetDateTime::now_utc() - s).whole_minutes().max(0);
        format!(" after ~{mins} min down")
      })
      .unwrap_or_default();
    (
      "[OPS] recovered: Kratos auth is healthy again".to_string(),
      format!(
        "https://auth.pidgeiot.com/health/ready is returning 200 again{downtime}.\n\nNo action needed; this closes the earlier DOWN notification."
      ),
    )
  } else {
    (
      "[OPS] DOWN: Kratos auth is failing its readiness probe".to_string(),
      "https://auth.pidgeiot.com/health/ready failed twice in a row (initial + 3s retry) from the production Worker's 5-minute cron.\n\nLogins, signups, and every dashboard API call depend on this origin. Check the VPS: docker/kratos status, cloudflared tunnel, and the Crunchy DSN.\n\nYou will get exactly one 'recovered' email when it passes again -- no repeats in between."
        .to_string(),
    )
  };

  console_log!("ops probe: kratos transition -> healthy={healthy}, notifying");
  send_via_resend(env, recipient, &subject, &text).await
}
