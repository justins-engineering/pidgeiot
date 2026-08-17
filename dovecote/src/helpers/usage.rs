use capsules::{BillingPlan, SubscriptionStatus};
use time::OffsetDateTime;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Env, Error, Result, console_error, console_log};

use crate::helpers::get_db_client;

/// The tier an account is actually served at, right now. The entitlement
/// gate comes FIRST: a cancelled or unpaid org keeps a tier in its `plan`
/// column (the recoverable states deliberately remember it), so reading
/// `plan` without checking status would serve a dead subscription its old
/// allowance forever -- silently, and in the customer's favour, so nothing
/// would surface it. No org row at all is a personal account, which cannot
/// hold a subscription -- free tier.
pub fn effective_plan(org_plan: Option<&str>, org_status: Option<&str>) -> BillingPlan {
  let entitled = org_status
    .and_then(|raw| raw.parse::<SubscriptionStatus>().ok())
    .is_some_and(|status| status.is_entitled());
  if !entitled {
    return BillingPlan::Perch;
  }
  org_plan
    .and_then(|raw| raw.parse().ok())
    .unwrap_or(BillingPlan::Perch)
}

/// One counted report: which account it landed on, where the count now
/// stands, and the org billing columns needed to interpret it.
struct BillableMessageTally {
  owner_kind: String,
  owner_id: Uuid,
  period_start: OffsetDateTime,
  billable_messages: i64,
  org_plan: Option<String>,
  org_status: Option<String>,
}

/// Counts one billable device message against the owning account's current
/// billing period, creating the period row on first use. Resolution and
/// upsert are one statement so the queue consumer pays a single round
/// trip: pigeon -> flock -> (org | user), anchored to the org's Stripe
/// period when a live subscription covers now(), calendar month otherwise.
///
/// `None` means the pigeon has no Postgres mirror row (the mirror is
/// best-effort by design), so there is no account to bill -- an undercount
/// in the customer's favour, not an error.
async fn record_billable_message(
  client: &Client,
  pigeon_id: &str,
) -> Result<Option<BillableMessageTally>> {
  let rows = client
    .query_typed(
      "WITH target AS (
         SELECT
           CASE WHEN f.org_id IS NULL THEN 'user' ELSE 'org' END AS owner_kind,
           COALESCE(f.org_id, f.user_id) AS owner_id,
           (o.id IS NOT NULL
             AND o.subscription_status IN ('trialing', 'active', 'past_due')
             AND o.current_period_start IS NOT NULL
             AND o.current_period_end IS NOT NULL
             AND now() >= o.current_period_start
             AND now() < o.current_period_end) AS use_org_period,
           o.current_period_start AS org_period_start,
           o.current_period_end AS org_period_end,
           o.plan AS org_plan,
           o.subscription_status AS org_status
         FROM pigeons p
         JOIN flocks f ON f.id = p.flock_id
         LEFT JOIN organizations o ON o.id = f.org_id
         WHERE p.id = $1
       ),
       upserted AS (
         INSERT INTO billing_usage_periods
           (owner_kind, owner_id, period_start, period_end, billable_messages)
         SELECT
           owner_kind,
           owner_id,
           CASE WHEN use_org_period THEN org_period_start
                ELSE date_trunc('month', now()) END,
           CASE WHEN use_org_period THEN org_period_end
                ELSE date_trunc('month', now()) + interval '1 month' END,
           1
         FROM target
         ON CONFLICT (owner_kind, owner_id, period_start) DO UPDATE
           SET billable_messages = billing_usage_periods.billable_messages + 1,
               period_end = EXCLUDED.period_end,
               updated_at = now()
         RETURNING owner_kind, owner_id, period_start, billable_messages
       )
       SELECT u.owner_kind, u.owner_id, u.period_start, u.billable_messages,
              t.org_plan, t.org_status
       FROM upserted u CROSS JOIN target t;",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Billable message tally error for '{pigeon_id}': {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(|row| BillableMessageTally {
    owner_kind: row.get("owner_kind"),
    owner_id: row.get("owner_id"),
    period_start: row.get("period_start"),
    billable_messages: row.get("billable_messages"),
    org_plan: row.get("org_plan"),
    org_status: row.get("org_status"),
  }))
}

/// Counts one billable device message and, for free-tier accounts, runs
/// the once-per-period threshold bookkeeping (80% warning email, allowance
/// crossing stamp). Entirely best-effort: this runs in the queue consumer,
/// off the device path, and a failure here logs and undercounts -- it must
/// never fail or delay ingestion.
pub async fn count_billable_message(env: &Env, pigeon_id: &str) {
  let client = match get_db_client(env).await {
    Ok(client) => client,
    Err(e) => {
      console_error!("Billable message tally skipped for '{pigeon_id}': {e}");
      return;
    }
  };

  let tally = match record_billable_message(&client, pigeon_id).await {
    Ok(Some(tally)) => tally,
    // No Postgres mirror row for this pigeon -- nothing to bill against.
    Ok(None) => return,
    Err(_) => return,
  };

  // Paid tiers convert over-allowance usage into metered overage (the
  // reporter's job); only the free tier warns and pauses.
  if effective_plan(tally.org_plan.as_deref(), tally.org_status.as_deref()) != BillingPlan::Perch {
    return;
  }

  let allowance = BillingPlan::Perch.included_messages();

  if tally.billable_messages >= allowance {
    stamp_perch_pause(&client, &tally, allowance).await;
  }

  // 8/10 in integers: the allowance is a round constant, so no precision
  // is lost, and it keeps the comparison in the same i64 domain as the
  // counter itself.
  if tally.billable_messages >= allowance * 8 / 10 {
    send_perch_warning(env, &client, &tally, allowance).await;
  }
}

/// Records the moment a free-tier account first crossed its allowance --
/// observability for the ingest-path fuse, which enforces from the counter
/// itself rather than from this stamp. Claimed atomically so concurrent
/// consumers stamp exactly once.
async fn stamp_perch_pause(client: &Client, tally: &BillableMessageTally, allowance: i64) {
  match client
    .query_typed(
      "UPDATE billing_usage_periods
       SET paused_at = now(), updated_at = now()
       WHERE owner_kind = $1 AND owner_id = $2 AND period_start = $3
         AND paused_at IS NULL AND billable_messages >= $4
       RETURNING paused_at;",
      &[
        (&tally.owner_kind, Type::TEXT),
        (&tally.owner_id, Type::UUID),
        (&tally.period_start, Type::TIMESTAMPTZ),
        (&allowance, Type::INT8),
      ],
    )
    .await
  {
    Ok(rows) if !rows.is_empty() => {
      console_log!(
        "Perch fuse: {} {} crossed the free-tier message allowance ({allowance}) for the period starting {}",
        tally.owner_kind,
        tally.owner_id,
        tally.period_start
      );
    }
    Ok(_) => {}
    Err(e) => console_error!(
      "Perch pause stamp failed for {} {}: {e}",
      tally.owner_kind,
      tally.owner_id
    ),
  }
}

/// Sends the one-per-period 80% warning email, if this consumer wins the
/// `warned_at` claim. Losing the claim (another consumer got there first,
/// or the mail already went out) is silent by design.
async fn send_perch_warning(
  env: &Env,
  client: &Client,
  tally: &BillableMessageTally,
  allowance: i64,
) {
  let claimed = match client
    .query_typed(
      "UPDATE billing_usage_periods
       SET warned_at = now(), updated_at = now()
       WHERE owner_kind = $1 AND owner_id = $2 AND period_start = $3
         AND warned_at IS NULL AND billable_messages >= $4
       RETURNING billable_messages;",
      &[
        (&tally.owner_kind, Type::TEXT),
        (&tally.owner_id, Type::UUID),
        (&tally.period_start, Type::TIMESTAMPTZ),
        (&(allowance * 8 / 10), Type::INT8),
      ],
    )
    .await
  {
    Ok(rows) => !rows.is_empty(),
    Err(e) => {
      console_error!(
        "Perch warning claim failed for {} {}: {e}",
        tally.owner_kind,
        tally.owner_id
      );
      false
    }
  };

  if !claimed {
    return;
  }

  let Some(recipient) = resolve_billing_recipient(client, &tally.owner_kind, &tally.owner_id).await
  else {
    console_error!(
      "Perch warning: no recipient resolvable for {} {} -- warning claimed but not sent",
      tally.owner_kind,
      tally.owner_id
    );
    return;
  };

  let root_url = env
    .var("ROOT_URL")
    .map(|v| v.to_string())
    .unwrap_or_else(|_| "https://pidgeiot.com".to_string());

  let subject = "PidgeIoT: free tier message allowance at 80%";
  let text = format!(
    "Your PidgeIoT account has used {} of its {} included device messages for the current period.\n\n\
     If usage reaches 100%, telemetry ingestion pauses for the rest of the period. Devices using the \
     pigeon library keep unsent readings queued and deliver them once ingestion resumes, so data is \
     delayed rather than lost.\n\n\
     Upgrading to a paid tier lifts the limit: {root_url}/pricing/\n",
    tally.billable_messages, allowance
  );

  if let Err(e) = super::alerts::send_via_usesend(env, &recipient, subject, &text).await {
    console_error!(
      "Perch warning email send failed for {} {}: {e}",
      tally.owner_kind,
      tally.owner_id
    );
  }
}

/// The address billing notices go to, mirroring the alert system's
/// recipient convention: an org notice goes to the earliest owner with an
/// email on record (`organization_members.email`, denormalized at join
/// time); a personal account's goes to its flocks' denormalized
/// `owner_email`.
async fn resolve_billing_recipient(
  client: &Client,
  owner_kind: &str,
  owner_id: &Uuid,
) -> Option<String> {
  let sql = if owner_kind == "org" {
    "SELECT email AS recipient FROM organization_members
     WHERE org_id = $1 AND role = 'owner' AND email IS NOT NULL
     ORDER BY created_at ASC LIMIT 1;"
  } else {
    "SELECT owner_email AS recipient FROM flocks
     WHERE user_id = $1 AND owner_email IS NOT NULL
     ORDER BY created_at ASC LIMIT 1;"
  };

  match client.query_typed(sql, &[(owner_id, Type::UUID)]).await {
    Ok(rows) => rows.first().and_then(|row| row.get("recipient")),
    Err(e) => {
      console_error!("Billing recipient lookup failed for {owner_kind} {owner_id}: {e}");
      None
    }
  }
}

#[cfg(test)]
mod tests {
  use super::effective_plan;
  use capsules::BillingPlan;

  #[test]
  fn status_gates_before_plan_is_read() {
    // The trap this exists to prevent: a cancelled org still carries a
    // plan column value in recoverable-adjacent states, and past writes --
    // reading it without the status gate would serve the old allowance
    // forever.
    assert_eq!(
      effective_plan(Some("growth"), Some("canceled")),
      BillingPlan::Perch
    );
    assert_eq!(
      effective_plan(Some("fleet"), Some("unpaid")),
      BillingPlan::Perch
    );
    assert_eq!(
      effective_plan(Some("scale"), Some("incomplete_expired")),
      BillingPlan::Perch
    );
    assert_eq!(
      effective_plan(Some("builder"), Some("none")),
      BillingPlan::Perch
    );
  }

  #[test]
  fn entitled_statuses_serve_the_stored_plan() {
    assert_eq!(
      effective_plan(Some("growth"), Some("active")),
      BillingPlan::Growth
    );
    assert_eq!(
      effective_plan(Some("scale"), Some("trialing")),
      BillingPlan::Scale
    );
    // PastDue stays entitled while Stripe retries the card.
    assert_eq!(
      effective_plan(Some("fleet"), Some("past_due")),
      BillingPlan::Fleet
    );
  }

  #[test]
  fn missing_org_or_unparseable_values_resolve_free() {
    assert_eq!(effective_plan(None, None), BillingPlan::Perch);
    assert_eq!(effective_plan(Some("growth"), None), BillingPlan::Perch);
    assert_eq!(
      effective_plan(Some("growth"), Some("some_future_state")),
      BillingPlan::Perch
    );
    assert_eq!(
      effective_plan(Some("not-a-plan"), Some("active")),
      BillingPlan::Perch
    );
  }
}
