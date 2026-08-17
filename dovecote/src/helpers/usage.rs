use capsules::{BillingPlan, SubscriptionStatus};
use time::OffsetDateTime;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Env, Error, Result, console_error, console_log};

use crate::helpers::get_db_client;

/// Runtime schema bootstrap, same lazy-DDL convention as
/// `ensure_billing_tables` (`helpers/billing.rs`) -- but deliberately
/// called only from the cron path, never from the ingest or queue-consumer
/// paths: those run per device report, where a DDL round trip per message
/// would be pure overhead. Until the migration (or the first cron
/// invocation) has created the tables, the hot paths fail open / undercount
/// and log, which is the customer-favorable direction.
pub async fn ensure_billing_usage_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS billing_usage_periods (
        owner_kind TEXT NOT NULL CHECK (owner_kind IN ('org', 'user')),
        owner_id UUID NOT NULL,
        period_start TIMESTAMPTZ NOT NULL,
        period_end TIMESTAMPTZ NOT NULL,
        billable_messages BIGINT NOT NULL DEFAULT 0,
        warned_at TIMESTAMPTZ,
        paused_at TIMESTAMPTZ,
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (owner_kind, owner_id, period_start)
      );
      CREATE TABLE IF NOT EXISTS billing_meter_reports (
        org_id UUID NOT NULL,
        period_start TIMESTAMPTZ NOT NULL,
        report_day DATE NOT NULL,
        meter TEXT NOT NULL CHECK (meter IN ('messages', 'devices')),
        quantity BIGINT NOT NULL,
        stripe_identifier TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        posted_at TIMESTAMPTZ,
        PRIMARY KEY (org_id, period_start, report_day, meter)
      );
      CREATE TABLE IF NOT EXISTS billing_reporter_state (
        id SMALLINT PRIMARY KEY,
        last_run_at TIMESTAMPTZ NOT NULL
      );",
    )
    .await
    .map_err(|e| {
      console_error!("Billing usage tables bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

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

// --- Stripe meter reporting ---
//
// Our Postgres rows above are the source of truth for usage; Stripe's
// meters are a reporting sink fed from them, so a processor migration
// re-points this reporter rather than reconstructing history.

/// One subscribed org the reporter considers: its Stripe identity, billing
/// columns, and the tallied message count for the current period.
struct BillableOrg {
  id: Uuid,
  customer_id: String,
  plan: Option<String>,
  status: String,
  period_start: OffsetDateTime,
  period_end: OffsetDateTime,
  billable_messages: i64,
}

/// Reports usage to Stripe's billing meters: daily message-overage deltas
/// (only usage above the tier allowance) and, near period end, the
/// billable extra-devices figure. Rides the existing 5-minute cron behind
/// its own ~daily cadence claim. Every hand-off to Stripe is claimed in
/// `billing_meter_reports` first, so a figure can be reported at most once
/// -- a failure after the claim undercounts (logged, replayable by hand
/// via the stored identifier) rather than ever double-billing.
pub async fn report_billing_meters(env: &Env) -> Result<()> {
  // No API key, nothing to report against -- checked before the cadence
  // claim so the daily slot isn't burned doing nothing.
  if !super::stripe_api::stripe_configured(env) {
    return Ok(());
  }

  let client = get_db_client(env).await?;
  ensure_billing_usage_tables(&client).await?;

  if !claim_reporter_run(&client).await? {
    return Ok(());
  }

  let orgs = load_billable_orgs(&client).await?;
  if orgs.is_empty() {
    return Ok(());
  }
  console_log!(
    "Billing meter reporter: considering {} subscribed org(s)",
    orgs.len()
  );

  // lookup_key -> meter event_name, resolved once per run per key.
  let mut event_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();

  for org in orgs {
    let plan = effective_plan(org.plan.as_deref(), Some(&org.status));
    if plan == BillingPlan::Perch {
      // Entitled but the plan column names no paid tier: refuse to meter
      // against a guess, same rule the webhook applies to plans.
      console_error!(
        "Billing meter reporter: org {} is entitled but resolves to the free tier -- skipping",
        org.id
      );
      continue;
    }
    report_message_overage(env, &client, &org, plan, &mut event_names).await;
    report_device_overage(env, &client, &org, plan, &mut event_names).await;
  }

  Ok(())
}

/// The ~daily cadence claim. 20 hours rather than 24 so a cron invocation
/// landing slightly earlier each day still runs daily instead of slipping
/// a day once the drift accumulates.
async fn claim_reporter_run(client: &Client) -> Result<bool> {
  let rows = client
    .query_typed(
      "INSERT INTO billing_reporter_state (id, last_run_at) VALUES (1, now())
       ON CONFLICT (id) DO UPDATE SET last_run_at = now()
         WHERE billing_reporter_state.last_run_at < now() - interval '20 hours'
       RETURNING last_run_at;",
      &[],
    )
    .await
    .map_err(|e| {
      console_error!("Billing reporter cadence claim error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(!rows.is_empty())
}

/// Orgs with a live Stripe relationship whose current period covers now,
/// joined to their tallied usage. Anything else has nothing meterable.
async fn load_billable_orgs(client: &Client) -> Result<Vec<BillableOrg>> {
  let rows = client
    .query_typed(
      "SELECT o.id, o.stripe_customer_id, o.plan, o.subscription_status,
              o.current_period_start, o.current_period_end,
              COALESCE(u.billable_messages, 0) AS billable_messages
       FROM organizations o
       LEFT JOIN billing_usage_periods u
         ON u.owner_kind = 'org' AND u.owner_id = o.id
        AND u.period_start = o.current_period_start
       WHERE o.stripe_customer_id IS NOT NULL
         AND o.subscription_status IN ('trialing', 'active', 'past_due')
         AND o.current_period_start IS NOT NULL
         AND o.current_period_end IS NOT NULL
         AND now() >= o.current_period_start
         AND now() < o.current_period_end;",
      &[],
    )
    .await
    .map_err(|e| {
      console_error!("Billing reporter org load error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .iter()
      .map(|row| BillableOrg {
        id: row.get("id"),
        customer_id: row.get("stripe_customer_id"),
        plan: row.get("plan"),
        status: row.get("subscription_status"),
        period_start: row.get("current_period_start"),
        period_end: row.get("current_period_end"),
        billable_messages: row.get("billable_messages"),
      })
      .collect(),
  )
}

/// Resolves (and memoizes for this run) the Stripe meter event_name behind
/// a metered price's lookup_key.
async fn meter_event_name(
  env: &Env,
  cache: &mut std::collections::HashMap<String, String>,
  price_lookup_key: &str,
) -> Option<String> {
  if let Some(name) = cache.get(price_lookup_key) {
    return Some(name.clone());
  }
  match super::stripe_api::resolve_meter_event_name(env, price_lookup_key).await {
    Ok(name) => {
      cache.insert(price_lookup_key.to_string(), name.clone());
      Some(name)
    }
    Err(e) => {
      console_error!(
        "Billing meter reporter: could not resolve meter for '{price_lookup_key}': {e}"
      );
      None
    }
  }
}

/// Claims and posts today's message-overage delta: total overage so far
/// this period, minus everything already claimed for the period.
async fn report_message_overage(
  env: &Env,
  client: &Client,
  org: &BillableOrg,
  plan: BillingPlan,
  event_names: &mut std::collections::HashMap<String, String>,
) {
  let overage = (org.billable_messages - plan.included_messages()).max(0);

  let already: i64 = match client
    .query_typed(
      "SELECT COALESCE(SUM(quantity), 0)::bigint AS already
       FROM billing_meter_reports
       WHERE org_id = $1 AND period_start = $2 AND meter = 'messages';",
      &[
        (&org.id, Type::UUID),
        (&org.period_start, Type::TIMESTAMPTZ),
      ],
    )
    .await
  {
    Ok(rows) => rows.first().map(|r| r.get("already")).unwrap_or(0),
    Err(e) => {
      console_error!(
        "Billing meter reporter: overage sum failed for org {}: {e}",
        org.id
      );
      return;
    }
  };

  let delta = overage - already;
  if delta > 0 {
    let identifier = format!(
      "msgov-{}-{}-{}",
      org.id,
      org.period_start.unix_timestamp(),
      OffsetDateTime::now_utc().date()
    );
    // ON CONFLICT DO NOTHING: a second run on the same day (forced, or a
    // cadence-gate reset) leaves the existing claim alone; the remaining
    // delta reports tomorrow.
    if let Err(e) = client
      .execute_typed(
        "INSERT INTO billing_meter_reports
           (org_id, period_start, report_day, meter, quantity, stripe_identifier)
         VALUES ($1, $2, CURRENT_DATE, 'messages', $3, $4)
         ON CONFLICT DO NOTHING;",
        &[
          (&org.id, Type::UUID),
          (&org.period_start, Type::TIMESTAMPTZ),
          (&delta, Type::INT8),
          (&identifier, Type::TEXT),
        ],
      )
      .await
    {
      console_error!(
        "Billing meter reporter: overage claim failed for org {}: {e}",
        org.id
      );
    }
  }

  if let Some(event_name) = meter_event_name(env, event_names, "message-overage").await {
    post_unposted_reports(env, client, org, "messages", &event_name).await;
  }
}

/// Within the final day of the period, claims and posts the billable
/// extra-devices count (devices above the tier's included count) -- once
/// per period, identifier keyed on org + period. A device removed or added
/// after this snapshot isn't re-reported; a missed final-day run
/// undercounts and logs, never bills late into the next period.
async fn report_device_overage(
  env: &Env,
  client: &Client,
  org: &BillableOrg,
  plan: BillingPlan,
  event_names: &mut std::collections::HashMap<String, String>,
) {
  if org.period_end - OffsetDateTime::now_utc() > time::Duration::days(1) {
    return;
  }

  let device_count: i64 = match client
    .query_typed(
      "SELECT COUNT(*)::bigint AS device_count
       FROM pigeons p JOIN flocks f ON f.id = p.flock_id
       WHERE f.org_id = $1;",
      &[(&org.id, Type::UUID)],
    )
    .await
  {
    Ok(rows) => rows.first().map(|r| r.get("device_count")).unwrap_or(0),
    Err(e) => {
      console_error!(
        "Billing meter reporter: device count failed for org {}: {e}",
        org.id
      );
      return;
    }
  };

  let extra = (device_count - plan.included_devices()).max(0);
  if extra > 0 {
    let identifier = format!("devov-{}-{}", org.id, org.period_start.unix_timestamp());
    let report_day = org.period_end.date();
    if let Err(e) = client
      .execute_typed(
        "INSERT INTO billing_meter_reports
           (org_id, period_start, report_day, meter, quantity, stripe_identifier)
         VALUES ($1, $2, $3, 'devices', $4, $5)
         ON CONFLICT DO NOTHING;",
        &[
          (&org.id, Type::UUID),
          (&org.period_start, Type::TIMESTAMPTZ),
          (&report_day, Type::DATE),
          (&extra, Type::INT8),
          (&identifier, Type::TEXT),
        ],
      )
      .await
    {
      console_error!(
        "Billing meter reporter: device claim failed for org {}: {e}",
        org.id
      );
    }
  }

  let device_price_key = format!("device-overage-{plan}");
  if let Some(event_name) = meter_event_name(env, event_names, &device_price_key).await {
    post_unposted_reports(env, client, org, "devices", &event_name).await;
  }
}

/// Marks this org+period's unposted claims posted, then hands each to
/// Stripe. Mark-before-post on purpose: the failure mode is an undercount
/// with a loud log line (the stored identifier makes a manual replay
/// safe), never a double-bill from re-posting a figure whose first POST
/// actually landed.
async fn post_unposted_reports(
  env: &Env,
  client: &Client,
  org: &BillableOrg,
  meter: &str,
  event_name: &str,
) {
  let rows = match client
    .query_typed(
      "UPDATE billing_meter_reports SET posted_at = now()
       WHERE org_id = $1 AND period_start = $2 AND meter = $3 AND posted_at IS NULL
       RETURNING quantity, stripe_identifier;",
      &[
        (&org.id, Type::UUID),
        (&org.period_start, Type::TIMESTAMPTZ),
        (&meter, Type::TEXT),
      ],
    )
    .await
  {
    Ok(rows) => rows,
    Err(e) => {
      console_error!(
        "Billing meter reporter: post-claim failed for org {} ({meter}): {e}",
        org.id
      );
      return;
    }
  };

  for row in rows {
    let quantity: i64 = row.get("quantity");
    let identifier: String = row.get("stripe_identifier");
    match super::stripe_api::post_meter_event(
      env,
      event_name,
      &org.customer_id,
      quantity,
      &identifier,
    )
    .await
    {
      Ok(()) => console_log!(
        "Billing meter reporter: posted {quantity} to '{event_name}' for org {} ({identifier})",
        org.id
      ),
      Err(e) => console_error!(
        "Billing meter reporter: meter event '{identifier}' claimed but POST failed (org {}, value {quantity}): {e}",
        org.id
      ),
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
