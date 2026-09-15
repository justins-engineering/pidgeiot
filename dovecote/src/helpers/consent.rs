//! The evidence half of consent: the `consent_events` table and the
//! statements that append to it.
//!
//! Only this module writes the table, and it only ever inserts. Two
//! purposes share it. For marketing consent the trait on the Kratos
//! identity is the current state and the person owns it, and these rows
//! are the history. For Terms assent there is no trait at all: the row is
//! the entire record that a published version was accepted, which is why
//! its version and its time are both the server's.
//!
//! The two writers stay separate on purpose. Each hard-codes its own
//! purpose and its own version, so no caller can stamp a terms row with
//! the privacy date. See `capsules::consent` for the wording that goes
//! with them, and `docs/consent.md` for how it is all configured.

use std::sync::atomic::{AtomicBool, Ordering};

use capsules::consent::{
  ConsentKind, ConsentSource, MARKETING_EMAIL_PURPOSE, MAX_CONSENT_CONTEXT_BYTES,
  TERMS_OF_SERVICE_PURPOSE,
};
use capsules::{PRIVACY_NOTICE_VERSION, TERMS_VERSION};
use time::OffsetDateTime;
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Ensured-once-per-isolate flag, same convention as
/// `ensure_contact_table_once`: the hook fires on every registration and
/// every settings save, and none of those should pay a DDL round trip.
static TABLE_READY: AtomicBool = AtomicBool::new(false);

/// Idempotently ensures `consent_events` exists and carries every column
/// and value the code writes. The deploy-time applies are
/// `infra/migrations/2026-08-27-consent-events.sql` and
/// `infra/migrations/2026-09-14-terms-assent.sql`, which hold the same
/// statements and the reasoning behind each column; this is the
/// belt-and-suspenders that lets the routes work against a database nobody
/// remembered to migrate.
///
/// The `source` widening has to be conditional. `CREATE TABLE IF NOT
/// EXISTS` is inert against a table that already exists, so without it the
/// first `gate` insert fails against every deployed database; a blind
/// DROP/ADD instead would take the table's exclusive lock on every isolate
/// boot. The loop finds nothing after the first run.
pub async fn ensure_consent_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS consent_events (
        seq BIGSERIAL PRIMARY KEY,
        identity_id UUID NOT NULL,
        purpose TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('granted', 'withdrawn')),
        source TEXT NOT NULL
          CHECK (source IN ('registration', 'settings', 'import', 'gate', 'checkout')),
        notice_version TEXT NOT NULL,
        flow_id UUID,
        org_id UUID,
        ip TEXT,
        user_agent TEXT,
        at TIMESTAMPTZ NOT NULL DEFAULT now()
      );
      ALTER TABLE consent_events ADD COLUMN IF NOT EXISTS ip TEXT;
      ALTER TABLE consent_events ADD COLUMN IF NOT EXISTS user_agent TEXT;
      ALTER TABLE consent_events ADD COLUMN IF NOT EXISTS org_id UUID;
      DO $$
      DECLARE c record;
      BEGIN
        FOR c IN
          SELECT conname FROM pg_constraint
           WHERE conrelid = 'consent_events'::regclass AND contype = 'c'
             AND pg_get_constraintdef(oid) LIKE '%source%'
             AND pg_get_constraintdef(oid) NOT LIKE '%gate%'
        LOOP
          EXECUTE format('ALTER TABLE consent_events DROP CONSTRAINT %I', c.conname);
          EXECUTE 'ALTER TABLE consent_events ADD CONSTRAINT consent_events_source_check
                   CHECK (source IN (''registration'',''settings'',''import'',''gate'',''checkout''))';
        END LOOP;
      END $$;
      CREATE INDEX IF NOT EXISTS idx_consent_events_identity
        ON consent_events(identity_id, purpose, seq DESC);",
    )
    .await
    .map_err(|e| {
      console_error!("Consent events table bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

async fn ensure_consent_tables_once(client: &Client) -> Result<()> {
  if TABLE_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  ensure_consent_tables(client).await?;
  TABLE_READY.store(true, Ordering::Relaxed);
  Ok(())
}

/// Bounds a caller-supplied `ip` or `user_agent` rather than trusting it.
/// Truncation on a char boundary, so a multi-byte agent string cannot
/// produce invalid UTF-8 at the cut.
fn clamp_context(value: Option<&str>) -> Option<String> {
  value.filter(|s| !s.is_empty()).map(|s| {
    let end = s
      .char_indices()
      .map(|(i, _)| i)
      .chain([s.len()])
      .take_while(|i| *i <= MAX_CONSENT_CONTEXT_BYTES)
      .last()
      .unwrap_or(0);
    s[..end].to_string()
  })
}

/// Appends a consent event, but only if it moves the state.
///
/// The decision and the write are one statement on purpose. Reading the
/// last event and then inserting would leave a window where two settings
/// saves in flight both see the old state and both append, and a history
/// with a duplicate transition in it is a history that has to be
/// explained. The `WHERE` clause is `capsules::consent_transition`'s rule
/// expressed in SQL: an identity with no row on file has never consented,
/// so the absence reads as `withdrawn`.
///
/// `notice_version` is stamped here from `PRIVACY_NOTICE_VERSION` rather
/// than taken from the caller. Kratos cannot know which notice was on
/// screen, and a version the caller supplies is an assertion, not a
/// record.
///
/// Returns the `seq` of the row written, or `None` when the flow changed
/// nothing.
pub async fn record_consent_event(
  client: &Client,
  identity_id: Uuid,
  granted: bool,
  source: ConsentSource,
  flow_id: Option<Uuid>,
  ip: Option<&str>,
  user_agent: Option<&str>,
) -> Result<Option<i64>> {
  ensure_consent_tables_once(client).await?;

  let kind = if granted {
    ConsentKind::Granted
  } else {
    ConsentKind::Withdrawn
  }
  .as_str();
  let source = source.as_str();
  let purpose = MARKETING_EMAIL_PURPOSE;
  let notice_version = PRIVACY_NOTICE_VERSION;
  let ip = clamp_context(ip);
  let user_agent = clamp_context(user_agent);

  let rows = client
    .query_typed(
      "INSERT INTO consent_events
         (identity_id, purpose, kind, source, notice_version, flow_id, ip, user_agent)
       SELECT $1, $2, $3, $4, $5, $6, $7, $8
       WHERE $3 <> COALESCE(
         (SELECT e.kind FROM consent_events e
           WHERE e.identity_id = $1 AND e.purpose = $2
           ORDER BY e.seq DESC LIMIT 1),
         'withdrawn')
       RETURNING seq;",
      &[
        (&identity_id, Type::UUID),
        (&purpose, Type::TEXT),
        (&kind, Type::TEXT),
        (&source, Type::TEXT),
        (&notice_version, Type::TEXT),
        (&flow_id, Type::UUID),
        (&ip, Type::TEXT),
        (&user_agent, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Consent event insert failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(|row| row.get("seq")))
}

/// Appends a Terms assent. Purpose and version are hard-coded here for the
/// same reason the marketing writer hard-codes its own: a caller that could
/// pass either pair could stamp a terms row with the privacy date.
///
/// A `Checkout` assent always appends. It names an organization and carries
/// the authority-to-bind representation the Terms extract, so it is a
/// distinct act that an earlier gate row for the same version must not
/// suppress. Every other source appends only when this identity has no
/// grant on file for this version yet, and that decision is inside the
/// INSERT rather than a read before it: two tabs clicking Accept at the
/// same moment would otherwise both read "nothing on file" and both append.
///
/// `consent_transition` is deliberately not reused. Its rule suppresses a
/// second grant for the same identity and purpose regardless of version,
/// which is exactly the shape of assent to a new version.
///
/// No address and no user agent. The published notice describes both only
/// as transient web logs, and this row is meant to outlive the account, so
/// storing them here would keep a category of personal data the notice
/// does not disclose. The memo asks for version, account and time, and all
/// three are still here.
///
/// Returns the `seq` of the row written, or `None` when one was already on
/// file.
pub async fn record_terms_assent(
  client: &Client,
  identity_id: Uuid,
  source: ConsentSource,
  org_id: Option<Uuid>,
) -> Result<Option<i64>> {
  ensure_consent_tables_once(client).await?;

  let kind = ConsentKind::Granted.as_str();
  let source = source.as_str();
  let purpose = TERMS_OF_SERVICE_PURPOSE;
  let notice_version = TERMS_VERSION;

  let sql = if source == ConsentSource::Checkout.as_str() {
    "INSERT INTO consent_events
       (identity_id, purpose, kind, source, notice_version, org_id)
     SELECT $1, $2, $3, $4, $5, $6
     RETURNING seq;"
  } else {
    "INSERT INTO consent_events
       (identity_id, purpose, kind, source, notice_version, org_id)
     SELECT $1, $2, $3, $4, $5, $6
     WHERE NOT EXISTS (
       SELECT 1 FROM consent_events e
        WHERE e.identity_id = $1 AND e.purpose = $2
          AND e.notice_version = $5 AND e.kind = $3)
     RETURNING seq;"
  };

  let rows = client
    .query_typed(
      sql,
      &[
        (&identity_id, Type::UUID),
        (&purpose, Type::TEXT),
        (&kind, Type::TEXT),
        (&source, Type::TEXT),
        (&notice_version, Type::TEXT),
        (&org_id, Type::UUID),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Terms assent insert failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.first().map(|row| row.get("seq")))
}

/// The account's most recent Terms assent, or `None` if it has never given
/// one.
///
/// Anchored on `now()`: Hyperdrive will not cache a statement carrying a
/// volatile function, and this read gates a screen the user has just
/// cleared. Without it the gate reappears for up to a minute after a
/// successful accept, which dev cannot reproduce because its Hyperdrive
/// binding is a local connection string with no query cache.
pub async fn load_terms_assent(
  client: &Client,
  identity_id: &Uuid,
) -> Result<Option<(String, OffsetDateTime)>> {
  ensure_consent_tables_once(client).await?;

  let purpose = TERMS_OF_SERVICE_PURPOSE;
  let kind = ConsentKind::Granted.as_str();
  let rows = client
    .query_typed(
      "SELECT notice_version, at, now() AS read_at
         FROM consent_events
        WHERE identity_id = $1 AND purpose = $2 AND kind = $3
        ORDER BY seq DESC LIMIT 1;",
      &[
        (identity_id, Type::UUID),
        (&purpose, Type::TEXT),
        (&kind, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Terms assent read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .first()
      .map(|row| (row.get("notice_version"), row.get("at"))),
  )
}
