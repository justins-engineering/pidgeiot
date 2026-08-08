use worker::{Env, console_error, console_log};

use super::alerts::send_via_resend;
use super::ops_probe::ops_alert_email;

/// Best-effort delivery of one formatted feedback email (task #13) --
/// subject/body come from `capsules::format_feedback_email` (host-testable
/// there; see that module's doc comment). Reuses the existing notification
/// plumbing end to end rather than adding any new provider or secret:
/// recipient is the `OPS_ALERT_EMAIL` var (`ops_probe::ops_alert_email` --
/// production-only by design, one knob), transport is `send_via_resend`
/// (`RESEND_API_KEY` secret). Staging/dev therefore degrade to a logged
/// no-op here (no recipient configured), and even in production a Resend
/// failure is fire-and-log -- the submitter's 202 never depends on
/// delivery, matching every other notification path in this codebase.
pub async fn send_feedback_email(env: &Env, subject: &str, text: &str) {
  let Some(recipient) = ops_alert_email(env) else {
    // The full body is logged (not just dropped) so a dev/staging
    // submission is still visible in `wrangler dev`/tail output --
    // feedback content isn't a credential, and this is the only place it
    // lands when no recipient is configured.
    console_log!(
      "feedback: OPS_ALERT_EMAIL not configured -- logging instead of sending\nsubject: {subject}\n{text}"
    );
    return;
  };

  if let Err(e) = send_via_resend(env, &recipient, subject, text).await {
    console_error!("feedback: email send failed: {e}");
  }
}
