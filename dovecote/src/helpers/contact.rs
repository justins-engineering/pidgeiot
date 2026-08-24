use std::sync::atomic::{AtomicBool, Ordering};

use capsules::ContactRequest;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Env, Error, Result, console_error, console_log};

use super::alerts::send_via_usesend;
use super::ops_probe::ops_alert_email;

/// Ensured-once-per-isolate flag: `POST /contact` is unauthenticated and
/// abuse-facing, so it must not pay (or let an attacker multiply) a DDL
/// round trip per request.
static TABLE_READY: AtomicBool = AtomicBool::new(false);

/// Idempotently ensures `contact_submissions` exists -- same convention as
/// `ensure_error_tables`: the migration file under `infra/migrations/` is
/// the deploy-time apply, this is the belt-and-suspenders that lets the
/// route work against a database nobody remembered to migrate.
pub async fn ensure_contact_table(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS contact_submissions (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        name TEXT NOT NULL,
        email TEXT NOT NULL,
        company TEXT,
        fleet_size TEXT,
        about TEXT,
        message TEXT NOT NULL,
        user_id UUID,
        notified_at TIMESTAMPTZ
      );
      CREATE INDEX IF NOT EXISTS idx_contact_submissions_received ON contact_submissions(received_at DESC);
      CREATE INDEX IF NOT EXISTS idx_contact_submissions_unnotified ON contact_submissions(received_at) WHERE notified_at IS NULL;",
    )
    .await
    .map_err(|e| {
      console_error!("Contact table bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

async fn ensure_contact_table_once(client: &Client) -> Result<()> {
  if TABLE_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  ensure_contact_table(client).await?;
  TABLE_READY.store(true, Ordering::Relaxed);
  Ok(())
}

/// Stores one enquiry and returns its row id.
///
/// Unlike the notification email this is **not** best-effort: a contact
/// form that answers 202 without keeping the message drops business the
/// sender believes reached us. The email is the recoverable half (the row
/// is still there to mail later); the row is not.
pub async fn store_contact_submission(
  client: &Client,
  req: &ContactRequest,
  user_id: Option<Uuid>,
) -> Result<Uuid> {
  ensure_contact_table_once(client).await?;

  let name = req.name.trim();
  let email = req.email.trim();
  let message = req.message.trim();
  let company = req
    .company
    .as_deref()
    .map(str::trim)
    .filter(|c| !c.is_empty());
  let fleet_size = req.fleet_size.map(|s| s.wire());
  let about = req.about.as_deref().filter(|a| !a.is_empty());

  let row = client
    .query_typed_one(
      "INSERT INTO contact_submissions (name, email, company, fleet_size, about, message, user_id)
       VALUES ($1, $2, $3, $4, $5, $6, $7)
       RETURNING id;",
      &[
        (&name, Type::TEXT),
        (&email, Type::TEXT),
        (&company, Type::TEXT),
        (&fleet_size, Type::TEXT),
        (&about, Type::TEXT),
        (&message, Type::TEXT),
        (&user_id, Type::UUID),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Contact submission insert failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(row.get("id"))
}

/// Best-effort delivery of one formatted enquiry email, stamping
/// `notified_at` on success.
///
/// Reuses the existing notification plumbing rather than adding a provider
/// or a secret: recipient is the `OPS_ALERT_EMAIL` var (production-only by
/// design, one knob), transport is `send_via_usesend`. Staging and dev
/// degrade to a logged no-op, and even in production a send failure is
/// fire-and-log -- the sender's 202 never depends on delivery, and the row
/// stored above is what makes that safe.
pub async fn notify_contact_submission(
  env: &Env,
  client: &Client,
  submission_id: Uuid,
  subject: &str,
  text: &str,
) {
  let Some(recipient) = ops_alert_email(env) else {
    // Confirms a dev/staging submission reached the send boundary without
    // retaining it: `subject` carries the sender's name, and `text` the
    // whole enquiry, so only the id and byte count are logged. The row
    // keeps its NULL `notified_at`, which is the truth -- nobody was told.
    console_log!(
      "contact: OPS_ALERT_EMAIL not configured -- logging instead of sending (submission {submission_id}, body: {} bytes)",
      text.len()
    );
    return;
  };

  if let Err(e) = send_via_usesend(env, &recipient, subject, text).await {
    console_error!("contact: email send failed for submission {submission_id}: {e}");
    return;
  }

  if let Err(e) = client
    .execute_typed(
      "UPDATE contact_submissions SET notified_at = now() WHERE id = $1;",
      &[(&submission_id, Type::UUID)],
    )
    .await
  {
    // The mail went out; only the bookkeeping failed. Worth knowing about
    // (the row will look unnotified) but not worth failing anything over.
    console_error!("contact: notified_at stamp failed for submission {submission_id}: {e}");
  }
}
