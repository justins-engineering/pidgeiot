//! The evidence half of marketing consent: the `consent_events` table and
//! the one statement that appends to it.
//!
//! Only this module writes the table, and it only ever inserts. The trait
//! on the Kratos identity is the current state and the person owns it;
//! these rows are the history, and nobody with an account can touch them.
//! See `capsules::consent` for the wording that goes with them, and
//! `docs/consent.md` for how the two are configured together.

use std::sync::atomic::{AtomicBool, Ordering};

use capsules::PRIVACY_NOTICE_VERSION;
use capsules::consent::{
  ConsentKind, ConsentSource, MARKETING_EMAIL_PURPOSE, MAX_CONSENT_CONTEXT_BYTES,
};
use tokio_postgres::Client;
use tokio_postgres::types::Type;
use uuid::Uuid;
use worker::{Error, Result, console_error};

/// Ensured-once-per-isolate flag, same convention as
/// `ensure_contact_table_once`: the hook fires on every registration and
/// every settings save, and none of those should pay a DDL round trip.
static TABLE_READY: AtomicBool = AtomicBool::new(false);

/// Idempotently ensures `consent_events` exists. The deploy-time apply is
/// `infra/migrations/2026-08-27-consent-events.sql`, which holds the same
/// statements and the reasoning behind each column; this is the
/// belt-and-suspenders that lets the route work against a database nobody
/// remembered to migrate.
pub async fn ensure_consent_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS consent_events (
        seq BIGSERIAL PRIMARY KEY,
        identity_id UUID NOT NULL,
        purpose TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('granted', 'withdrawn')),
        source TEXT NOT NULL CHECK (source IN ('registration', 'settings', 'import')),
        notice_version TEXT NOT NULL,
        flow_id UUID,
        ip TEXT,
        user_agent TEXT,
        at TIMESTAMPTZ NOT NULL DEFAULT now()
      );
      ALTER TABLE consent_events ADD COLUMN IF NOT EXISTS ip TEXT;
      ALTER TABLE consent_events ADD COLUMN IF NOT EXISTS user_agent TEXT;
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
  // Bounded rather than trusted: both arrive as caller-supplied strings.
  // Truncation on a char boundary, so a multi-byte agent string cannot
  // produce invalid UTF-8 at the cut.
  let clamp = |v: Option<&str>| {
    v.filter(|s| !s.is_empty()).map(|s| {
      let end = s
        .char_indices()
        .map(|(i, _)| i)
        .chain([s.len()])
        .take_while(|i| *i <= MAX_CONSENT_CONTEXT_BYTES)
        .last()
        .unwrap_or(0);
      s[..end].to_string()
    })
  };
  let ip = clamp(ip);
  let user_agent = clamp(user_agent);

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
