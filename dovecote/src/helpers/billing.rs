use capsules::{
  BillingPlan, OrganizationBilling, OrganizationBillingOverview, OrganizationBusinessDetails,
  SubscriptionStatus,
};
use time::OffsetDateTime;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Error, Result, console_error};

use crate::helpers::business_details::{
  DETAILS_COLUMNS, ensure_business_details_columns, row_to_details,
};

/// Runtime schema bootstrap, same lazy-DDL convention as
/// `ensure_org_tables` (`helpers/orgs.rs`): `infra/init-db.sql` and the
/// dated migration remain the documented schema, and this keeps a Worker
/// deployed ahead of an applied migration from failing every webhook.
pub async fn ensure_billing_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS stripe_subscription_id TEXT;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'perch';
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS subscription_status TEXT NOT NULL DEFAULT 'none';
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_start TIMESTAMPTZ;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS current_period_end TIMESTAMPTZ;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS cancel_at_period_end BOOLEAN NOT NULL DEFAULT false;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS billing_event_at TIMESTAMPTZ;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS comp_plan TEXT;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS comp_note TEXT;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS comp_granted_at TIMESTAMPTZ;
      CREATE TABLE IF NOT EXISTS stripe_webhook_events (
        event_id TEXT PRIMARY KEY,
        event_type TEXT NOT NULL,
        event_created TIMESTAMPTZ NOT NULL,
        livemode BOOLEAN NOT NULL DEFAULT false,
        api_version TEXT,
        received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        processed_at TIMESTAMPTZ,
        redelivery_count INTEGER NOT NULL DEFAULT 0
      );
      CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_customer
        ON organizations(stripe_customer_id) WHERE stripe_customer_id IS NOT NULL;
      CREATE UNIQUE INDEX IF NOT EXISTS idx_organizations_stripe_subscription
        ON organizations(stripe_subscription_id) WHERE stripe_subscription_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS idx_stripe_webhook_events_unprocessed
        ON stripe_webhook_events(received_at) WHERE processed_at IS NULL;",
    )
    .await
    .map_err(|e| {
      console_error!("Billing tables bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

/// What the idempotency claim found. Stripe retries a webhook for up to
/// three days and can deliver the same event more than once even on
/// success, so every delivery is claimed before anything is applied.
#[derive(Debug, PartialEq)]
pub enum WebhookClaim {
  /// First delivery, or a redelivery of something that never finished
  /// applying -- proceed.
  Unprocessed,
  /// A previous delivery already applied this event -- ack and do nothing.
  AlreadyProcessed,
}

/// Records this delivery and reports whether the event has already been
/// applied. The insert and the check are one statement so two concurrent
/// deliveries cannot both see "not yet recorded": the loser takes the
/// `DO UPDATE` branch and reads the winner's row.
///
/// Claiming does NOT mark the event processed -- that happens only after
/// the apply succeeds (`mark_webhook_event_processed`), so a delivery that
/// dies mid-apply is retried by Stripe rather than being silently swallowed
/// by its own claim.
pub async fn claim_webhook_event(
  client: &Client,
  event_id: &str,
  event_type: &str,
  event_created: OffsetDateTime,
  livemode: bool,
  api_version: Option<&str>,
) -> Result<WebhookClaim> {
  let rows = client
    .query_typed(
      "INSERT INTO stripe_webhook_events
         (event_id, event_type, event_created, livemode, api_version)
       VALUES ($1, $2, $3, $4, $5)
       ON CONFLICT (event_id) DO UPDATE
         SET redelivery_count = stripe_webhook_events.redelivery_count + 1
       RETURNING processed_at;",
      &[
        (&event_id, Type::TEXT),
        (&event_type, Type::TEXT),
        (&event_created, Type::TIMESTAMPTZ),
        (&livemode, Type::BOOL),
        (&api_version, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Stripe webhook claim error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let processed_at: Option<OffsetDateTime> = rows
    .first()
    .and_then(|row| row.get::<_, Option<OffsetDateTime>>("processed_at"));

  Ok(match processed_at {
    Some(_) => WebhookClaim::AlreadyProcessed,
    None => WebhookClaim::Unprocessed,
  })
}

pub async fn mark_webhook_event_processed(client: &Client, event_id: &str) -> Result<()> {
  client
    .query_typed(
      "UPDATE stripe_webhook_events SET processed_at = now() WHERE event_id = $1;",
      &[(&event_id, Type::TEXT)],
    )
    .await
    .map(|_| ())
    .map_err(|e| {
      console_error!("Stripe webhook processed-mark error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

/// Binds a Stripe customer to an org, keeping any id already there --
/// COALESCE so a lost race (or a replayed webhook) can never re-point an
/// org at a second customer. Returns the id that actually won.
pub async fn attach_stripe_customer(
  client: &Client,
  org_id: &Uuid,
  customer_id: &str,
) -> Result<Option<String>> {
  let rows = client
    .query_typed(
      "UPDATE organizations
       SET stripe_customer_id = COALESCE(stripe_customer_id, $2), updated_at = now()
       WHERE id = $1
       RETURNING stripe_customer_id;",
      &[(org_id, Type::UUID), (&customer_id, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Org Stripe customer attach error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(rows.first().and_then(|row| row.get("stripe_customer_id")))
}

/// Everything a billing mutation needs to know about the org before it
/// touches Stripe: the customer it bills (checkout creates one when this
/// is empty, the portal refuses without one), the stored tier and status
/// (the same-tier and no-live-subscription refusals), the subscription id
/// a plan change addresses, the usage-period bounds the message-allowance
/// floor is keyed on -- anchored exactly the way the usage tally anchors,
/// so the floor lands on the row the meter reporter actually reads -- and
/// the tax identity checkout forwards.
///
/// One statement on purpose. It is anchored on `now()`, and Hyperdrive
/// never caches a statement that carries a volatile or stable function
/// (see CLAUDE.md's Hyperdrive note), so a customer id attached by the
/// checkout that just returned, or a VAT number saved seconds ago, is
/// what the next billing route reads. Separate plain lookups for the
/// customer id and the tax identity used to sit behind the cache and
/// answered "no billing account yet" for a minute after a first checkout.
pub struct OrgBillingState {
  pub name: String,
  pub stripe_customer_id: Option<String>,
  pub plan: BillingPlan,
  pub status: SubscriptionStatus,
  pub stripe_subscription_id: Option<String>,
  pub usage_period_start: OffsetDateTime,
  pub usage_period_end: OffsetDateTime,
  pub business_details: OrganizationBusinessDetails,
}

/// `None` if the org doesn't exist. Bootstraps the billing and
/// business-detail columns it reads, so a caller needs no ensure of its
/// own before this.
pub async fn load_org_billing_state(
  client: &Client,
  org_id: &Uuid,
) -> Result<Option<OrgBillingState>> {
  ensure_billing_tables(client).await?;
  ensure_business_details_columns(client).await?;

  let rows = client
    .query_typed(
      &format!(
        "SELECT o.name, o.stripe_customer_id, o.plan, o.subscription_status,
           o.stripe_subscription_id, {DETAILS_COLUMNS},
           CASE WHEN use_org_period THEN o.current_period_start
                ELSE date_trunc('month', now()) END AS usage_period_start,
           CASE WHEN use_org_period THEN o.current_period_end
                ELSE date_trunc('month', now()) + interval '1 month' END AS usage_period_end
         FROM organizations o,
           LATERAL (SELECT (o.subscription_status IN ('trialing', 'active', 'past_due')
               AND o.current_period_start IS NOT NULL
               AND o.current_period_end IS NOT NULL
               AND now() >= o.current_period_start
               AND now() < o.current_period_end) AS use_org_period) anchor
         WHERE o.id = $1;"
      ),
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org billing state lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(row) = rows.first() else {
    return Ok(None);
  };

  let plan_raw: String = row.get("plan");
  let status_raw: String = row.get("subscription_status");

  Ok(Some(OrgBillingState {
    name: row.get("name"),
    stripe_customer_id: row.get("stripe_customer_id"),
    plan: plan_raw.parse().unwrap_or_default(),
    // Same conservative fallback as the overview: unknown status reads as
    // unentitled-but-subscribed, never as "never subscribed".
    status: status_raw.parse().unwrap_or(SubscriptionStatus::Incomplete),
    stripe_subscription_id: row.get("stripe_subscription_id"),
    usage_period_start: row.get("usage_period_start"),
    usage_period_end: row.get("usage_period_end"),
    business_details: row_to_details(row),
  }))
}

/// One read of everything the dashboard's billing panel shows: the org's
/// billing columns, the tallied usage for the period those columns anchor
/// (Stripe period while a live subscription covers now, calendar month
/// otherwise -- the same anchoring the tally itself uses), and the org's
/// current device count. `None` if the org doesn't exist.
pub async fn load_org_billing_overview(
  client: &Client,
  org_id: &Uuid,
) -> Result<Option<OrganizationBillingOverview>> {
  let rows = client
    .query_typed(
      "WITH anchored AS (
         SELECT o.*,
           (o.subscription_status IN ('trialing', 'active', 'past_due')
             AND o.current_period_start IS NOT NULL
             AND o.current_period_end IS NOT NULL
             AND now() >= o.current_period_start
             AND now() < o.current_period_end) AS use_org_period
         FROM organizations o WHERE o.id = $1
       )
       SELECT a.plan, a.subscription_status, a.stripe_customer_id, a.cancel_at_period_end,
         a.comp_plan, a.comp_note, a.comp_granted_at,
         CASE WHEN a.use_org_period THEN a.current_period_start
              ELSE date_trunc('month', now()) END AS usage_period_start,
         CASE WHEN a.use_org_period THEN a.current_period_end
              ELSE date_trunc('month', now()) + interval '1 month' END AS usage_period_end,
         COALESCE(u.billable_messages, 0) AS billable_messages,
         u.allowance_floor_messages,
         (SELECT COUNT(*)::bigint FROM pigeons p
            JOIN flocks f ON f.id = p.flock_id
            WHERE f.org_id = a.id) AS device_count,
         (SELECT COUNT(*)::bigint FROM pigeons p
            JOIN flocks f ON f.id = p.flock_id
            WHERE f.org_id = a.id
              AND p.last_billable_activity >=
                CASE WHEN a.use_org_period THEN a.current_period_start
                     ELSE date_trunc('month', now()) END
              AND p.last_billable_activity <
                CASE WHEN a.use_org_period THEN a.current_period_end
                     ELSE date_trunc('month', now()) + interval '1 month' END
           ) AS connected_device_count
       FROM anchored a
       LEFT JOIN billing_usage_periods u
         ON u.owner_kind = 'org' AND u.owner_id = a.id
        AND u.period_start = CASE WHEN a.use_org_period THEN a.current_period_start
                                  ELSE date_trunc('month', now()) END;",
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org billing overview error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(row) = rows.first() else {
    return Ok(None);
  };

  let plan_raw: String = row.get("plan");
  let status_raw: String = row.get("subscription_status");
  let customer_id: Option<String> = row.get("stripe_customer_id");

  let plan: BillingPlan = plan_raw.parse().unwrap_or_default();
  // Same conservative fallback the webhook conversion applies: an
  // unknown status string reads as unentitled-but-subscribed, never as
  // "never subscribed".
  let status: SubscriptionStatus = status_raw.parse().unwrap_or(SubscriptionStatus::Incomplete);
  let comp_plan_raw: Option<String> = row.get("comp_plan");
  let served =
    super::usage::served_plan(Some(&plan_raw), Some(&status_raw), comp_plan_raw.as_deref());
  let effective_plan = served.plan;
  // Only a grant that is actually carrying the tier is reported as one:
  // an inert grant behind a live subscription would otherwise make a
  // paying customer's dashboard call itself complimentary.
  let comp_plan = match served.source {
    super::usage::PlanSource::Comp => Some(served.plan),
    _ => None,
  };

  // The same allowance the meter charges against, not the bare tier
  // figure: the dashboard's usage bar is where a customer checks whether
  // they are about to pay overage, so showing them a denominator the
  // reporter doesn't use would be worse than showing nothing. Both halves
  // matter -- a mid-period downgrade's floor, and the pool the account's
  // billed extra devices carry.
  let connected_device_count: i64 = row.get("connected_device_count");
  let allowance_floor_messages: Option<i64> = row.get("allowance_floor_messages");
  let included_messages =
    served.period_message_allowance(allowance_floor_messages, connected_device_count);

  Ok(Some(OrganizationBillingOverview {
    plan,
    status,
    entitled: status.is_entitled(),
    effective_plan,
    comp_plan,
    cancel_at_period_end: row.get("cancel_at_period_end"),
    has_billing_account: customer_id.is_some(),
    usage_period_start: row.get("usage_period_start"),
    usage_period_end: row.get("usage_period_end"),
    billable_messages: row.get("billable_messages"),
    included_messages,
    device_count: row.get("device_count"),
    connected_device_count,
    included_devices: effective_plan.included_devices(),
  }))
}

/// Writes a subscription's state onto the org that owns it, matching on
/// the subscription id, or on the customer id when the org has no live
/// subscription to defend: none yet, or one that has ended. A customer who
/// cancelled and later buys again gets a brand-new subscription id from
/// Checkout, and the org still names the dead one; without the second
/// clause that purchase would never bind and the org would stay on the
/// free tier while Stripe charged for it. A live subscription's row is
/// never re-pointed by a different subscription's event.
///
/// Two guards, both because Stripe delivers events unordered:
/// `billing_event_at` refuses an event older than the one already applied,
/// and a `NULL` plan (the subscription named no tier) leaves the stored
/// plan alone rather than guessing.
///
/// Returns whether a row matched. `false` means no org claims this Stripe
/// customer -- worth logging, since it implies a customer created outside
/// our own provisioning path.
pub async fn apply_subscription(
  client: &Client,
  billing: &OrganizationBilling,
  event_created: OffsetDateTime,
) -> Result<bool> {
  let Some(customer_id) = billing.stripe_customer_id.as_deref() else {
    return Ok(false);
  };
  let Some(subscription_id) = billing.stripe_subscription_id.as_deref() else {
    return Ok(false);
  };

  let status = billing.status.as_str();

  // A cancelled subscription stops conferring its tier. Stripe's final
  // snapshot still lists the items it was billing, so the parsed plan is
  // the paid one right up to the end -- carrying that through would leave
  // a paid tier sitting beside a dead subscription, and anything that
  // consults `plan` without first checking `subscription_status` would
  // read it as entitlement. Cancellation is terminal, so the tier drops to
  // free here rather than being left for every reader to compensate for.
  // The recoverable non-entitled states are deliberately not included: a
  // past-due or unpaid subscription can still come back, and forgetting
  // which tier it was on would be worse than remembering.
  let plan = if billing.status == SubscriptionStatus::Canceled {
    Some(BillingPlan::Perch.as_str())
  } else {
    billing.plan.map(|plan| plan.as_str())
  };

  let rows = client
    .query_typed(
      "UPDATE organizations
       SET stripe_customer_id = $1,
           stripe_subscription_id = $2,
           subscription_status = $3,
           plan = COALESCE($4, plan),
           current_period_start = $5,
           current_period_end = $6,
           cancel_at_period_end = $7,
           billing_event_at = $8,
           updated_at = now()
       WHERE (stripe_subscription_id = $2
              OR (stripe_customer_id = $1
                  AND (stripe_subscription_id IS NULL
                       OR subscription_status IN ('canceled', 'incomplete_expired', 'unpaid'))))
         AND (billing_event_at IS NULL OR billing_event_at <= $8)
       RETURNING id;",
      &[
        (&customer_id, Type::TEXT),
        (&subscription_id, Type::TEXT),
        (&status, Type::TEXT),
        (&plan, Type::TEXT),
        (&billing.current_period_start, Type::TIMESTAMPTZ),
        (&billing.current_period_end, Type::TIMESTAMPTZ),
        (&billing.cancel_at_period_end, Type::BOOL),
        (&event_created, Type::TIMESTAMPTZ),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Billing subscription apply error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(!rows.is_empty())
}
