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
///
/// `pigeons.last_billable_activity` is bootstrapped here despite living on
/// a non-billing table: it exists only to feed the extra-devices meter, and
/// this is the DDL path the meter itself runs behind.
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
      ALTER TABLE billing_usage_periods
        ADD COLUMN IF NOT EXISTS allowance_floor_messages BIGINT;
      ALTER TABLE pigeons
        ADD COLUMN IF NOT EXISTS last_billable_activity TIMESTAMPTZ;
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

/// The message allowance a period is charged against: the tier's own
/// allowance (or the period's recorded floor if that is higher), plus the
/// pool the account's billed extra devices carry with them.
///
/// The floor is the customer-favorable half of a mid-period plan change --
/// it holds the highest allowance of any tier the org was entitled to
/// during the period, so a downgrade never converts already-included usage
/// into overage retroactively (an upgrade's higher allowance is simply the
/// tier's own).
///
/// The extra-device pool deliberately sits outside the floor rather than
/// being recorded into it: it is recomputed from the live connected count
/// on every reporter run, and a downgrade only ever grows it (the lower
/// tier includes fewer devices, so more of the same fleet bills as extra).
/// There is nothing for a floor to protect on that half.
///
/// `connected_devices` is the count the per-device meter charges on, not
/// the provisioned count -- see `report_device_overage`. A free-tier
/// account contributes no extras at any count
/// (`BillingPlan::billed_extra_devices`), which is what keeps the ingest
/// fuse's threshold exactly the free tier's own allowance.
pub fn period_message_allowance(
  plan: BillingPlan,
  floor_messages: Option<i64>,
  connected_devices: i64,
) -> i64 {
  plan
    .included_messages()
    .max(floor_messages.unwrap_or(0))
    .saturating_add(plan.extra_device_messages(connected_devices))
}

/// Records the outgoing tier's allowance as the period's floor, before a
/// plan change is sent to Stripe -- write-first so a failure here refuses
/// the change instead of letting a downgrade bill the in-flight period at
/// the new, lower allowance. GREATEST keeps the highest floor across
/// repeated changes within one period; the insert arm covers a period that
/// has tallied no messages yet.
pub async fn raise_message_allowance_floor(
  client: &Client,
  org_id: &Uuid,
  period_start: OffsetDateTime,
  period_end: OffsetDateTime,
  floor_messages: i64,
) -> Result<()> {
  client
    .execute_typed(
      "INSERT INTO billing_usage_periods
         (owner_kind, owner_id, period_start, period_end, billable_messages,
          allowance_floor_messages)
       VALUES ('org', $1, $2, $3, 0, $4)
       ON CONFLICT (owner_kind, owner_id, period_start) DO UPDATE
         SET allowance_floor_messages =
               GREATEST(COALESCE(billing_usage_periods.allowance_floor_messages, 0), $4),
             updated_at = now();",
      &[
        (org_id, Type::UUID),
        (&period_start, Type::TIMESTAMPTZ),
        (&period_end, Type::TIMESTAMPTZ),
        (&floor_messages, Type::INT8),
      ],
    )
    .await
    .map(|_| ())
    .map_err(|e| {
      console_error!("Message allowance floor write failed for org {org_id}: {e}");
      Error::RustError("Internal Server Error".into())
    })
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
///
/// The same statement stamps `pigeons.last_billable_activity`, which is
/// what makes device-overage billing connected-only (see
/// `report_device_overage`). It rides here rather than in its own query
/// because this is the one place every billable surface already converges
/// on, and folding it into the existing CTE costs no extra round trip on a
/// path that runs per device report. Against a database that predates the
/// column the whole statement errors, so the tally undercounts and logs
/// until the cron's `ensure_billing_usage_tables` adds it -- the same
/// fail-open-and-undercount direction that bootstrap already documents.
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
       stamped AS (
         UPDATE pigeons p
         SET last_billable_activity = now()
         FROM target t
         WHERE p.id = $1
           AND (p.last_billable_activity IS NULL
             OR p.last_billable_activity < now() - interval '6 hours'
             OR p.last_billable_activity < CASE WHEN t.use_org_period
                                                THEN t.org_period_start
                                                ELSE date_trunc('month', now()) END)
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

/// The ingest-path fuse decision. `Pause` maps to a 429 at the route --
/// deliberately NOT a 401 (a 401 anywhere is the dashboard's
/// session-expired signal) and not a 403 (the device's token is valid;
/// this is a quota, and 429 is the status retrying clients back off on).
pub enum IngestFuse {
  Allow,
  Pause,
}

/// The body every HTTP ingest surface answers a paused account with. One
/// constant rather than a literal per route so a device sees the same
/// refusal whether it was reporting telemetry, confirming a shadow or
/// uploading logs -- the account state being described is the same one.
pub const INGEST_PAUSED_MESSAGE: &str =
  "Too Many Requests: free tier message allowance exhausted for this billing period";

/// Whether a device report should be refused because its account is a
/// free-tier one that has exhausted this month's message allowance. Paid,
/// entitled tiers always pass -- their over-allowance usage bills as
/// metered overage instead of stopping.
///
/// Fail-open on every error path (connection, missing tables, missing
/// mirror row): an infrastructure blip must never brick ingestion for the
/// fleet. The query's shape is constant so Hyperdrive's ~60s result cache
/// absorbs the per-report cost; the same caching means the pause can
/// engage up to a minute after the allowance is crossed, which is fine for
/// a monthly quota.
pub async fn check_perch_ingest_fuse(env: &Env, pigeon_id: &str) -> IngestFuse {
  let client = match get_db_client(env).await {
    Ok(client) => client,
    Err(e) => {
      console_error!("Perch fuse check skipped for '{pigeon_id}' (failing open): {e}");
      return IngestFuse::Allow;
    }
  };

  // The usage join only needs the calendar-month period: an account is
  // only pausable when it resolves to the free tier, and free-tier usage
  // is always month-anchored (see record_billable_message).
  let rows = match client
    .query_typed(
      "SELECT o.plan AS org_plan, o.subscription_status AS org_status,
              u.billable_messages
       FROM pigeons p
       JOIN flocks f ON f.id = p.flock_id
       LEFT JOIN organizations o ON o.id = f.org_id
       LEFT JOIN billing_usage_periods u
         ON u.owner_kind = CASE WHEN f.org_id IS NULL THEN 'user' ELSE 'org' END
        AND u.owner_id = COALESCE(f.org_id, f.user_id)
        AND u.period_start = date_trunc('month', now())
       WHERE p.id = $1;",
      &[(&pigeon_id, Type::TEXT)],
    )
    .await
  {
    Ok(rows) => rows,
    Err(e) => {
      console_error!("Perch fuse check failed for '{pigeon_id}' (failing open): {e}");
      return IngestFuse::Allow;
    }
  };

  // No Postgres mirror row: no account to meter against, so nothing to
  // pause either -- consistent with the tally's own undercount direction.
  let Some(row) = rows.first() else {
    return IngestFuse::Allow;
  };

  let org_plan: Option<String> = row.get("org_plan");
  let org_status: Option<String> = row.get("org_status");
  if effective_plan(org_plan.as_deref(), org_status.as_deref()) != BillingPlan::Perch {
    return IngestFuse::Allow;
  }

  let billable: Option<i64> = row.get("billable_messages");
  if billable.unwrap_or(0) >= BillingPlan::Perch.included_messages() {
    IngestFuse::Pause
  } else {
    IngestFuse::Allow
  }
}

/// The pigeon-creation entitlement decision. Refusal blocks growth only --
/// nothing about an existing device's ingestion changes here.
pub enum DeviceCap {
  Allow,
  Refuse { device_count: i64, cap: i64 },
}

/// Whether the account owning `flock_id` may add another device. Only an
/// account served at the free tier has a hard cap; a paid, entitled tier
/// past its included device count is allowed through and billed per-device
/// overage instead (the reporter's job). Status gates before plan, as
/// everywhere.
///
/// Fail-open on lookup errors (logged): an infrastructure blip must not
/// block provisioning, and the count is re-derived on the next attempt
/// anyway.
///
/// Counts provisioned rows, deliberately unlike `report_device_overage`,
/// which counts only devices that reported in the period. The two answer
/// different questions and the difference is not drift: the free tier's
/// cap is on how many pigeons may exist (each one occupies a Durable
/// Object whether or not it ever powers on), while the meter charges only
/// for devices in use.
pub async fn check_device_cap(client: &Client, flock_id: &Uuid) -> DeviceCap {
  let rows = match client
    .query_typed(
      "SELECT o.plan AS org_plan, o.subscription_status AS org_status,
         (SELECT COUNT(*)::bigint
            FROM pigeons p JOIN flocks f2 ON f2.id = p.flock_id
            WHERE (f.org_id IS NOT NULL AND f2.org_id = f.org_id)
               OR (f.org_id IS NULL AND f2.org_id IS NULL
                   AND f2.user_id = f.user_id)) AS device_count
       FROM flocks f
       LEFT JOIN organizations o ON o.id = f.org_id
       WHERE f.id = $1;",
      &[(flock_id, Type::UUID)],
    )
    .await
  {
    Ok(rows) => rows,
    Err(e) => {
      console_error!("Device cap check failed for flock {flock_id} (failing open): {e}");
      return DeviceCap::Allow;
    }
  };

  // Unknown flock: the route's own authorization already 403s that case;
  // nothing for the cap to add.
  let Some(row) = rows.first() else {
    return DeviceCap::Allow;
  };

  let org_plan: Option<String> = row.get("org_plan");
  let org_status: Option<String> = row.get("org_status");
  let plan = effective_plan(org_plan.as_deref(), org_status.as_deref());
  if plan != BillingPlan::Perch {
    return DeviceCap::Allow;
  }

  let device_count: i64 = row.get("device_count");
  let cap = BillingPlan::Perch.included_devices();
  if device_count >= cap {
    DeviceCap::Refuse { device_count, cap }
  } else {
    DeviceCap::Allow
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
  allowance_floor_messages: Option<i64>,
  /// Devices that sent at least one billable message during this period.
  /// Both meters read this one figure: the per-device overage charges on
  /// it, and the message allowance is widened by it, so the two can never
  /// disagree about how many extra devices an account is being billed for.
  connected_device_count: i64,
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
///
/// The connected-device count comes from here rather than from the meter
/// that reports it, because both meters need it: a pigeon is connected for
/// a period when it sent at least one billable message during it, which is
/// the promise the pricing page makes and the definition it publishes.
/// `last_billable_activity` is NULL for a device that has never reported,
/// and NULL fails both comparisons, so provisioned-and-idle stock costs
/// nothing and adds nothing. The period bounds are the same ones the stamp
/// writer anchors on -- the WHERE clause below selects on the identical
/// live-subscription condition as `record_billable_message`'s
/// `use_org_period` -- so a device active anywhere in the period is
/// stamped inside it, not merely within six hours of it.
async fn load_billable_orgs(client: &Client) -> Result<Vec<BillableOrg>> {
  let rows = client
    .query_typed(
      "SELECT o.id, o.stripe_customer_id, o.plan, o.subscription_status,
              o.current_period_start, o.current_period_end,
              COALESCE(u.billable_messages, 0) AS billable_messages,
              u.allowance_floor_messages,
              (SELECT COUNT(*)::bigint
                 FROM pigeons p JOIN flocks f ON f.id = p.flock_id
                 WHERE f.org_id = o.id
                   AND p.last_billable_activity >= o.current_period_start
                   AND p.last_billable_activity < o.current_period_end
              ) AS connected_device_count
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
        allowance_floor_messages: row.get("allowance_floor_messages"),
        connected_device_count: row.get("connected_device_count"),
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
  let allowance = period_message_allowance(
    plan,
    org.allowance_floor_messages,
    org.connected_device_count,
  );
  let overage = (org.billable_messages - allowance).max(0);

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
/// extra-devices count (connected devices above the tier's included count)
/// -- once per period, identifier keyed on org + period. A device removed
/// or added after this snapshot isn't re-reported; a missed final-day run
/// undercounts and logs, never bills late into the next period.
///
/// The count itself is `load_billable_orgs`'s, read at the top of this
/// same reporter pass -- see its doc comment for what "connected" means
/// and why the period bounds are the ones they are. Sharing it is what
/// makes this meter and the message allowance agree by construction: an
/// account cannot be billed for an extra device whose 30 K of pool it was
/// not also granted. What the once-per-period snapshot does miss is a
/// device whose only activity lands in the period's final hours, after
/// this run; that undercounts, in the customer's favour.
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

  let extra = plan.billed_extra_devices(org.connected_device_count);
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
  use super::{effective_plan, period_message_allowance};
  use capsules::BillingPlan;

  #[test]
  fn period_allowance_is_the_higher_of_tier_and_floor() {
    // Mid-period downgrade scale -> growth: the floor recorded at change
    // time (scale's allowance) keeps governing the in-flight period, so
    // usage that was included when it happened can't become overage.
    assert_eq!(
      period_message_allowance(BillingPlan::Growth, Some(45_000_000), 0),
      45_000_000
    );
    // Mid-period upgrade growth -> scale: the floor holds the old, lower
    // allowance and must not cap the new tier's own.
    assert_eq!(
      period_message_allowance(BillingPlan::Scale, Some(7_500_000), 0),
      45_000_000
    );
    // No change this period: the tier's own allowance, floor or not.
    assert_eq!(
      period_message_allowance(BillingPlan::Growth, None, 0),
      7_500_000
    );
  }

  #[test]
  fn billed_extra_devices_extend_the_pool() {
    // The case this exists for: a Builder account at 150 connected
    // devices used to bill device overage and message overage at once for
    // one act of growth. 100 extra devices now carry 3 M of pool with
    // them, so the second meter stays quiet until the fleet genuinely
    // talks more than the devices were sold to.
    assert_eq!(
      period_message_allowance(BillingPlan::Builder, None, 150),
      4_500_000
    );
    // Under and at the included count, nothing is billed and nothing is
    // added.
    assert_eq!(
      period_message_allowance(BillingPlan::Builder, None, 50),
      1_500_000
    );
    assert_eq!(
      period_message_allowance(BillingPlan::Builder, None, 7),
      1_500_000
    );
  }

  #[test]
  fn the_extra_pool_stacks_on_the_floor_rather_than_competing_with_it() {
    // A downgrade mid-period is exactly when both halves are live at once:
    // scale -> growth leaves the floor at scale's 45 M, while the same
    // 1,600-device fleet now bills 1,350 extras against growth's 250
    // included. The extras add to the floor -- taking the larger of the
    // two would silently drop whichever half moved second.
    assert_eq!(
      period_message_allowance(BillingPlan::Growth, Some(45_000_000), 1_600),
      45_000_000 + 1_350 * 30_000
    );
  }

  #[test]
  fn the_free_tier_allowance_is_untouched_by_any_device_count() {
    // The guard the ingest fuse depends on. The fuse trips at
    // `BillingPlan::Perch.included_messages()`, so if a device count could
    // ever widen the free tier's allowance the two would part company and
    // the fuse would be enforcing a number nothing else believes.
    for connected in [0, 10, 11, 10_000] {
      assert_eq!(
        period_message_allowance(BillingPlan::Perch, None, connected),
        BillingPlan::Perch.included_messages()
      );
    }
    // And an unentitled org resolves to Perch before it ever gets here, so
    // a lapsed Fleet account with 10,000 devices connected is on the free
    // tier's allowance too, not a 300 M one.
    let plan = effective_plan(Some("fleet"), Some("canceled"));
    assert_eq!(
      period_message_allowance(plan, None, 10_000),
      BillingPlan::Perch.included_messages()
    );
  }

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
