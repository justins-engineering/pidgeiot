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
    // Confirms a dev/staging submission reached this point without
    // retaining it: `subject` is just `format_feedback_email`'s category
    // label (safe to log in full), but `text` is the full body -- resolved
    // user id, email, and free-text message -- so only its byte count is
    // logged, not its content.
    console_log!(
      "feedback: OPS_ALERT_EMAIL not configured -- logging instead of sending (subject: {subject}, body: {} bytes)",
      text.len()
    );
    return;
  };

  if let Err(e) = send_via_usesend(env, &recipient, subject, text).await {
    console_error!("feedback: email send failed: {e}");
  }
}
