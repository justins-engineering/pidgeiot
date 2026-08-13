use capsules::OrganizationBilling;
use time::OffsetDateTime;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use worker::{Error, Result, console_error};

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

/// Writes a subscription's state onto the org that owns it, matching on
/// the subscription id, or on the customer id for the first subscription a
/// customer has ever had.
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
  let plan = billing.plan.map(|plan| plan.as_str());

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
              OR (stripe_subscription_id IS NULL AND stripe_customer_id = $1))
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
