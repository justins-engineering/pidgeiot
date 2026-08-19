//! Storage, alerting, retention, and erasure for client error reports
//! (`POST /errors`). The pure logic -- normalizers, signature, wire types,
//! caps -- lives in `capsules::errors`; this module owns the Postgres side
//! and the ops email.
//!
//! Every incoming field is treated as hostile: the source of this platform
//! is public, so an attacker can compute real signature inputs and craft
//! arbitrary ones. The server re-normalizes message and route itself,
//! never accepts a client signature, validates the build hash shape,
//! clamps the client-claimed timestamp, and budgets the new-signature
//! email globally so crafted uniqueness can't become a mail flood.

use std::sync::atomic::{AtomicBool, Ordering};

use capsules::{ErrorReport, truncate_bytes};
use time::{Duration, OffsetDateTime};
use tokio_postgres::{Client, types::Type};
use uuid::Uuid;
use worker::{Env, Error, Result, console_error, console_log};

use super::alerts::send_via_usesend;
use super::hyperdrive::get_db_client;
use super::ops_probe::ops_alert_email;

/// Global budget for new-signature ops emails. Unique crafted messages
/// each mint a new signature, and the per-IP request limiter can't guard
/// the inbox (its counters are roughly per-colo) -- so at most this many
/// notification emails per hour, with the overflow folded into the next
/// allowed email as a suppressed-count line.
const NEW_SIGNATURE_EMAILS_PER_HOUR: i64 = 5;

/// Rows deleted per sweep statement per cron invocation -- same
/// CPU-budget reasoning as `retention.rs`'s batch limit, sized for a
/// table the 200-per-signature cap already keeps small.
const ERROR_RETENTION_BATCH_LIMIT: i64 = 5_000;

/// Ensured-once-per-isolate flag: this route is unauthenticated and
/// abuse-facing, so it must not pay (or let an attacker multiply) a DDL
/// round trip per request. The cron sweep still ensures unconditionally.
static TABLES_READY: AtomicBool = AtomicBool::new(false);

/// Idempotently ensures the `error_groups`/`error_events` tables + indexes
/// exist -- mirrors `ensure_alert_tables`' rationale (no separate migration
/// runner), and like every runtime `ensure_*` here it creates no triggers.
pub async fn ensure_error_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS error_groups (
        signature TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        message TEXT NOT NULL,
        location TEXT,
        first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
        last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
        first_build TEXT,
        last_build TEXT,
        occurrences BIGINT NOT NULL DEFAULT 0,
        notified_at TIMESTAMPTZ,
        resolved_at TIMESTAMPTZ
      );
      CREATE TABLE IF NOT EXISTS error_events (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        signature TEXT NOT NULL REFERENCES error_groups(signature) ON DELETE CASCADE,
        client_event_id UUID,
        occurred_at TIMESTAMPTZ NOT NULL,
        received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        user_id UUID,
        message TEXT,
        route TEXT,
        build TEXT,
        user_agent TEXT,
        stack TEXT,
        breadcrumbs JSONB,
        report_note TEXT,
        CONSTRAINT error_events_identity_requires_note CHECK (user_id IS NULL OR report_note IS NOT NULL)
      );
      CREATE INDEX IF NOT EXISTS idx_error_events_signature ON error_events(signature);
      CREATE INDEX IF NOT EXISTS idx_error_events_received ON error_events(received_at);
      CREATE INDEX IF NOT EXISTS idx_error_events_user ON error_events(user_id) WHERE user_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS idx_error_groups_last_seen ON error_groups(last_seen DESC);",
    )
    .await
    .map_err(|e| {
      console_error!("Error-report tables bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

async fn ensure_error_tables_once(client: &Client) -> Result<()> {
  if TABLES_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  ensure_error_tables(client).await?;
  TABLES_READY.store(true, Ordering::Relaxed);
  Ok(())
}

/// Control characters become spaces so a crafted message can't fake email
/// header/body structure or smuggle terminal escapes into a psql session.
fn strip_control(s: &str) -> String {
  s.chars()
    .map(|c| if c.is_control() { ' ' } else { c })
    .collect()
}

/// Stores one report: group upsert + event insert, then the (budgeted)
/// new-signature notification. `user_id`/`note` are `Some` only on the
/// identified manual path -- the anonymous branch never resolves either.
pub async fn ingest_error_report(
  env: &Env,
  client: &Client,
  report: &ErrorReport,
  user_id: Option<Uuid>,
  note: Option<&str>,
) -> Result<()> {
  ensure_error_tables_once(client).await?;

  // Server-side re-derivation, never client trust: normalized message and
  // route are recomputed here even though the client normalizes too --
  // a hostile or buggy client must not be able to store raw URLs or an
  // unredacted exemplar for the group's indefinite lifetime.
  let normalized_message = capsules::normalize_message(&report.message);
  let location = report
    .location
    .as_deref()
    .map(|l| strip_control(truncate_bytes(l, capsules::MAX_ERROR_FIELD_BYTES)));
  let signature = capsules::error_signature(report.kind, &normalized_message, location.as_deref());
  let route = capsules::normalize_route(&report.route);
  let build = report
    .build
    .as_deref()
    .filter(|b| capsules::is_valid_build(b))
    .map(str::to_string);
  let raw_message = truncate_bytes(&report.message, capsules::MAX_ERROR_MESSAGE_BYTES).to_string();
  let user_agent = report
    .user_agent
    .as_deref()
    .map(|ua| strip_control(truncate_bytes(ua, capsules::MAX_ERROR_FIELD_BYTES)));
  let stack = report
    .stack
    .as_deref()
    .map(|s| truncate_bytes(s, capsules::MAX_ERROR_STACK_BYTES).to_string());

  // The client's clock is a claim, not a fact -- clamp so a future-stamped
  // row can't dodge the received_at retention sweep's intent, and an
  // ancient one can't backdate a group's history.
  let now = OffsetDateTime::now_utc();
  let claimed =
    OffsetDateTime::from_unix_timestamp_nanos(report.occurred_at_ms as i128 * 1_000_000)
      .unwrap_or(now);
  let occurred_at = claimed.clamp(now - Duration::hours(24), now + Duration::hours(24));

  let breadcrumbs: Vec<capsules::Breadcrumb> = report
    .breadcrumbs
    .iter()
    .take(capsules::MAX_ERROR_BREADCRUMBS)
    .map(|b| capsules::Breadcrumb {
      age_ms: b.age_ms,
      kind: b.kind,
      detail: strip_control(truncate_bytes(
        &b.detail,
        capsules::MAX_ERROR_BREADCRUMB_DETAIL_BYTES,
      )),
    })
    .collect();
  let breadcrumbs_json = if breadcrumbs.is_empty() {
    None
  } else {
    serde_json::to_string(&breadcrumbs).ok()
  };

  let kind = report.kind.as_str();

  // `xmax = 0` on the upserted row distinguishes insert from update in the
  // same round trip, which is all the new-signature alert needs.
  let group_row = client
    .query_typed_one(
      "INSERT INTO error_groups (signature, kind, message, location, first_build, last_build, occurrences)
       VALUES ($1, $2, $3, $4, $5, $5, 1)
       ON CONFLICT (signature) DO UPDATE
         SET last_seen = now(),
             last_build = COALESCE(EXCLUDED.last_build, error_groups.last_build),
             occurrences = error_groups.occurrences + 1
       RETURNING (xmax = 0) AS is_new;",
      &[
        (&signature, Type::TEXT),
        (&kind, Type::TEXT),
        (&normalized_message, Type::TEXT),
        (&location, Type::TEXT),
        (&build, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Error group upsert failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  let is_new: bool = group_row.get("is_new");

  client
    .execute_typed(
      "INSERT INTO error_events
         (signature, client_event_id, occurred_at, user_id, message, route, build, user_agent, stack, breadcrumbs, report_note)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb, $11);",
      &[
        (&signature, Type::TEXT),
        (&report.client_event_id, Type::UUID),
        (&occurred_at, Type::TIMESTAMPTZ),
        (&user_id, Type::UUID),
        (&raw_message, Type::TEXT),
        (&route, Type::TEXT),
        (&build, Type::TEXT),
        (&user_agent, Type::TEXT),
        (&stack, Type::TEXT),
        (&breadcrumbs_json, Type::TEXT),
        (&note, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Error event insert failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  if is_new {
    notify_new_signature(
      env,
      client,
      &signature,
      kind,
      &normalized_message,
      location.as_deref(),
      &route,
      build.as_deref(),
    )
    .await;
  }

  Ok(())
}

/// One email per new signature, to `OPS_ALERT_EMAIL` (production-only by
/// design, so staging/dev never mail) -- the same notify-on-first-sight
/// restraint as the ops probe's transition emails. Best-effort throughout:
/// a notification failure never fails the ingest.
#[allow(clippy::too_many_arguments)]
async fn notify_new_signature(
  env: &Env,
  client: &Client,
  signature: &str,
  kind: &str,
  normalized_message: &str,
  location: Option<&str>,
  route: &str,
  build: Option<&str>,
) {
  let Some(recipient) = ops_alert_email(env) else {
    console_log!(
      "error report: new signature {signature} ({kind}); OPS_ALERT_EMAIL unset, not mailing"
    );
    return;
  };

  // Atomic claim under the hourly budget, in the warned_at style:
  // concurrent isolates can't double-send, and a signature that loses the
  // budget keeps notified_at NULL so the next allowed email counts it in
  // its suppressed line instead.
  let claimed = match client
    .query_typed(
      "UPDATE error_groups SET notified_at = now()
       WHERE signature = $1 AND notified_at IS NULL
         AND (SELECT count(*) FROM error_groups
              WHERE notified_at > now() - interval '1 hour') < $2
       RETURNING signature;",
      &[
        (&signature, Type::TEXT),
        (&NEW_SIGNATURE_EMAILS_PER_HOUR, Type::INT8),
      ],
    )
    .await
  {
    Ok(rows) => !rows.is_empty(),
    Err(e) => {
      console_error!("error report: notify claim failed for {signature}: {e}");
      false
    }
  };
  if !claimed {
    console_log!("error report: new signature {signature} suppressed by the email budget");
    return;
  }

  let suppressed: i64 = match client
    .query_typed(
      "SELECT count(*) AS n FROM error_groups
       WHERE notified_at IS NULL AND first_seen > now() - interval '1 hour';",
      &[],
    )
    .await
  {
    Ok(rows) => rows.first().map(|r| r.get("n")).unwrap_or(0),
    Err(_) => 0,
  };

  // Everything interpolated below is attacker-controllable text headed for
  // a human's inbox: control characters are already stripped upstream for
  // location/route, and the message is the normalized+redacted form, but
  // the subject excerpt gets flattened again so a crafted newline can't
  // forge headers or extra lines.
  let excerpt: String = strip_control(normalized_message)
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ");
  let excerpt = truncate_bytes(&excerpt, 72);
  let subject = format!("[ERROR] New: {excerpt}");

  let suppressed_line = if suppressed > 0 {
    format!(
      "\n{suppressed} further new signature(s) in the last hour were suppressed by the email budget; they sit in error_groups with notified_at unset.\n"
    )
  } else {
    String::new()
  };
  let text = format!(
    "A new error signature was recorded by the dashboard error reporter.\n\
     \n\
     Kind:      {kind}\n\
     Message:   {}\n\
     Location:  {}\n\
     Route:     {route}\n\
     Build:     {}\n\
     Signature: {signature}\n\
     {suppressed_line}\n\
     Only the first occurrence of a signature mails; follow the group in error_groups/error_events.\n",
    strip_control(normalized_message),
    location.unwrap_or("(none)"),
    build.unwrap_or("(unknown)"),
  );

  if let Err(e) = send_via_usesend(env, &recipient, &subject, &text).await {
    console_error!("error report: notification email failed for {signature}: {e}");
  }
}

/// Retention, riding the existing 5-minute cron like `retention.rs` and
/// for the same 5-cron-trigger account limit. Three sweeps, each
/// batch-limited: events past 90 days of `received_at`; per-signature
/// overflow beyond the newest 200 (manual reports and each group's oldest
/// few exemplars are exempt, so a flood aimed at a real group can't evict
/// the evidence that matters); and junk groups -- low-volume, stale, never
/// manually reported -- so crafted unique signatures age out while every
/// group that ever mattered stays.
pub async fn sweep_error_retention(env: &Env) -> Result<()> {
  let client = get_db_client(env).await?;
  ensure_error_tables(&client).await?;

  let aged = client
    .execute_typed(
      "DELETE FROM error_events WHERE id IN (
         SELECT id FROM error_events
         WHERE received_at < now() - interval '90 days'
         LIMIT $1
       );",
      &[(&ERROR_RETENTION_BATCH_LIMIT, Type::INT8)],
    )
    .await
    .map_err(|e| {
      console_error!("Error-report age sweep failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let overflow = client
    .execute_typed(
      "DELETE FROM error_events WHERE id IN (
         SELECT id FROM (
           SELECT id, report_note,
                  row_number() OVER (PARTITION BY signature ORDER BY received_at DESC) AS newest_rank,
                  row_number() OVER (PARTITION BY signature ORDER BY received_at ASC) AS oldest_rank
           FROM error_events
         ) ranked
         WHERE newest_rank > 200 AND oldest_rank > 5 AND report_note IS NULL
         LIMIT $1
       );",
      &[(&ERROR_RETENTION_BATCH_LIMIT, Type::INT8)],
    )
    .await
    .map_err(|e| {
      console_error!("Error-report per-signature sweep failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let junk = client
    .execute_typed(
      "DELETE FROM error_groups WHERE signature IN (
         SELECT g.signature FROM error_groups g
         WHERE g.occurrences <= 2
           AND g.last_seen < now() - interval '30 days'
           AND NOT EXISTS (
             SELECT 1 FROM error_events e
             WHERE e.signature = g.signature AND e.report_note IS NOT NULL
           )
         LIMIT $1
       );",
      &[(&ERROR_RETENTION_BATCH_LIMIT, Type::INT8)],
    )
    .await
    .map_err(|e| {
      console_error!("Error-report junk-group sweep failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  if aged > 0 || overflow > 0 || junk > 0 {
    console_log!(
      "Error-report retention: {aged} aged event(s), {overflow} overflow event(s), {junk} junk group(s) removed"
    );
  }

  Ok(())
}

/// Erases a user's identified report rows -- the automatic rows were never
/// theirs to begin with (no identity stored). Called by the authenticated
/// `DELETE /errors` route; the manual account-deletion runbook runs the
/// same statement.
pub async fn erase_user_error_reports(client: &Client, user_id: &Uuid) -> Result<u64> {
  ensure_error_tables_once(client).await?;
  client
    .execute_typed(
      "DELETE FROM error_events WHERE user_id = $1;",
      &[(&user_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Error-report erasure failed for user {user_id}: {e}");
      Error::RustError("Internal Server Error".into())
    })
}
