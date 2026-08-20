use crate::models::AlertVariant;
use ory_kratos_client_wasm::models::ui_node_input_attributes::{
  AutocompleteEnum, OnclickTriggerEnum, OnloadTriggerEnum, TypeEnum as InputTypeEnum,
};
use ory_kratos_client_wasm::models::{UiContainer, UiNodeAttributes, ui_text::TypeEnum};

pub fn extract_ui_messages(ui: &UiContainer) -> Vec<(AlertVariant, String)> {
  let mut alerts = Vec::new();

  // ONLY fetch global form-level message nodes
  if let Some(messages) = &ui.messages {
    for msg in messages {
      let variant = match msg.r#type {
        TypeEnum::Error => AlertVariant::Error,
        TypeEnum::Info => AlertVariant::Info,
        TypeEnum::Success => AlertVariant::Success,
      };
      alerts.push((variant, msg.text.clone()));
    }
  }

  alerts
}

/// The href of the first anchor ("a") node in a flow's UI, if any.
///
/// Kratos v26 with `feature_flags.use_continue_with_transitions: true`
/// renders a completed browser flow (e.g. a `passed_challenge` verification)
/// as a success message plus a single manual "Continue" anchor pointing at
/// the flow's after-completion return URL. A SPA is expected to follow that
/// transition itself rather than make the user click it — see the
/// verification view, which auto-navigates when this returns `Some`.
pub fn continue_anchor_href(ui: &UiContainer) -> Option<String> {
  ui.nodes
    .iter()
    .find_map(|node| match node.attributes.as_ref() {
      UiNodeAttributes::A(anchor) => Some(anchor.href.clone()),
      _ => None,
    })
}

/// The HTML `type` token for a Kratos input node.
///
/// The variant's Rust name is not the HTML token: `DatetimeLocal` is
/// `datetime-local`, and lowercasing the Debug name yields `datetimelocal`,
/// which no browser recognises — the field falls back to a plain text box
/// and loses its native picker. Only that one variant differs today, which
/// is exactly why a Debug-derived token survived unnoticed.
///
/// Matching every variant by hand, rather than round-tripping through
/// serde, is deliberate: a crate upgrade that adds a variant then fails to
/// compile here instead of quietly emitting a token that looks plausible.
/// The tests below pin each arm to the model's own serde rename so the two
/// cannot drift.
pub fn input_type_token(t: InputTypeEnum) -> &'static str {
  match t {
    InputTypeEnum::Text => "text",
    InputTypeEnum::Password => "password",
    InputTypeEnum::Number => "number",
    InputTypeEnum::Checkbox => "checkbox",
    InputTypeEnum::Hidden => "hidden",
    InputTypeEnum::Email => "email",
    InputTypeEnum::Tel => "tel",
    InputTypeEnum::Submit => "submit",
    InputTypeEnum::Button => "button",
    InputTypeEnum::DatetimeLocal => "datetime-local",
    InputTypeEnum::Date => "date",
    InputTypeEnum::Url => "url",
  }
}

/// The WHATWG `autocomplete` token for a Kratos input node.
///
/// These are the tokens password managers and OTP autofill match on, and
/// most of them are hyphenated or spaced: a lowercased Debug name gives
/// `currentpassword`/`newpassword`/`onetimecode`/`usernamewebauthn`, none of
/// which are valid, so the browser treats the field as unannotated. Kratos
/// v26.2 emits `current-password` on the login form and `new-password` on
/// the settings password form, so this is what decides whether a saved
/// password is offered at sign-in and whether a changed one is captured.
/// `username webauthn` is what passkey conditional UI keys on.
///
/// Hand-matched for the same reason as `input_type_token` above.
pub fn autocomplete_token(a: AutocompleteEnum) -> &'static str {
  match a {
    AutocompleteEnum::Email => "email",
    AutocompleteEnum::Tel => "tel",
    AutocompleteEnum::Url => "url",
    AutocompleteEnum::CurrentPassword => "current-password",
    AutocompleteEnum::NewPassword => "new-password",
    AutocompleteEnum::OneTimeCode => "one-time-code",
    AutocompleteEnum::UsernameWebauthn => "username webauthn",
  }
}

/// The name of the Ory-provided WebAuthn global a trigger button invokes.
///
/// Kratos's webauthn.js -- delivered to the page as a script UI node --
/// defines these functions on `window`; a node's trigger attribute names
/// which one to call. The serde rename is exactly the JavaScript function
/// name, so the tests below pin each arm to it the same way the attribute
/// tokens above are pinned.
pub fn onclick_trigger_fn(t: OnclickTriggerEnum) -> &'static str {
  match t {
    OnclickTriggerEnum::OryWebAuthnRegistration => "oryWebAuthnRegistration",
    OnclickTriggerEnum::OryWebAuthnLogin => "oryWebAuthnLogin",
    OnclickTriggerEnum::OryPasskeyLogin => "oryPasskeyLogin",
    OnclickTriggerEnum::OryPasskeyLoginAutocompleteInit => "oryPasskeyLoginAutocompleteInit",
    OnclickTriggerEnum::OryPasskeyRegistration => "oryPasskeyRegistration",
    OnclickTriggerEnum::OryPasskeySettingsRegistration => "oryPasskeySettingsRegistration",
  }
}

/// `onclick_trigger_fn`'s counterpart for onload triggers -- the same six
/// Ory globals, arriving as a distinct generated enum type.
pub fn onload_trigger_fn(t: OnloadTriggerEnum) -> &'static str {
  match t {
    OnloadTriggerEnum::OryWebAuthnRegistration => "oryWebAuthnRegistration",
    OnloadTriggerEnum::OryWebAuthnLogin => "oryWebAuthnLogin",
    OnloadTriggerEnum::OryPasskeyLogin => "oryPasskeyLogin",
    OnloadTriggerEnum::OryPasskeyLoginAutocompleteInit => "oryPasskeyLoginAutocompleteInit",
    OnloadTriggerEnum::OryPasskeyRegistration => "oryPasskeyRegistration",
    OnloadTriggerEnum::OryPasskeySettingsRegistration => "oryPasskeySettingsRegistration",
  }
}

#[cfg(test)]
mod attribute_tokens {
  use super::{
    AutocompleteEnum, InputTypeEnum, OnclickTriggerEnum, OnloadTriggerEnum, autocomplete_token,
    input_type_token, onclick_trigger_fn, onload_trigger_fn,
  };

  // The model carries the HTML token as its serde rename, so serializing a
  // variant is the authoritative answer for what the attribute must say.
  // Listing the variants by hand here is the point: the day the crate grows
  // one, both this list and the mapping have to be updated together.
  #[test]
  fn input_types_match_the_models_serde_names() {
    for t in [
      InputTypeEnum::Text,
      InputTypeEnum::Password,
      InputTypeEnum::Number,
      InputTypeEnum::Checkbox,
      InputTypeEnum::Hidden,
      InputTypeEnum::Email,
      InputTypeEnum::Tel,
      InputTypeEnum::Submit,
      InputTypeEnum::Button,
      InputTypeEnum::DatetimeLocal,
      InputTypeEnum::Date,
      InputTypeEnum::Url,
    ] {
      let wire = serde_json::to_value(t).unwrap();
      assert_eq!(input_type_token(t), wire.as_str().unwrap(), "{t:?}");
    }
  }

  #[test]
  fn autocompletes_match_the_models_serde_names() {
    for a in [
      AutocompleteEnum::Email,
      AutocompleteEnum::Tel,
      AutocompleteEnum::Url,
      AutocompleteEnum::CurrentPassword,
      AutocompleteEnum::NewPassword,
      AutocompleteEnum::OneTimeCode,
      AutocompleteEnum::UsernameWebauthn,
    ] {
      let wire = serde_json::to_value(a).unwrap();
      assert_eq!(autocomplete_token(a), wire.as_str().unwrap(), "{a:?}");
    }
  }

  #[test]
  fn onclick_triggers_match_the_models_serde_names() {
    for t in [
      OnclickTriggerEnum::OryWebAuthnRegistration,
      OnclickTriggerEnum::OryWebAuthnLogin,
      OnclickTriggerEnum::OryPasskeyLogin,
      OnclickTriggerEnum::OryPasskeyLoginAutocompleteInit,
      OnclickTriggerEnum::OryPasskeyRegistration,
      OnclickTriggerEnum::OryPasskeySettingsRegistration,
    ] {
      let wire = serde_json::to_value(t).unwrap();
      assert_eq!(onclick_trigger_fn(t), wire.as_str().unwrap(), "{t:?}");
    }
  }

  #[test]
  fn onload_triggers_match_the_models_serde_names() {
    for t in [
      OnloadTriggerEnum::OryWebAuthnRegistration,
      OnloadTriggerEnum::OryWebAuthnLogin,
      OnloadTriggerEnum::OryPasskeyLogin,
      OnloadTriggerEnum::OryPasskeyLoginAutocompleteInit,
      OnloadTriggerEnum::OryPasskeyRegistration,
      OnloadTriggerEnum::OryPasskeySettingsRegistration,
    ] {
      let wire = serde_json::to_value(t).unwrap();
      assert_eq!(onload_trigger_fn(t), wire.as_str().unwrap(), "{t:?}");
    }
  }

  // The regression itself: the tokens a lowercased Debug name would have
  // produced are the ones browsers ignore.
  #[test]
  fn hyphenated_tokens_are_not_debug_lowercased() {
    assert_eq!(
      autocomplete_token(AutocompleteEnum::CurrentPassword),
      "current-password"
    );
    assert_eq!(
      autocomplete_token(AutocompleteEnum::OneTimeCode),
      "one-time-code"
    );
    assert_eq!(
      input_type_token(InputTypeEnum::DatetimeLocal),
      "datetime-local"
    );
  }
}
