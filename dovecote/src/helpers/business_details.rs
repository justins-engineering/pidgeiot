//! Business identity + tax registration on an organization -- the Postgres
//! side, the VIES-backed save path, and the sweep that finishes a lookup we
//! could not finish at save time.
//!
//! Why these columns hang off `organizations` and not off the Kratos
//! identity is argued in `infra/migrations/2026-08-24-org-business-details.sql`
//! and in `docs/api.md`. In one line: the org is the billing entity, and
//! tax identity belongs to whoever the invoice is made out to.

use capsules::{
  MAX_BUSINESS_NAME_CHARS, OrganizationBusinessDetails, OrganizationBusinessDetailsRequest,
  TaxIdDecision, TaxIdStatus, TaxIdType, decide_status, parse_eu_vat, prepare_tax_id,
  recheck_status, tax_id_log_label,
};
use tokio_postgres::types::Type;
use tokio_postgres::{Client, Row};
use uuid::Uuid;
use worker::{Env, Error, Result, console_error, console_log};

use crate::helpers::get_db_client;
use crate::helpers::vies::check_vat;

/// How long a pending lookup rests before the sweep asks VIES again.
///
/// The sweep rides the 5-minute cron, so without this every pending org
/// would be re-queried twelve times an hour against a free public service
/// that is under no obligation to serve us. An hour is far below the
/// timescale on which a member state's outage resolves and far above the
/// rate at which anyone could reasonably call it polite.
const RECHECK_INTERVAL: &str = "1 hour";

/// Pending orgs re-checked per sweep. Bounds both the cron invocation's
/// wall time (the lookups run one at a time, ~0.3-0.5s each) and our
/// footprint on VIES. At 12 sweeps an hour this clears 240 pending
/// registrations an hour, which is orders of magnitude more than a backlog
/// can plausibly reach.
const RECHECK_BATCH_LIMIT: i64 = 20;

/// Runtime schema bootstrap, same lazy-DDL convention as
/// `ensure_org_tables`/`ensure_billing_tables`: the dated migration is the
/// documented schema, and this keeps a Worker deployed ahead of an applied
/// migration from failing every org read.
pub async fn ensure_business_details_columns(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "ALTER TABLE organizations ADD COLUMN IF NOT EXISTS business_name TEXT;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id TEXT;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_type TEXT NOT NULL DEFAULT 'none';
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_status TEXT NOT NULL DEFAULT 'none';
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_validated_at TIMESTAMPTZ;
      ALTER TABLE organizations ADD COLUMN IF NOT EXISTS tax_id_checked_at TIMESTAMPTZ;
      CREATE INDEX IF NOT EXISTS idx_organizations_tax_id_pending
        ON organizations(tax_id_checked_at) WHERE tax_id_status = 'pending';",
    )
    .await
    .map_err(|e| {
      console_error!("Business details bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

/// Both enum columns parse permissively, matching the convention on
/// `OrgRole` and `SubscriptionStatus`: a value we cannot read came from a
/// hand-edited row, and the fallbacks are chosen so that the failure
/// under-claims rather than over-claims. An unreadable type reads as
/// `Other` ("held, not checked") rather than `None`, which would deny a
/// stored identifier exists; an unreadable status reads as `Pending`
/// ("we owe an answer"), which the sweep then resolves on its own, rather
/// than as `Validated`, which we could not support.
fn row_to_details(row: &Row) -> OrganizationBusinessDetails {
  let tax_id_type: TaxIdType = row
    .get::<_, String>("tax_id_type")
    .parse()
    .unwrap_or(TaxIdType::Other);
  let tax_id_status: TaxIdStatus = row
    .get::<_, String>("tax_id_status")
    .parse()
    .unwrap_or(TaxIdStatus::Pending);

  OrganizationBusinessDetails {
    org_id: row.get("id"),
    business_name: row.get("business_name"),
    tax_id: row.get("tax_id"),
    tax_id_type,
    tax_id_status,
    tax_id_validated_at: row.get("tax_id_validated_at"),
    tax_id_checked_at: row.get("tax_id_checked_at"),
  }
}

const DETAILS_COLUMNS: &str = "id, business_name, tax_id, tax_id_type, tax_id_status,
   tax_id_validated_at, tax_id_checked_at";

pub async fn load_business_details(
  client: &Client,
  org_id: &Uuid,
) -> Result<Option<OrganizationBusinessDetails>> {
  let rows = client
    .query_typed(
      &format!("SELECT {DETAILS_COLUMNS} FROM organizations WHERE id = $1"),
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Business details read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(row_to_details))
}

/// A validated, looked-up submission, ready to write. Held separately from
/// the write so `POST /orgs` can settle the registration BEFORE it inserts
/// an org -- a definitive invalid should refuse the whole creation, not
/// leave an org behind carrying a number we already know is wrong.
pub struct BusinessDetailsPlan {
  business_name: Option<String>,
  stored: Option<String>,
  kind: TaxIdType,
  status: TaxIdStatus,
  attempted_lookup: bool,
}

impl BusinessDetailsPlan {
  /// Nothing was submitted, so there is nothing to write. Lets org
  /// creation skip the update entirely for the common case of somebody who
  /// filled in only a name.
  pub fn is_empty(&self) -> bool {
    self.business_name.is_none() && self.stored.is_none() && self.kind == TaxIdType::None
  }

  /// How this submission should be named in a log line -- kind, country
  /// prefix and length, never the identifier.
  fn log_label(&self) -> String {
    self
      .stored
      .as_deref()
      .map(|stored| tax_id_log_label(self.kind, stored))
      .unwrap_or_else(|| "none".to_string())
  }
}

/// Validates a submission and settles what we can learn about it, touching
/// no database.
///
/// The order matters and is the feature's whole contract:
///
/// 1. **Shape first, locally.** A typo is refused without spending a VIES
///    call, and the customer gets a specific reason instead of "invalid".
/// 2. **Ask VIES, for EU VAT only.** Nothing else has an authority.
/// 3. **Refuse ONLY on a definitive `invalid`.** Every other non-answer --
///    the service down, the member state down, a timeout, a body we could
///    not read -- keeps the identifier and marks it `pending` for the
///    sweep. A customer must never be unable to record their own VAT
///    number because a government service is having a bad afternoon.
///
/// `Err` is the user-facing refusal, already prefixed for the route to
/// return verbatim; it never carries a fragment of the submitted
/// identifier.
pub async fn plan_business_details(
  request: &OrganizationBusinessDetailsRequest,
) -> std::result::Result<BusinessDetailsPlan, String> {
  let business_name = request
    .business_name
    .as_deref()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(str::to_string);

  if business_name
    .as_deref()
    .is_some_and(|name| name.chars().count() > MAX_BUSINESS_NAME_CHARS)
  {
    return Err(format!(
      "Bad Request: business name is longer than {MAX_BUSINESS_NAME_CHARS} characters"
    ));
  }

  let prepared = prepare_tax_id(request.tax_id_type, request.tax_id.as_deref())
    .map_err(|e| format!("Bad Request: {e}"))?;

  let lookup = match &prepared.lookup {
    Some(vat) => Some(check_vat(vat).await),
    None => None,
  };

  let status = match decide_status(request.tax_id_type, lookup) {
    TaxIdDecision::Refuse => {
      return Err(
        "Bad Request: VIES does not recognize that VAT ID as a live registration".to_string(),
      );
    }
    TaxIdDecision::Store(status) => status,
  };

  Ok(BusinessDetailsPlan {
    business_name,
    stored: prepared.stored,
    kind: request.tax_id_type,
    status,
    attempted_lookup: lookup.is_some(),
  })
}

/// Writes a settled submission onto an existing org. `None` means no such
/// org.
pub async fn write_business_details(
  client: &Client,
  org_id: &Uuid,
  plan: &BusinessDetailsPlan,
) -> Result<Option<OrganizationBusinessDetails>> {
  if plan.status == TaxIdStatus::Pending {
    console_log!(
      "VAT id stored pending a VIES answer on org {org_id} ({})",
      plan.log_label()
    );
  }

  let business_name = &plan.business_name;
  let stored = &plan.stored;
  let attempted_lookup = plan.attempted_lookup;

  // Both CASE expressions read the row's PRE-update `tax_id`/`tax_id_status`
  // -- every SET expression in one UPDATE is evaluated against the old row --
  // which is what lets them ask "is this the same number as before?".
  //
  // An inconclusive re-save must not downgrade a registration we already
  // confirmed. Somebody editing their business name during a VIES outage
  // did nothing to cast doubt on a number VIES already accepted, and
  // flipping them to `pending` would read as a problem they caused. The
  // number has to be unchanged for this to apply; a new number carries none
  // of the old one's history.
  //
  // Same reasoning for `tax_id_validated_at`: it is a confirmation date,
  // not a save date, so "confirmed on the 3rd, unreachable since" survives.
  let rows = client
    .query_typed(
      &format!(
        "UPDATE organizations SET
           business_name = $2,
           tax_id = $3,
           tax_id_type = $4,
           tax_id_status = CASE
             WHEN $5 = 'pending'
               AND tax_id_status = 'validated'
               AND tax_id IS NOT DISTINCT FROM $3
               THEN 'validated'
             ELSE $5
           END,
           tax_id_validated_at = CASE
             WHEN $5 = 'validated' THEN now()
             WHEN tax_id IS NOT DISTINCT FROM $3 THEN tax_id_validated_at
             ELSE NULL
           END,
           tax_id_checked_at = CASE WHEN $6 THEN now() ELSE NULL END,
           updated_at = now()
         WHERE id = $1
         RETURNING {DETAILS_COLUMNS}"
      ),
      &[
        (org_id, Type::UUID),
        (business_name, Type::TEXT),
        (stored, Type::TEXT),
        (&plan.kind.as_str(), Type::TEXT),
        (&plan.status.as_str(), Type::TEXT),
        (&attempted_lookup, Type::BOOL),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Business details write error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(row_to_details))
}

/// Finishes the lookups that could not finish at save time.
///
/// Rides the existing 5-minute cron rather than a trigger of its own, for
/// the same account-wide 5-cron-trigger limit documented on
/// `probe_kratos_health` in `scheduled.rs`. Best-effort/logged throughout,
/// like every other passenger on that invocation.
///
/// Only `pending` + `eu_vat` rows are eligible: a non-EU identifier has no
/// authority to ask, and a settled row has nothing left to learn. Rows are
/// taken oldest-attempt-first so a backlog drains in order rather than
/// starving whichever org sorts last.
///
/// Deliberately NOT a revalidation cadence: a `validated` row is never
/// re-checked here, so a registration deregistered after we confirmed it
/// keeps reading as validated until its owner next saves the form. Adding a
/// periodic re-check of confirmed registrations is a reasonable future
/// change; it is a different feature (it needs a cadence, and a decision
/// about what a customer sees when their own registration lapses) and not
/// something to do by widening this predicate.
pub async fn sweep_pending_tax_ids(env: &Env) -> Result<()> {
  let client = get_db_client(env).await?;
  // The sweep's predicate reads columns a database behind the migration
  // would not have, and an error here fails the whole sweep rather than
  // failing open -- so bootstrap first, exactly as the retention sweep
  // does with the billing columns.
  ensure_business_details_columns(&client).await?;

  let rows = client
    .query_typed(
      &format!(
        "SELECT id, tax_id FROM organizations
         WHERE tax_id_status = 'pending'
           AND tax_id_type = 'eu_vat'
           AND tax_id IS NOT NULL
           AND (tax_id_checked_at IS NULL
                OR tax_id_checked_at < now() - interval '{RECHECK_INTERVAL}')
         ORDER BY tax_id_checked_at ASC NULLS FIRST
         LIMIT $1"
      ),
      &[(&RECHECK_BATCH_LIMIT, Type::INT8)],
    )
    .await
    .map_err(|e| {
      console_error!("Pending VAT sweep read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  if rows.is_empty() {
    return Ok(());
  }
  console_log!("Re-checking {} pending VAT registration(s)", rows.len());

  for row in &rows {
    let org_id: Uuid = row.get("id");
    let stored: String = row.get("tax_id");

    // Stored values passed `prepare_tax_id` when they were written, so a
    // parse failure here means the row was edited by hand. Stamping the
    // attempt anyway keeps one bad row from being re-read every sweep
    // forever, and leaves it visible as a row that stays pending.
    let Ok(vat) = parse_eu_vat(&stored) else {
      console_error!("Pending VAT sweep: org {org_id} holds an unparseable tax_id, skipping");
      stamp_checked(&client, &org_id).await;
      continue;
    };

    let status = recheck_status(check_vat(&vat).await);
    console_log!(
      "Pending VAT re-check for org {org_id} ({}): {status}",
      tax_id_log_label(TaxIdType::EuVat, &stored)
    );

    // Guarded on the number AND on the row still being pending: a customer
    // who edited their details while the sweep was mid-flight must not have
    // the sweep's answer about the OLD number written over the new one.
    if let Err(e) = client
      .query_typed(
        "UPDATE organizations SET
           tax_id_status = $2,
           tax_id_validated_at = CASE WHEN $2 = 'validated' THEN now() ELSE tax_id_validated_at END,
           tax_id_checked_at = now()
         WHERE id = $1 AND tax_id = $3 AND tax_id_status = 'pending'",
        &[
          (&org_id, Type::UUID),
          (&status.as_str(), Type::TEXT),
          (&stored, Type::TEXT),
        ],
      )
      .await
    {
      console_error!("Pending VAT sweep write error for org {org_id}: {e}");
    }
  }

  Ok(())
}

/// Records that we tried, without claiming anything about the outcome.
async fn stamp_checked(client: &Client, org_id: &Uuid) {
  if let Err(e) = client
    .query_typed(
      "UPDATE organizations SET tax_id_checked_at = now() WHERE id = $1",
      &[(org_id, Type::UUID)],
    )
    .await
  {
    console_error!("Pending VAT sweep could not stamp org {org_id}: {e}");
  }
}
