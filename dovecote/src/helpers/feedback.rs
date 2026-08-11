use worker::{Env, console_error, console_log};

use super::alerts::send_via_usesend;
use super::ops_probe::ops_alert_email;

/// Best-effort delivery of one formatted feedback email -- subject/body
/// come from `capsules::format_feedback_email`. Reuses the existing
/// notification plumbing rather than adding a new provider or secret:
/// recipient is the `OPS_ALERT_EMAIL` var (`ops_probe::ops_alert_email` --
/// production-only by design, one knob), transport is `send_via_usesend`
/// (`RESEND_API_KEY` secret). Staging/dev degrade to a logged no-op, and
/// even in production a Resend failure is fire-and-log -- the submitter's
/// 202 never depends on delivery, matching every other notification path
/// in this codebase.
pub async fn send_feedback_email(env: &Env, subject: &str, text: &str) {
  let Some(recipient) = ops_alert_email(env) else {
    // Log the full body (not just drop it) so a dev/staging submission is
    // still visible in wrangler tail output when no recipient is set.
    console_log!(
      "feedback: OPS_ALERT_EMAIL not configured -- logging instead of sending\nsubject: {subject}\n{text}"
    );
    return;
  };

  if let Err(e) = send_via_usesend(env, &recipient, subject, text).await {
    console_error!("feedback: email send failed: {e}");
  }
}
