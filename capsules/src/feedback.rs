//! User-feedback form types + email formatting -- shared by dovecote's
//! `POST /feedback` route and fancier's feedback modal.
//!
//! The email formatting lives here (not in dovecote's route/helper layer)
//! for the same reason `connection_state` does: it's pure string logic with
//! no Worker dependency, and `dovecote` is a wasm-only `cdylib` whose unit
//! tests can't run on a host target -- this crate's `cargo test -p capsules`
//! can, so the subject/body shape is directly testable.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Size cap enforced by dovecote's `POST /feedback` route on the `message`
/// field alone -- a few KB of prose is a long feedback note already, and the
/// whole payload lands verbatim in one ops email, so this is an abuse cap,
/// not a storage tuning knob. Exported so the dashboard's textarea can
/// pre-check (and `maxlength`) without duplicating the number, same
/// convention as `MAX_FIRMWARE_BYTES`/`MAX_LOG_DICTIONARY_BYTES`.
pub const MAX_FEEDBACK_MESSAGE_BYTES: usize = 4 * 1024;

/// Size cap on the entire raw `POST /feedback` request body, checked before
/// JSON parsing even starts -- the message cap above bounds the one
/// free-text field, this bounds everything else (so a payload can't smuggle
/// megabytes through `contact_email`/`page_context`/unknown keys).
pub const MAX_FEEDBACK_BODY_BYTES: usize = 8 * 1024;

/// Length cap on `contact_email` -- 254 octets is the RFC 5321 path limit;
/// anything longer is not a deliverable address.
pub const MAX_FEEDBACK_CONTACT_EMAIL_BYTES: usize = 254;

/// Length cap on `page_context` -- it's a dashboard route path (e.g.
/// `/flocks/<uuid>/pigeons/<id>`), not free text.
pub const MAX_FEEDBACK_PAGE_CONTEXT_BYTES: usize = 512;

/// The feedback form's category select. Wire values are snake_case
/// (`"bug"`, `"feature_request"`, `"general"`); an unknown value fails
/// deserialization, which dovecote's route surfaces as a 400 rather than
/// silently coercing.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackCategory {
  Bug,
  FeatureRequest,
  General,
}

impl FeedbackCategory {
  /// Human-readable label used in the notification email's subject/body
  /// (and reusable by the dashboard's select options).
  pub fn label(&self) -> &'static str {
    match self {
      FeedbackCategory::Bug => "Bug report",
      FeedbackCategory::FeatureRequest => "Feature request",
      FeedbackCategory::General => "General feedback",
    }
  }
}

/// Body for `POST /feedback`. Only `message` is required -- the route is
/// deliberately usable logged-out (public marketing pages link the same
/// form), so everything identifying is optional.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FeedbackRequest {
  pub message: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub category: Option<FeedbackCategory>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub contact_email: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub page_context: Option<String>,
}

/// Who submitted the feedback, when a Kratos session was present on the
/// request -- resolved server-side by dovecote (never trusted from the
/// body), `None` for anonymous submissions.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackSubmitter {
  pub user_id: String,
  pub email: Option<String>,
}

/// Formats the ops notification email for one feedback submission.
/// Returns `(subject, plain_text_body)` for `send_via_resend`-style
/// transports. Pure function of its inputs so the exact output is
/// unit-testable below.
pub fn format_feedback_email(
  req: &FeedbackRequest,
  submitter: Option<&FeedbackSubmitter>,
  submitted_at: OffsetDateTime,
) -> (String, String) {
  let category_label = req
    .category
    .map(|c| c.label())
    .unwrap_or(FeedbackCategory::General.label());

  // `[FEEDBACK]` prefix matches the `[OPS]`/`[SEVERITY]` subject convention
  // the existing Resend senders use, so inbox filters can key on one shape.
  let subject = format!("[FEEDBACK] {category_label} via the PidgeIoT dashboard");

  let submitted_at_str = submitted_at
    .format(&Rfc3339)
    .unwrap_or_else(|_| "unknown".to_string());

  let submitter_str = match submitter {
    Some(FeedbackSubmitter {
      user_id,
      email: Some(email),
    }) => format!("logged-in user {user_id} ({email})"),
    Some(FeedbackSubmitter {
      user_id,
      email: None,
    }) => format!("logged-in user {user_id} (no email trait)"),
    None => "anonymous (no session)".to_string(),
  };

  let contact_str = req
    .contact_email
    .as_deref()
    .filter(|s| !s.trim().is_empty())
    .unwrap_or("not provided");

  let page_str = req
    .page_context
    .as_deref()
    .filter(|s| !s.trim().is_empty())
    .unwrap_or("not provided");

  let text = format!(
    "New feedback submitted via the PidgeIoT dashboard feedback form.\n\
     \n\
     Category:      {category_label}\n\
     Submitted at:  {submitted_at_str}\n\
     Submitter:     {submitter_str}\n\
     Contact email: {contact_str}\n\
     Page:          {page_str}\n\
     \n\
     Message:\n\
     ----------------------------------------\n\
     {}\n\
     ----------------------------------------\n",
    req.message.trim()
  );

  (subject, text)
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::macros::datetime;

  fn base_request() -> FeedbackRequest {
    FeedbackRequest {
      message: "The shadow editor loses my edits when I switch tabs.".to_string(),
      category: Some(FeedbackCategory::Bug),
      contact_email: Some("reporter@example.com".to_string()),
      page_context: Some("/flocks/abc/pigeons/def".to_string()),
    }
  }

  #[test]
  fn subject_carries_category_label() {
    let (subject, _) =
      format_feedback_email(&base_request(), None, datetime!(2026-08-08 15:04:05 UTC));
    assert_eq!(subject, "[FEEDBACK] Bug report via the PidgeIoT dashboard");
  }

  #[test]
  fn missing_category_defaults_to_general() {
    let mut req = base_request();
    req.category = None;
    let (subject, body) = format_feedback_email(&req, None, datetime!(2026-08-08 15:04:05 UTC));
    assert_eq!(
      subject,
      "[FEEDBACK] General feedback via the PidgeIoT dashboard"
    );
    assert!(body.contains("Category:      General feedback"));
  }

  #[test]
  fn body_carries_message_contact_page_and_timestamp() {
    let (_, body) =
      format_feedback_email(&base_request(), None, datetime!(2026-08-08 15:04:05 UTC));
    assert!(body.contains("The shadow editor loses my edits when I switch tabs."));
    assert!(body.contains("Contact email: reporter@example.com"));
    assert!(body.contains("Page:          /flocks/abc/pigeons/def"));
    assert!(body.contains("Submitted at:  2026-08-08T15:04:05Z"));
  }

  #[test]
  fn anonymous_submission_is_labeled() {
    let (_, body) =
      format_feedback_email(&base_request(), None, datetime!(2026-08-08 15:04:05 UTC));
    assert!(body.contains("Submitter:     anonymous (no session)"));
  }

  #[test]
  fn authenticated_submitter_includes_id_and_email() {
    let submitter = FeedbackSubmitter {
      user_id: "8dc58300-70e6-4484-99f3-18ff7487b6fd".to_string(),
      email: Some("owner@example.com".to_string()),
    };
    let (_, body) = format_feedback_email(
      &base_request(),
      Some(&submitter),
      datetime!(2026-08-08 15:04:05 UTC),
    );
    assert!(body.contains(
      "Submitter:     logged-in user 8dc58300-70e6-4484-99f3-18ff7487b6fd (owner@example.com)"
    ));
  }

  #[test]
  fn empty_optional_fields_render_not_provided() {
    let mut req = base_request();
    req.contact_email = Some("   ".to_string());
    req.page_context = None;
    let (_, body) = format_feedback_email(&req, None, datetime!(2026-08-08 15:04:05 UTC));
    assert!(body.contains("Contact email: not provided"));
    assert!(body.contains("Page:          not provided"));
  }

  #[test]
  fn category_wire_format_is_snake_case() {
    assert_eq!(
      serde_json::to_string(&FeedbackCategory::FeatureRequest).unwrap(),
      "\"feature_request\""
    );
    let req: FeedbackRequest =
      serde_json::from_str(r#"{"message":"hi","category":"bug"}"#).unwrap();
    assert_eq!(req.category, Some(FeedbackCategory::Bug));
    assert!(
      serde_json::from_str::<FeedbackRequest>(r#"{"message":"hi","category":"spam"}"#).is_err()
    );
  }
}
