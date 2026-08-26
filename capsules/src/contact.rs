//! Contact-form types, validation and email formatting -- shared by
//! dovecote's `POST /contact` route and fancier's `/contact/` page.
//!
//! Same split as `feedback`: the pure logic lives here because `dovecote`
//! is a wasm-only `cdylib` whose unit tests cannot run on a host target,
//! while `cargo test -p capsules` can. Validation in particular belongs
//! here rather than inline in the route -- the form and the route must
//! agree on what "valid" means, and the only way to guarantee that is for
//! both to call the same function.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Size cap on the entire raw `POST /contact` request body, checked before
/// JSON parsing starts -- the per-field caps below bound each field, this
/// bounds everything else (unknown keys, whitespace padding).
pub const MAX_CONTACT_BODY_BYTES: usize = 8 * 1024;

/// The one free-text field. A few KB is a long enquiry already, and the
/// whole thing lands verbatim in one ops email.
pub const MAX_CONTACT_MESSAGE_BYTES: usize = 4 * 1024;

/// Minimum length for `message`, so a single stray character cannot
/// become an ops email. Low enough that "Do you support LoRaWAN?" passes.
pub const MIN_CONTACT_MESSAGE_BYTES: usize = 10;

pub const MAX_CONTACT_NAME_BYTES: usize = 128;

/// 254 octets is the RFC 5321 path limit; anything longer is not a
/// deliverable address.
pub const MAX_CONTACT_EMAIL_BYTES: usize = 254;

pub const MAX_CONTACT_COMPANY_BYTES: usize = 128;

/// `about` is a short funnel-context slug set by whichever link opened the
/// form (`fleet` from the pricing page's Fleet tier), not free text.
pub const MAX_CONTACT_ABOUT_BYTES: usize = 32;

/// How long a person must have had the form open before a submission is
/// believable. Scripted posts fill and submit in one pass with no render
/// in between, so they arrive far under this; a human cannot type a name,
/// an address and ten characters of message faster. Deliberately short:
/// this is a floor no real submission reaches, not a deliberation timer.
pub const MIN_CONTACT_FILL_MS: u32 = 2_000;

/// The fleet-size select. Boundaries follow the published pricing tiers
/// (Perch 10, Builder 50, Growth 250, Scale 1,500, Fleet 10,000) so an
/// answer maps straight onto a tier conversation. Wire values are
/// explicit `rename`s because the natural names start with digits.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactFleetSize {
  #[serde(rename = "under_50")]
  Under50,
  #[serde(rename = "50_to_250")]
  From50To250,
  #[serde(rename = "250_to_1500")]
  From250To1500,
  #[serde(rename = "1500_to_10000")]
  From1500To10000,
  #[serde(rename = "over_10000")]
  Over10000,
  #[serde(rename = "not_sure")]
  NotSure,
}

impl ContactFleetSize {
  /// Human-readable label, used both for the select's options and in the
  /// notification email, so the two can never describe a band differently.
  pub fn label(&self) -> &'static str {
    match self {
      ContactFleetSize::Under50 => "Fewer than 50 devices",
      ContactFleetSize::From50To250 => "50 to 250 devices",
      ContactFleetSize::From250To1500 => "250 to 1,500 devices",
      ContactFleetSize::From1500To10000 => "1,500 to 10,000 devices",
      ContactFleetSize::Over10000 => "More than 10,000 devices",
      ContactFleetSize::NotSure => "Not sure yet",
    }
  }

  /// Wire value, so the form's `<option value>` and serde stay in step
  /// without the option list restating the renames above.
  pub fn wire(&self) -> &'static str {
    match self {
      ContactFleetSize::Under50 => "under_50",
      ContactFleetSize::From50To250 => "50_to_250",
      ContactFleetSize::From250To1500 => "250_to_1500",
      ContactFleetSize::From1500To10000 => "1500_to_10000",
      ContactFleetSize::Over10000 => "over_10000",
      ContactFleetSize::NotSure => "not_sure",
    }
  }

  /// Every variant, in the order the select should offer them.
  pub const ALL: [ContactFleetSize; 6] = [
    ContactFleetSize::Under50,
    ContactFleetSize::From50To250,
    ContactFleetSize::From250To1500,
    ContactFleetSize::From1500To10000,
    ContactFleetSize::Over10000,
    ContactFleetSize::NotSure,
  ];

  pub fn from_wire(value: &str) -> Option<ContactFleetSize> {
    ContactFleetSize::ALL
      .into_iter()
      .find(|s| s.wire() == value)
  }
}

/// Body for `POST /contact`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct ContactRequest {
  pub name: String,
  pub email: String,
  pub message: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub company: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub fleet_size: Option<ContactFleetSize>,
  /// Which link opened the form, so a Fleet enquiry is recognisable in the
  /// ops inbox without the sender having to say so.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub about: Option<String>,
  /// Honeypot. The form renders it off-screen with `aria-hidden` and
  /// `tabindex="-1"`, so a person never sees or tabs into it and it is
  /// always empty from a real browser; a form-filling script that walks
  /// the DOM fills it because it is named like a field worth filling.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub website: Option<String>,
  /// Milliseconds between the form mounting and the submit click.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub elapsed_ms: Option<u32>,
  /// The one-time token Cloudflare Turnstile issued to the browser for
  /// this submission. Optional on the wire because the route only demands
  /// it once its verification secret is configured; it is spent at
  /// verification, never stored, and not part of the enquiry.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub turnstile_token: Option<String>,
}

/// Why a submission was refused. Carries its own HTTP status and
/// user-facing message so the route maps a rejection without a second
/// `match` that could drift from this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactRejection {
  NameMissing,
  NameTooLong,
  EmailMissing,
  EmailMalformed,
  EmailTooLong,
  MessageTooShort,
  MessageTooLong,
  CompanyTooLong,
  AboutMalformed,
  /// The honeypot field arrived non-empty.
  Honeypot,
  /// Submitted faster than `MIN_CONTACT_FILL_MS`, or with no timing at all.
  TooFast,
}

impl ContactRejection {
  pub fn status(&self) -> u16 {
    match self {
      ContactRejection::MessageTooLong => 413,
      // A honeypot hit answers 202 like a success: telling a script which
      // control caught it is telling it what to change. Nothing is stored
      // or emailed, which is the part that matters.
      ContactRejection::Honeypot => 202,
      _ => 400,
    }
  }

  /// Copy the form renders verbatim.
  ///
  /// `TooFast` gets a recoverable message rather than a silent drop: the
  /// floor is low enough that only a script should reach it, but a human
  /// who somehow does must be able to fix it by clicking send again --
  /// silently discarding a real enquiry is the worse failure here. The
  /// honeypot, which no real browser can trip, stays silent.
  pub fn message(&self) -> &'static str {
    match self {
      ContactRejection::NameMissing => "Please tell us your name.",
      ContactRejection::NameTooLong => "That name is too long.",
      ContactRejection::EmailMissing => "Please give us an email address to reply to.",
      ContactRejection::EmailMalformed => "That does not look like an email address.",
      ContactRejection::EmailTooLong => "That email address is too long.",
      ContactRejection::MessageTooShort => "Please tell us a little more about what you need.",
      ContactRejection::MessageTooLong => "That message is too long. Please shorten it.",
      ContactRejection::CompanyTooLong => "That company name is too long.",
      ContactRejection::AboutMalformed => {
        "That link is malformed. Please try again from the page you came from."
      }
      ContactRejection::Honeypot => "Thanks, we will be in touch.",
      ContactRejection::TooFast => {
        "That came in faster than a person can type. Please send it again."
      }
    }
  }
}

/// Shape-only address check: exactly one `@`, both halves non-empty, a dot
/// in the domain, and no whitespace, control characters or commas anywhere.
///
/// Deliberately not a full RFC 5322 grammar. The only proof an address is
/// real is a reply arriving, so this rejects what is obviously not an
/// address (and what could forge structure in the notification email's
/// headers) and lets everything else through rather than turning away a
/// valid but unusual address.
pub fn is_plausible_email(email: &str) -> bool {
  let email = email.trim();
  if email.is_empty() || email.len() > MAX_CONTACT_EMAIL_BYTES {
    return false;
  }
  if email
    .chars()
    .any(|c| c.is_whitespace() || c.is_control() || c == ',' || c == ';' || c == '<' || c == '>')
  {
    return false;
  }
  let Some((local, domain)) = email.split_once('@') else {
    return false;
  };
  if local.is_empty() || domain.contains('@') {
    return false;
  }
  // A domain needs a dot-separated label pair, and neither half of any
  // split may be empty (rules out `a@.com`, `a@com.`, `a@b..c`).
  let mut labels = domain.split('.');
  domain.contains('.') && labels.all(|label| !label.is_empty())
}

/// The single definition of a valid submission, called by both the form
/// (before sending) and the route (before storing).
pub fn validate(req: &ContactRequest) -> Result<(), ContactRejection> {
  // Abuse controls first: a script that trips one should not be told
  // which of its other fields were also wrong.
  if req.website.as_deref().is_some_and(|w| !w.trim().is_empty()) {
    return Err(ContactRejection::Honeypot);
  }
  if !req.elapsed_ms.is_some_and(|ms| ms >= MIN_CONTACT_FILL_MS) {
    return Err(ContactRejection::TooFast);
  }

  let name = req.name.trim();
  if name.is_empty() {
    return Err(ContactRejection::NameMissing);
  }
  if req.name.len() > MAX_CONTACT_NAME_BYTES {
    return Err(ContactRejection::NameTooLong);
  }

  let email = req.email.trim();
  if email.is_empty() {
    return Err(ContactRejection::EmailMissing);
  }
  if req.email.len() > MAX_CONTACT_EMAIL_BYTES {
    return Err(ContactRejection::EmailTooLong);
  }
  if !is_plausible_email(email) {
    return Err(ContactRejection::EmailMalformed);
  }

  let message = req.message.trim();
  if message.len() < MIN_CONTACT_MESSAGE_BYTES {
    return Err(ContactRejection::MessageTooShort);
  }
  if req.message.len() > MAX_CONTACT_MESSAGE_BYTES {
    return Err(ContactRejection::MessageTooLong);
  }

  if req
    .company
    .as_ref()
    .is_some_and(|c| c.len() > MAX_CONTACT_COMPANY_BYTES)
  {
    return Err(ContactRejection::CompanyTooLong);
  }

  // A slug, not prose: it is chosen by our own links, and it reaches the
  // email subject, so anything that is not a plain token is a forgery
  // attempt rather than a typo.
  if let Some(about) = req.about.as_deref().filter(|a| !a.is_empty()) {
    let shaped = about.len() <= MAX_CONTACT_ABOUT_BYTES
      && about
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !shaped {
      return Err(ContactRejection::AboutMalformed);
    }
  }

  Ok(())
}

/// Control characters become spaces, so a crafted field cannot fake email
/// header or body structure (a newline in a subject is header injection)
/// or smuggle terminal escapes into a psql session. Same reasoning as
/// `helpers/errors.rs::strip_control` in dovecote.
fn single_line(s: &str) -> String {
  s.trim()
    .chars()
    .map(|c| if c.is_control() { ' ' } else { c })
    .collect()
}

/// Formats the ops notification email for one contact submission.
/// Returns `(subject, plain_text_body)`. Pure function of its inputs so
/// the exact output is unit-testable below.
pub fn format_contact_email(
  req: &ContactRequest,
  submitted_at: OffsetDateTime,
) -> (String, String) {
  let name = single_line(&req.name);
  let company = req
    .company
    .as_deref()
    .map(single_line)
    .filter(|c| !c.is_empty());

  // `[CONTACT]` prefix matches the `[FEEDBACK]`/`[OPS]` subject
  // convention, so one inbox filter can key on the shape.
  let who = match &company {
    Some(company) => format!("{name} at {company}"),
    None => name.clone(),
  };
  let subject = match req.fleet_size {
    Some(size) => format!("[CONTACT] {who} ({})", size.label()),
    None => format!("[CONTACT] {who}"),
  };

  let submitted_at_str = submitted_at
    .format(&Rfc3339)
    .unwrap_or_else(|_| "unknown".to_string());

  let fleet_str = req
    .fleet_size
    .map(|s| s.label())
    .unwrap_or("not answered")
    .to_string();
  let about_str = req
    .about
    .as_deref()
    .map(single_line)
    .filter(|a| !a.is_empty())
    .unwrap_or_else(|| "general".to_string());

  let text = format!(
    "New enquiry from the PidgeIoT contact form.\n\
     \n\
     Name:         {name}\n\
     Email:        {}\n\
     Company:      {}\n\
     Fleet size:   {fleet_str}\n\
     Came from:    {about_str}\n\
     Submitted at: {submitted_at_str}\n\
     \n\
     Message:\n\
     ----------------------------------------\n\
     {}\n\
     ----------------------------------------\n",
    single_line(&req.email),
    company.as_deref().unwrap_or("not provided"),
    req.message.trim()
  );

  (subject, text)
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::macros::datetime;

  fn valid_request() -> ContactRequest {
    ContactRequest {
      name: "Dana Okafor".to_string(),
      email: "dana@example.com".to_string(),
      message: "We have about 900 water meters and need OTA updates.".to_string(),
      company: Some("Meterworks".to_string()),
      fleet_size: Some(ContactFleetSize::From250To1500),
      about: Some("fleet".to_string()),
      website: None,
      elapsed_ms: Some(9_000),
      turnstile_token: None,
    }
  }

  #[test]
  fn a_complete_submission_validates() {
    assert_eq!(validate(&valid_request()), Ok(()));
  }

  #[test]
  fn optional_fields_may_all_be_absent() {
    let req = ContactRequest {
      company: None,
      fleet_size: None,
      about: None,
      ..valid_request()
    };
    assert_eq!(validate(&req), Ok(()));
  }

  #[test]
  fn name_and_email_are_required() {
    let mut req = valid_request();
    req.name = "   ".to_string();
    assert_eq!(validate(&req), Err(ContactRejection::NameMissing));

    let mut req = valid_request();
    req.email = String::new();
    assert_eq!(validate(&req), Err(ContactRejection::EmailMissing));
  }

  #[test]
  fn message_has_a_floor_and_a_ceiling() {
    let mut req = valid_request();
    req.message = "hi".to_string();
    assert_eq!(validate(&req), Err(ContactRejection::MessageTooShort));

    let mut req = valid_request();
    req.message = "x".repeat(MAX_CONTACT_MESSAGE_BYTES + 1);
    assert_eq!(validate(&req), Err(ContactRejection::MessageTooLong));
    assert_eq!(ContactRejection::MessageTooLong.status(), 413);
  }

  #[test]
  fn length_caps_are_byte_counts_not_char_counts() {
    let mut req = valid_request();
    // Two bytes per char, so half the cap in chars is exactly the cap.
    req.name = "é".repeat(MAX_CONTACT_NAME_BYTES / 2 + 1);
    assert_eq!(validate(&req), Err(ContactRejection::NameTooLong));

    let mut req = valid_request();
    req.company = Some("é".repeat(MAX_CONTACT_COMPANY_BYTES / 2 + 1));
    assert_eq!(validate(&req), Err(ContactRejection::CompanyTooLong));
  }

  #[test]
  fn email_shape_is_checked() {
    for bad in [
      "no-at-sign",
      "two@at@signs.com",
      "@nolocal.com",
      "nodomain@",
      "no-dot@localhost",
      "trailing@dot.",
      "leading@.dot",
      "double@dots..com",
      "spaces in@example.com",
      "comma,injection@example.com",
      "header\ninjection@example.com",
      "Name <real@example.com>",
    ] {
      let mut req = valid_request();
      req.email = bad.to_string();
      assert!(
        matches!(
          validate(&req),
          Err(ContactRejection::EmailMalformed | ContactRejection::EmailMissing)
        ),
        "{bad} should not pass the shape check"
      );
    }

    for good in [
      "a@b.co",
      "first.last+tag@sub.domain.example",
      "UPPER@Example.COM",
      "dashed-name@ex-ample.io",
      "  padded@example.com  ",
    ] {
      let mut req = valid_request();
      req.email = good.to_string();
      assert_eq!(validate(&req), Ok(()), "{good} should pass");
    }
  }

  #[test]
  fn honeypot_refuses_but_reads_as_success() {
    let mut req = valid_request();
    req.website = Some("https://example.com".to_string());
    assert_eq!(validate(&req), Err(ContactRejection::Honeypot));
    // The status a script sees is indistinguishable from a real send.
    assert_eq!(ContactRejection::Honeypot.status(), 202);

    // An empty honeypot is what a real browser always sends.
    req.website = Some(String::new());
    assert_eq!(validate(&req), Ok(()));
  }

  #[test]
  fn honeypot_outranks_every_other_field_error() {
    let req = ContactRequest {
      name: String::new(),
      email: "not-an-email".to_string(),
      message: "x".to_string(),
      website: Some("filled".to_string()),
      ..valid_request()
    };
    assert_eq!(validate(&req), Err(ContactRejection::Honeypot));
  }

  #[test]
  fn submissions_faster_than_the_floor_are_refused() {
    let mut req = valid_request();
    req.elapsed_ms = Some(MIN_CONTACT_FILL_MS - 1);
    assert_eq!(validate(&req), Err(ContactRejection::TooFast));

    req.elapsed_ms = Some(MIN_CONTACT_FILL_MS);
    assert_eq!(validate(&req), Ok(()));
  }

  #[test]
  fn missing_timing_counts_as_too_fast() {
    let mut req = valid_request();
    req.elapsed_ms = None;
    assert_eq!(validate(&req), Err(ContactRejection::TooFast));
  }

  #[test]
  fn about_must_be_a_slug() {
    for bad in ["Fleet", "has space", "semi;colon", "<script>", "ünicode"] {
      let mut req = valid_request();
      req.about = Some(bad.to_string());
      assert_eq!(
        validate(&req),
        Err(ContactRejection::AboutMalformed),
        "{bad} should not pass"
      );
    }
    for good in ["fleet", "use-cases", "pricing_2", ""] {
      let mut req = valid_request();
      req.about = Some(good.to_string());
      assert_eq!(validate(&req), Ok(()), "{good} should pass");
    }
  }

  #[test]
  fn fleet_size_wire_values_round_trip() {
    for size in ContactFleetSize::ALL {
      let json = serde_json::to_string(&size).unwrap();
      assert_eq!(json, format!("\"{}\"", size.wire()));
      assert_eq!(ContactFleetSize::from_wire(size.wire()), Some(size));
    }
    assert_eq!(ContactFleetSize::from_wire("enormous"), None);
    assert!(serde_json::from_str::<ContactFleetSize>("\"enormous\"").is_err());
  }

  #[test]
  fn subject_carries_who_and_the_fleet_band() {
    let (subject, _) = format_contact_email(&valid_request(), datetime!(2026-08-24 09:30:00 UTC));
    assert_eq!(
      subject,
      "[CONTACT] Dana Okafor at Meterworks (250 to 1,500 devices)"
    );
  }

  #[test]
  fn subject_degrades_without_company_or_fleet_size() {
    let req = ContactRequest {
      company: None,
      fleet_size: None,
      ..valid_request()
    };
    let (subject, _) = format_contact_email(&req, datetime!(2026-08-24 09:30:00 UTC));
    assert_eq!(subject, "[CONTACT] Dana Okafor");
  }

  #[test]
  fn body_carries_every_field_and_the_timestamp() {
    let (_, body) = format_contact_email(&valid_request(), datetime!(2026-08-24 09:30:00 UTC));
    assert!(body.contains("Name:         Dana Okafor"));
    assert!(body.contains("Email:        dana@example.com"));
    assert!(body.contains("Company:      Meterworks"));
    assert!(body.contains("Fleet size:   250 to 1,500 devices"));
    assert!(body.contains("Came from:    fleet"));
    assert!(body.contains("Submitted at: 2026-08-24T09:30:00Z"));
    assert!(body.contains("We have about 900 water meters and need OTA updates."));
  }

  #[test]
  fn absent_optional_fields_render_placeholders() {
    let req = ContactRequest {
      company: None,
      fleet_size: None,
      about: None,
      ..valid_request()
    };
    let (_, body) = format_contact_email(&req, datetime!(2026-08-24 09:30:00 UTC));
    assert!(body.contains("Company:      not provided"));
    assert!(body.contains("Fleet size:   not answered"));
    assert!(body.contains("Came from:    general"));
  }

  #[test]
  fn control_characters_cannot_break_out_of_the_subject() {
    let mut req = valid_request();
    req.name = "Dana\r\nBcc: victim@example.com".to_string();
    req.company = Some("Meter\nworks".to_string());
    let (subject, body) = format_contact_email(&req, datetime!(2026-08-24 09:30:00 UTC));
    assert!(!subject.contains('\n') && !subject.contains('\r'));
    assert!(subject.contains("Dana  Bcc: victim@example.com at Meter works"));
    // The header lines of the body are equally injectable if left raw.
    assert!(body.contains("Name:         Dana  Bcc: victim@example.com"));
    assert!(body.contains("Company:      Meter works"));
  }

  /// The token rides only when the widget issued one. A form without it
  /// must not send a `null` field, because the route reads absence and
  /// null identically and the wire shape should say what it means.
  #[test]
  fn the_turnstile_token_is_omitted_when_absent_and_round_trips_when_present() {
    let without = serde_json::to_string(&valid_request()).unwrap();
    assert!(!without.contains("turnstile_token"));

    let with = ContactRequest {
      turnstile_token: Some("0.token".to_string()),
      ..valid_request()
    };
    let back: ContactRequest =
      serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(back.turnstile_token.as_deref(), Some("0.token"));
    assert_eq!(validate(&back), Ok(()));
  }
}
