//! Marketing-consent wording, and the shape of the record that proves it.
//!
//! Two things have to describe the same event: the words a person reads
//! when they choose to receive marketing email, and the row we keep
//! showing that they chose it. Article 7(1) puts the burden of
//! demonstrating consent on us, so a tick with nothing behind it is not
//! consent we can rely on. Both live here so neither can move without the
//! other being in the diff.
//!
//! The split of responsibilities is the point of the design:
//!
//! - The **trait** (`traits.marketing_emails` in the Kratos identity
//!   schema) is the current state, and the person owns it. They set it at
//!   registration and change it in the settings form.
//! - The **row** (`consent_events` in Postgres, written only by
//!   dovecote's `POST /internal/consent`) is the evidence, and only the
//!   backend writes it. Evidence the subject can edit is not evidence,
//!   which is why the trait is one bare boolean and nothing more. An
//!   object trait would be worse than useless here: Kratos renders every
//!   property of one as its own form input, so an `at`/`source`/
//!   `notice_version` trait would hand the person it is evidence against
//!   a text box for each.
//!
//! Every row is stamped with the crate-root `PRIVACY_NOTICE_VERSION`,
//! server-side and never from the webhook body: Kratos has no idea which
//! notice was on screen, and a version the caller supplies is an
//! assertion rather than a record. That constant lives at the root rather
//! than here because the privacy page renders it too, and the page and
//! the rows must never name different notices.
//!
//! Pure logic, so `cargo test -p capsules` can cover it on a host target
//! while dovecote (a wasm-only `cdylib`) cannot test its own routes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The checkbox label, on both the registration and the settings form.
///
/// This is also the `title` of the trait in
/// `schemas/kratos/identity.user.schema.json`, because that is the string
/// Kratos puts in the form node and `ory_form_builder` renders. The two
/// are checked against each other by
/// `schema_title_matches_label` below, so the schema cannot be reworded
/// without this constant moving with it.
///
/// It names what the messages are about and who sends them, rather than
/// asking agreement to "communications", because consent covers the
/// purpose described and not a broader one.
pub const MARKETING_CONSENT_LABEL: &str = "Email me occasional product updates about PidgeIoT";

/// Shown under the checkbox wherever it appears. Article 7(3) requires
/// withdrawal to be as easy as granting and to be stated *before* consent
/// is given, so this cannot wait for the settings page to say it.
pub const MARKETING_CONSENT_HELPER: &str = "Optional. This has nothing to do with your account, and you can turn it off at any time in your settings.";

/// Shown beside the setting on the settings page, where the withdrawal
/// actually happens. The last clause is Article 7(3)'s "shall not affect
/// the lawfulness of processing based on consent before its withdrawal",
/// said in a way a person can read; the middle one exists because a
/// person turning off marketing must not be left wondering whether they
/// have also turned off their sign-in codes.
pub const MARKETING_CONSENT_WITHDRAWAL: &str = "You can turn this off at any time. Turning it off stops the product updates and nothing else; we will still send you the email your account needs, such as sign-in codes, invitations, alerts you set up, and billing notices. Withdrawing does not affect messages we already sent.";

/// The Kratos form-node name of the consent checkbox, in both flows.
/// fancier keys its helper text off this rather than off a position in
/// the node list, which changes whenever a trait is added.
pub const MARKETING_CONSENT_NODE: &str = "traits.marketing_emails";

/// What a `consent_events` row is about. One purpose exists today, named
/// after the trait it records so the two are obviously the same thing;
/// the column is there so a second one (say, a research panel) is a new
/// value rather than a new table, and so a query for marketing consent
/// never has to mean "every row".
pub const MARKETING_EMAIL_PURPOSE: &str = "marketing_emails";

/// Which direction a consent event went.
///
/// Withdrawal is recorded exactly as granting is: a withdrawal that
/// leaves no row is the same evidence problem as a grant that leaves
/// none, because what has to be producible is the history, not the
/// current state.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConsentKind {
  Granted,
  Withdrawn,
}

impl ConsentKind {
  /// The wire and column spelling. `CHECK (kind IN ('granted',
  /// 'withdrawn'))` in Postgres holds the same two values.
  pub fn as_str(self) -> &'static str {
    match self {
      ConsentKind::Granted => "granted",
      ConsentKind::Withdrawn => "withdrawn",
    }
  }

  /// Reads a stored value back. An unrecognised string is `None` rather
  /// than a default, because guessing which way a row went would
  /// fabricate evidence.
  pub fn parse(value: &str) -> Option<Self> {
    match value {
      "granted" => Some(ConsentKind::Granted),
      "withdrawn" => Some(ConsentKind::Withdrawn),
      _ => None,
    }
  }
}

/// Where a consent event came from. Not free text: each value is a
/// surface whose wording we can produce, which is what makes the row mean
/// something.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConsentSource {
  /// The registration form's checkbox.
  Registration,
  /// The settings form's checkbox, in either direction.
  Settings,
  /// Consent carried in from somewhere else, with its own paper trail.
  /// Nothing writes this today; it exists so that if a list is ever
  /// migrated in, those rows are distinguishable from ones a person
  /// gave us directly.
  Import,
}

impl ConsentSource {
  pub fn as_str(self) -> &'static str {
    match self {
      ConsentSource::Registration => "registration",
      ConsentSource::Settings => "settings",
      ConsentSource::Import => "import",
    }
  }
}

/// Body of `POST /internal/consent`, posted by Kratos's
/// after-registration and after-settings web hooks.
///
/// Deliberately small. `notice_version` is not here: dovecote stamps
/// `PRIVACY_NOTICE_VERSION` itself, since Kratos cannot know which notice
/// was on screen and a caller-supplied version would be an assertion
/// rather than a record. Neither is a timestamp, for the same reason --
/// the row's `at` is the server's.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConsentHookPayload {
  /// The Kratos identity the trait belongs to. Kratos identity ids are
  /// the join key for `flocks.user_id` and every DO's `pigeon_acl`, so
  /// this is the same id the rest of the platform means by "user".
  pub identity_id: Uuid,
  /// The trait's value *after* the flow that fired this hook.
  pub granted: bool,
  pub source: ConsentSource,
  /// The Kratos self-service flow the change happened in, when the hook
  /// context carried one. Optional because it is a cross-reference into
  /// Kratos's own tables, useful when reconstructing a disputed event and
  /// not worth failing a write over.
  #[serde(default)]
  pub flow_id: Option<Uuid>,
}

/// Whether a flow that ended with `granted` should write a row, given the
/// last event already on file for this identity and purpose.
///
/// One rule covers both hooks, which is why neither has a special case:
/// an identity with no event on file has never consented, so the absence
/// reads as `Withdrawn`. Registration with the box unticked therefore
/// records nothing (there is no consent to evidence, and a "withdrawn"
/// row for someone who never granted would be a fiction), registration
/// with it ticked records a grant, and a settings save records only the
/// saves that actually moved the trait.
///
/// Recording every save instead would bury the two events that matter in
/// a pile of rows saying nothing changed, and each of those would carry
/// the notice version in force at the time -- making it look as though
/// consent had been re-given against a notice the person may never have
/// been shown.
pub fn consent_transition(last: Option<ConsentKind>, granted: bool) -> Option<ConsentKind> {
  let now = if granted {
    ConsentKind::Granted
  } else {
    ConsentKind::Withdrawn
  };
  let before = last.unwrap_or(ConsentKind::Withdrawn);
  (now != before).then_some(now)
}

#[cfg(test)]
mod tests {
  use super::*;

  /// The identity schema Kratos actually loads. Compiled in so the test
  /// below reads the shipped file rather than a copy of it.
  const IDENTITY_SCHEMA: &str = include_str!("../../schemas/kratos/identity.user.schema.json");

  fn traits() -> serde_json::Value {
    let schema: serde_json::Value =
      serde_json::from_str(IDENTITY_SCHEMA).expect("identity schema should be valid JSON");
    schema["properties"]["traits"].clone()
  }

  #[test]
  fn schema_title_matches_label() {
    let title = traits()["properties"]["marketing_emails"]["title"]
      .as_str()
      .expect("marketing_emails should declare a title")
      .to_string();
    assert_eq!(
      title, MARKETING_CONSENT_LABEL,
      "the schema's title is the label Kratos renders; it and MARKETING_CONSENT_LABEL are the same string"
    );
  }

  /// One checkbox, and a bare boolean is the only shape that stays one.
  /// Kratos emits a form input per property of an object trait, so
  /// anything nested here becomes another field the subject can edit --
  /// which is why `at`, `source` and `notice_version` are columns in
  /// `consent_events` and not traits.
  #[test]
  fn the_trait_is_one_plain_boolean() {
    let consent = traits()["properties"]["marketing_emails"].clone();
    assert_eq!(consent["type"], "boolean");
    assert!(
      consent.get("properties").is_none(),
      "an object trait renders one form input per property"
    );
  }

  /// `subscribed` was the bare boolean this replaces, and nothing ever
  /// read it. Declaring both would put two subscribe-shaped checkboxes on
  /// the registration form, which is the opposite of the clarity a
  /// consent request needs.
  #[test]
  fn the_traits_it_replaces_are_gone() {
    let props = traits()["properties"].clone();
    assert!(props.get("subscribed").is_none());
    assert!(props.get("marketing_consent").is_none());
  }

  /// A ticked-by-default box is not consent, and the schema achieves an
  /// unticked one by giving the property no default. It must also stay
  /// out of `required`, or refusing marketing would block registration --
  /// which is what Article 7(4) means by consent not being freely given
  /// when it is bundled with something else.
  #[test]
  fn consent_trait_is_neither_defaulted_nor_required() {
    let t = traits();
    assert!(
      t["properties"]["marketing_emails"].get("default").is_none(),
      "a default would render the box pre-ticked"
    );
    let required: Vec<String> = t["required"]
      .as_array()
      .map(|a| {
        a.iter()
          .filter_map(|v| v.as_str().map(str::to_string))
          .collect()
      })
      .unwrap_or_default();
    assert!(
      !required.contains(&"marketing_emails".to_string()),
      "requiring the trait would bundle marketing with account creation"
    );
  }

  #[test]
  fn first_grant_is_recorded() {
    assert_eq!(
      consent_transition(None, true),
      Some(ConsentKind::Granted),
      "registration with the box ticked is the event the whole table exists for"
    );
  }

  #[test]
  fn registration_without_the_box_records_nothing() {
    assert_eq!(consent_transition(None, false), None);
  }

  #[test]
  fn withdrawal_after_a_grant_is_recorded() {
    assert_eq!(
      consent_transition(Some(ConsentKind::Granted), false),
      Some(ConsentKind::Withdrawn)
    );
  }

  #[test]
  fn regranting_after_a_withdrawal_is_recorded() {
    assert_eq!(
      consent_transition(Some(ConsentKind::Withdrawn), true),
      Some(ConsentKind::Granted)
    );
  }

  #[test]
  fn a_settings_save_that_moves_nothing_records_nothing() {
    assert_eq!(consent_transition(Some(ConsentKind::Granted), true), None);
    assert_eq!(
      consent_transition(Some(ConsentKind::Withdrawn), false),
      None
    );
  }

  #[test]
  fn kind_round_trips_through_its_column_spelling() {
    for kind in [ConsentKind::Granted, ConsentKind::Withdrawn] {
      assert_eq!(ConsentKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(ConsentKind::parse("maybe"), None);
  }

  #[test]
  fn hook_payload_accepts_a_body_without_a_flow_id() {
    let payload: ConsentHookPayload = serde_json::from_str(
      r#"{"identity_id":"6c0f1a5e-3f4b-4a1e-9a2d-8b7c6d5e4f30",
          "granted":true,"source":"registration"}"#,
    )
    .expect("a hook body without a flow id should still record the event");
    assert!(payload.flow_id.is_none());
    assert_eq!(payload.source, ConsentSource::Registration);
  }
}
