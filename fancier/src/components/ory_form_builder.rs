use crate::{
  components::Alert,
  helpers::{
    autocomplete_token, input_type_token, invoke_webauthn_trigger, onclick_trigger_fn,
    onload_trigger_fn, parse_json_bool, parse_json_string,
  },
  models::AlertVariant,
};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdCopy;
use ory_kratos_client_wasm::models::UiNodeAttributes::{A, Div, Img, Input, Script, Text};
use std::collections::BTreeMap;
#[cfg(feature = "web")]
use wasm_bindgen_futures::JsFuture;

/// The leading `+` is mandatory because the server insists on it: Kratos parses
/// a `format: tel` trait with no default region, so a number carrying no country
/// code is refused there however permissive the browser was.
const TEL_REGEX: &str = "\\+(9[976]\\d|8[987530]\\d|6[987]\\d|5[90]\\d|42\\d|3[875]\\d|2[98654321]\\d|9[8543210]|8[6421]|6[6543210]|5[87654321]|4[987654310]|3[9643210]|2[70]|7|1)\\d{1,14}";

/// Text the browser appends to its own "match the requested format" bubble.
/// Without a `title` that bubble names no format at all.
const TEL_TITLE: &str = "Start with + and the country code, like +18605551234";

/// Drop the punctuation people group digits with, keeping everything else.
///
/// Phone numbers get written `860.777.5695`, `(860) 777-5695` or with plain
/// spaces, and `TEL_REGEX` accepts none of it. Stripping the separators as
/// the field is typed keeps that regex as the acceptance bar instead of
/// widening it. Anything that is not a separator survives, so a real typo
/// still fails validation rather than being silently reshaped.
fn strip_separators(raw: &str) -> String {
  let mut cleaned = String::with_capacity(raw.len());
  for c in raw.chars() {
    if !(c.is_whitespace() || matches!(c, '.' | '-' | '(' | ')')) {
      cleaned.push(c);
    }
  }
  cleaned
}

// --- Input Node Components ---

#[component]
fn InputFieldNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  messages: Vec<ory_kratos_client_wasm::models::UiText>,
  validate: bool,
  pattern: Option<String>,
  hint: Option<Element>,
  /// Appended to the browser's native validation bubble.
  title: Option<String>,
  /// Show the hint from the start rather than only once the value is bad.
  #[props(default)]
  persistent_hint: bool,
  /// Strip grouping punctuation as the field is typed. This makes the field
  /// controlled, which is the point: the browser must validate and post the
  /// cleaned value, not what was keyed.
  #[props(default)]
  strip_separators_on_input: bool,
  id_suffix: String,
) -> Element {
  let input_id = format!("{}_{}", attrs.name, id_suffix);
  let label_text = meta
    .as_ref()
    .map(|m| m.text.clone())
    .unwrap_or_else(|| format!("{:?}", attrs.r#type));
  let initial = parse_json_string(&attrs.value);
  let mut typed = use_signal(|| initial.clone());

  rsx! {
    // `floating-label` is a flex row, so a hint nested in it lands beside a
    // full-width input instead of under it -- which is why the hints were
    // never seen. It sits outside the label, and the label carries
    // `validator` too so daisyUI's `:has(:user-invalid) ~ .validator-hint`
    // still reveals it; the input keeps its own for the error border.
    div { class: "my-4",
      label { class: "floating-label", class: if validate { "validator" }, r#for: "{input_id}",
        span { "{label_text}" }
        input {
          id: "{input_id}",
          name: attrs.name,
          class: "input w-full",
          class: if validate { "validator" },
          required: attrs.required.unwrap_or_default(),
          disabled: attrs.disabled,
          autocomplete: attrs.autocomplete.map(autocomplete_token),
          placeholder: label_text,
          title,
          r#type: input_type_token(attrs.r#type),
          pattern,
          value: if strip_separators_on_input { typed() } else { initial.clone() },
          oninput: move |event| {
            if strip_separators_on_input {
              typed.set(strip_separators(&event.value()));
            }
          },
        }
      }
      if validate {
        div {
          class: "validator-hint",
          class: if persistent_hint { "visible" } else { "hidden" },
          {hint}
        }
      }
    }
    // Render field-specific field validation errors neatly below the input element
    if !messages.is_empty() {
      div { class: "flex flex-col gap-1 mt-1 mb-2",
        for message in messages {
          Alert {
            variant: match message.r#type {
                ory_kratos_client_wasm::models::ui_text::TypeEnum::Error => AlertVariant::Error,
                ory_kratos_client_wasm::models::ui_text::TypeEnum::Info => AlertVariant::Info,
                ory_kratos_client_wasm::models::ui_text::TypeEnum::Success => {
                    AlertVariant::Success
                }
            },
            persistent: true, // Keep it attached to the input field natively
            "{message.text}"
          }
        }
      }
    }
  }
}

#[component]
fn InputButtonNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  id_suffix: String,
) -> Element {
  let input_id = format!("{}_{}", attrs.name, id_suffix);
  // Kratos ships its WebAuthn/passkey trigger buttons as type="button" and
  // expects the page to invoke the named Ory global on click: that function
  // runs the browser ceremony, writes the credential into the form's hidden
  // input, and submits the form itself. Without the call such a button does
  // nothing at all. Plain submit buttons carry no trigger and keep native
  // submission; the deprecated stringly `onclick` field is ignored because
  // honoring it would mean eval().
  let trigger = attrs.onclick_trigger;

  rsx! {
    button {
      id: "{input_id}",
      disabled: attrs.disabled,
      class: "btn btn-primary w-full my-4",
      name: attrs.name,
      r#type: input_type_token(attrs.r#type),
      value: parse_json_string(&attrs.value),
      onclick: move |_| async move {
        if let Some(trigger) = trigger {
          invoke_webauthn_trigger(onclick_trigger_fn(trigger)).await;
        }
      },
      if let Some(ref label) = meta {
        {label.text.to_string()}
      }
    }
  }
}

#[component]
fn OnloadTriggerNode(
  trigger: ory_kratos_client_wasm::models::ui_node_input_attributes::OnloadTriggerEnum,
) -> Element {
  // Kratos marks a node with an onload trigger when the page should start a
  // WebAuthn action on arrival -- the passkey-autofill (conditional UI)
  // initializer on the login flow. There is no DOM to render; mounting is
  // the load event.
  use_future(move || async move {
    invoke_webauthn_trigger(onload_trigger_fn(trigger)).await;
  });

  rsx! {}
}

#[component]
fn InputOtherNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  id_suffix: String,
) -> Element {
  let input_id = format!("{}_{}", attrs.name, id_suffix);

  rsx! {
    if let Some(ref label) = meta {
      label { class: "w-full", r#for: "{input_id}",
        {label.text.to_owned()}
        input {
          id: "{input_id}",
          disabled: attrs.disabled,
          class: "input w-full",
          name: attrs.name,
          required: attrs.required.unwrap_or_default(),
          r#type: input_type_token(attrs.r#type),
          value: parse_json_string(&attrs.value),
        }
      }
    } else {
      input {
        id: "{input_id}",
        disabled: attrs.disabled,
        class: "input w-full",
        name: attrs.name,
        required: attrs.required.unwrap_or_default(),
        r#type: input_type_token(attrs.r#type),
        value: parse_json_string(&attrs.value),
      }
    }
  }
}

/// Explanatory text shown under a checkbox, keyed by Kratos node name.
///
/// Kratos's form node carries one label string and nowhere to put a second
/// line, but the marketing-consent box needs one: Article 7(3) requires
/// the withdrawal right to be stated *before* consent is given, not only
/// on the page where the withdrawal happens. Keyed by node name rather
/// than by position, which changes whenever a trait is added.
///
/// The settings flow says more than the registration flow does, because
/// that is where someone reading it is deciding whether to turn it off,
/// and the thing they need to know is what stays switched on.
fn checkbox_helper(name: &str, in_settings: bool) -> Option<&'static str> {
  (name == capsules::MARKETING_CONSENT_NODE).then(|| {
    if in_settings {
      capsules::MARKETING_CONSENT_WITHDRAWAL
    } else {
      capsules::MARKETING_CONSENT_HELPER
    }
  })
}

/// Trait nodes Kratos still renders that the forms should not show.
///
/// A trait removed from the identity schema does not stop appearing:
/// Kratos builds the profile form from the schema *and* the identity's
/// stored traits, so an identity whose traits still carry the retired key
/// gets a node for it -- as a checkbox with the stored value and, having
/// no schema entry to take a title from, no label at all, which
/// `InputCheckBoxNode` would render as a ticked box labelled
/// `traits.subscribed`. Confirmed against a real settings flow.
///
/// Hiding it is also what clears it. The form then posts every trait
/// except that one, and a profile save writes the submitted object
/// wholesale, so the stale key drops out of storage on the person's next
/// save -- verified end to end on the dev stack. Once nothing carries
/// either key the whole rule can go; docs/consent.md has the query.
fn is_retired_node(name: &str) -> bool {
  // The bare boolean `marketing_emails` replaces, which nothing ever read.
  name == "traits.subscribed"
    // Briefly on main as an object trait before it was flattened. No
    // production identity ever saw it, so this line is for dev accounts
    // created in that window and can go sooner than the one above.
    || name == "traits.marketing_consent.granted"
}

#[component]
fn InputCheckBoxNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  id_suffix: String,
  helper: Option<&'static str>,
) -> Element {
  let input_id = format!("{}_{}", attrs.name, id_suffix);
  let label_text = meta.map(|m| m.text).unwrap_or_else(|| attrs.name.clone());

  let parsed_val = parse_json_string(&attrs.value);
  let node_value = if parsed_val.is_empty() {
    "true".to_string()
  } else {
    parsed_val
  };

  rsx! {
    label { class: "w-full", r#for: "{input_id}",
      input {
        id: "{input_id}",
        disabled: attrs.disabled,
        class: "checkbox",
        name: attrs.name,
        required: attrs.required.unwrap_or_default(),
        r#type: input_type_token(attrs.r#type),
        checked: parse_json_bool(&attrs.value),
        value: node_value,
      }
      span { class: "ml-4", "{label_text}" }
    }
    if let Some(helper) = helper {
      // Outside the label, so a click on the explanation does not toggle
      // the box it is explaining.
      p { class: "text-sm text-base-content/60 mt-1 mb-2", "{helper}" }
    }
  }
}

// --- Static Media / Structural Nodes ---

#[component]
fn ImageNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeImageAttributes,
) -> Element {
  // The only image Kratos sends is the TOTP enrolment QR code, which is
  // something the user points a phone at: centred, and never wider than the
  // form it sits in.
  rsx! {
    div { class: "flex flex-col items-center gap-2 my-4",
      if let Some(ref label) = meta {
        span { id: label.id, class: "text-sm text-base-content/80", {label.text.clone()} }
        img {
          class: "max-w-full h-auto rounded-box border border-base-content/10 bg-base-100 p-2",
          height: attrs.height,
          id: attrs.id,
          src: attrs.src,
          width: attrs.width,
          alt: label.text.to_owned(),
        }
      } else {
        img {
          class: "max-w-full h-auto rounded-box border border-base-content/10 bg-base-100 p-2",
          height: attrs.height,
          id: attrs.id,
          src: attrs.src,
          width: attrs.width,
        }
      }
    }
  }
}

#[component]
fn TextNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeTextAttributes,
) -> Element {
  // Kratos sends the TOTP secret as an ordinary text node, so by default it
  // renders as a run of prose the user is expected to transcribe by eye.
  // Give that one node the treatment every other credential in the app gets:
  // monospace, selectable, and copyable.
  if attrs.id == TOTP_SECRET_NODE_ID {
    return rsx! {
      SecretTextNode { meta, attrs }
    };
  }

  rsx! {
    if let Some(ref label) = meta {
      label { r#for: attrs.id.clone(), id: label.id, class: "text-lg",
        {label.text.to_owned()}
      }
    }
    p { id: attrs.id, class: "", {attrs.text.text} }
  }
}

/// Kratos's id for the text node carrying the TOTP secret in plain text.
const TOTP_SECRET_NODE_ID: &str = "totp_secret_key";

#[component]
fn SecretTextNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeTextAttributes,
) -> Element {
  let copied = use_signal(|| false);
  let copy_failed = use_signal(|| false);
  #[cfg(feature = "web")]
  let secret = attrs.text.text.clone();

  rsx! {
    div { class: "flex flex-col gap-2 my-4",
      if let Some(ref label) = meta {
        span { id: label.id, class: "text-sm text-base-content/80", {label.text.to_owned()} }
      }
      div { class: "flex items-center gap-3 rounded-box border border-base-content/10 bg-base-200 p-3",
        code {
          id: attrs.id,
          class: "font-mono text-sm break-all grow select-all tracking-wider",
          {attrs.text.text}
        }
        button {
          // The enclosing element is a Kratos <form>: without this a click
          // would submit it instead of copying.
          r#type: "button",
          class: "btn btn-square btn-ghost btn-sm shrink-0",
          "aria-label": "Copy secret",
          onclick: move |_| {
              #[cfg(feature = "web")]
              let secret = secret.clone();
              async move {
                  #[cfg(feature = "web")]
                  if let Some(window) = web_sys::window() {
                      let mut copied = copied;
                      let mut copy_failed = copy_failed;
                      let result = JsFuture::from(window.navigator().clipboard().write_text(&secret))
                          .await;
                      copied.set(result.is_ok());
                      copy_failed.set(result.is_err());
                      if result.is_ok() {
                          crate::helpers::sleep_ms(2000).await;
                          copied.set(false);
                      }
                  }
              }
          },
          if copied() {
            span { class: "text-success text-xs", "Copied!" }
          } else if copy_failed() {
            span { class: "text-error text-xs", "Copy failed" }
          } else {
            Icon { icon: LdCopy }
          }
        }
      }
    }
  }
}

#[component]
fn LinkNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeAnchorAttributes,
) -> Element {
  rsx! {
    if let Some(ref label) = meta {
      label { r#for: attrs.id.clone(), id: label.id, class: "text-lg",
        {label.text.to_owned()}
      }
    }
    a {
      id: attrs.id,
      class: "link-primary link-hover",
      href: attrs.href,
      {attrs.title.text}
    }
  }
}

#[component]
fn DivNode(attrs: ory_kratos_client_wasm::models::UiNodeDivisionAttributes) -> Element {
  rsx! {
    div { id: attrs.id,
      if let Some(class) = attrs.class {
        "class: {class}"
      }
      if let Some(data) = attrs.data {
        for (key , value) in data {
          "data-{key}: {value}"
        }
      }
    }
  }
}

#[component]
fn ScriptNode(attrs: ory_kratos_client_wasm::models::UiNodeScriptAttributes) -> Element {
  // Dioxus mounts rsx by cloning <template> contents, and a script element
  // parsed into a template carries the parser's "already started" flag, so
  // a declaratively rendered script tag lands in the DOM but never executes.
  // Kratos's webauthn.js has to actually run for the passkey/WebAuthn
  // trigger buttons to have anything to call, so the element is created and
  // inserted imperatively instead.
  use_effect(move || {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
      return;
    };
    // Flow refetches remount this component, but the script element itself
    // outlives it in <head>; inserting again would double-execute it.
    if document.get_element_by_id(&attrs.id).is_some() {
      return;
    }
    let Ok(element) = document.create_element("script") else {
      return;
    };
    let _ = element.set_attribute("id", &attrs.id);
    let _ = element.set_attribute("src", &attrs.src);
    let _ = element.set_attribute("type", &attrs.r#type);
    if attrs.r#async {
      let _ = element.set_attribute("async", "");
    }
    // The model types these as bare strings where empty means unset, and an
    // empty-string crossorigin attribute would mean anonymous CORS mode
    // rather than "no attribute" -- map empty to absent.
    for (name, value) in [
      ("crossorigin", &attrs.crossorigin),
      ("integrity", &attrs.integrity),
      ("nonce", &attrs.nonce),
      ("referrerpolicy", &attrs.referrerpolicy),
    ] {
      if !value.is_empty() {
        let _ = element.set_attribute(name, value);
      }
    }
    if let Some(head) = document.head() {
      let _ = head.append_child(&element);
    }
  });

  rsx! {}
}

#[component]
fn MessageNode(message: ory_kratos_client_wasm::models::UiText) -> Element {
  rsx! {
    div {
      id: message.id,
      role: "alert",
      class: match message.r#type {
          ory_kratos_client_wasm::models::ui_text::TypeEnum::Error => "alert alert-error",
          ory_kratos_client_wasm::models::ui_text::TypeEnum::Info => "alert alert-info",
          ory_kratos_client_wasm::models::ui_text::TypeEnum::Success => {
              "alert alert-success"
          }
      },
      span { {message.text} }
    }
  }
}

// --- Node Router ---

#[component]
fn NodeBuilder(
  nodes: Vec<ory_kratos_client_wasm::models::UiNode>,
  id_suffix: String,
  // Only the settings flow passes true; it is what picks the withdrawal
  // wording over the shorter registration helper.
  #[props(default)] in_settings: bool,
) -> Element {
  rsx! {
    for node in nodes.into_iter().filter(|n| !matches!(n.attributes.as_ref(), Input(i) if is_retired_node(&i.name))) {
      match *node.attributes {
          Input(i) => {
              // A node of any input type can carry an onload trigger; run it
              // alongside whatever element the node renders as.
              let onload_trigger = i.onload_trigger;
              let body = match i.r#type {
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Text => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
                          messages: node.messages,
                          validate: false,
                          id_suffix: id_suffix.clone(),
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Password => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
                          messages: node.messages,
                          validate: true,
                          hint: rsx! {
                            p { "Password must be more than 8 characters, and include:" }
                            ul { class: "list-disc list-inside",
                              li { "At least one number" }
                              li { "At least one lowercase letter" }
                              li { "At least one uppercase letter" }
                            }
                          },
                          pattern: "(?=.*\\d)(?=.*[a-z])(?=.*[A-Z]).{{8,}}",
                          id_suffix: id_suffix.clone(),
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Email => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
                          messages: node.messages,
                          validate: true,
                          hint: rsx! {
                            p { "Please enter a valid email address" }
                          },
                          id_suffix: id_suffix.clone(),
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Tel => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
                          messages: node.messages,
                          validate: true,
                          // The format is not guessable, so say it before the
                          // first keystroke rather than after a rejection.
                          persistent_hint: true,
                          strip_separators_on_input: true,
                          hint: rsx! {
                            p { "Optional. Start with + and the country code." }
                            p { "Spaces, dots, dashes and parentheses are fine: +1 860 555-1234." }
                          },
                          title: TEL_TITLE,
                          pattern: TEL_REGEX,
                          id_suffix: id_suffix.clone(),
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Number
                  | ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::DatetimeLocal
                  | ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Date
                  | ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Url => {
                      rsx! {
                        InputOtherNode { meta: node.meta.label, attrs: *i, id_suffix: id_suffix.clone() }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Checkbox => {
                      let helper = checkbox_helper(&i.name, in_settings);
                      rsx! {
                        InputCheckBoxNode {
                          meta: node.meta.label,
                          attrs: *i,
                          id_suffix: id_suffix.clone(),
                          helper,
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Hidden => {
                      rsx! {
                        input {
                          id: format!("{}_{}", i.name, id_suffix),
                          autocomplete: i.autocomplete.map(autocomplete_token),
                          disabled: i.disabled,
                          name: i.name,
                          r#type: input_type_token(i.r#type),
                          value: parse_json_string(&i.value),
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Submit
                  | ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Button => {
                      rsx! {
                        InputButtonNode { meta: node.meta.label, attrs: *i, id_suffix: id_suffix.clone() }
                      }
                  }
              };
              rsx! {
                if let Some(trigger) = onload_trigger {
                  OnloadTriggerNode { trigger }
                }
                {body}
              }
          }
          Text(text) => rsx! {
            TextNode { meta: node.meta.label, attrs: *text }
          },
          Img(img) => rsx! {
            ImageNode { meta: node.meta.label, attrs: *img }
          },
          A(link) => rsx! {
            LinkNode { meta: node.meta.label, attrs: *link }
          },
          Div(div) => rsx! {
            DivNode { attrs: *div }
          },
          Script(script) => rsx! {
            ScriptNode { attrs: *script }
          },
      }
    }
  }
}

// --- Main Builder Component ---

/// Human title for a node group, used as the heading of the form built from
/// it. `LookupSecret` is Kratos's name for backup codes, not for the account
/// recovery flow -- calling it "Recovery" on the settings page put two
/// unrelated things under one word.
fn group_title(group: ory_kratos_client_wasm::models::ui_node::GroupEnum) -> &'static str {
  use ory_kratos_client_wasm::models::ui_node::GroupEnum;

  match group {
    GroupEnum::Password => "Password",
    GroupEnum::Oidc => "OIDC",
    GroupEnum::Profile => "Profile",
    GroupEnum::Code => "Code",
    GroupEnum::Totp => "Authenticator app",
    GroupEnum::LookupSecret => "Backup recovery codes",
    GroupEnum::Webauthn => "Web Authentication",
    GroupEnum::Passkey => "Passkey",
    GroupEnum::Captcha => "Captcha",
    GroupEnum::Saml => "SAML",
    _ => "",
  }
}

/// Slug for the section id a group's form is wrapped in. Kebab-case, and
/// stable per group rather than derived from the title, so rewording a
/// heading cannot silently break a link into the page.
fn group_slug(group: ory_kratos_client_wasm::models::ui_node::GroupEnum) -> &'static str {
  use ory_kratos_client_wasm::models::ui_node::GroupEnum;

  match group {
    GroupEnum::Password => "password",
    GroupEnum::Oidc => "oidc",
    GroupEnum::Profile => "profile",
    GroupEnum::Code => "code",
    GroupEnum::Totp => "totp",
    GroupEnum::LookupSecret => "backup-codes",
    GroupEnum::Webauthn => "webauthn",
    GroupEnum::Passkey => "passkey",
    GroupEnum::Captcha => "captcha",
    GroupEnum::Saml => "saml",
    _ => "other",
  }
}

#[component]
pub fn FormBuilder(
  ui: ory_kratos_client_wasm::models::UiContainer,
  // When set, each method gets its own titled `section` (id
  // `<prefix>-<group>`) instead of one anonymous stack of fieldsets. Only
  // the settings flow passes it: it is the one flow where several unrelated
  // methods -- profile, password, authenticator app, backup codes -- are on
  // screen at once and need telling apart. The single-purpose flows
  // (login, registration, recovery, verification) already have the page's
  // own heading for that.
  #[props(default)] section_prefix: Option<String>,
) -> Element {
  // Settings is the one flow that passes a section prefix (see the prop's
  // own comment), so it doubles as "this is the page where a withdrawal
  // happens" -- which is the wording the consent checkbox needs there.
  // Reusing it beats a second prop that would have to be kept in step.
  let in_settings = section_prefix.is_some();

  // 1. O(N) Stable Partition: Separate CSRF/Default nodes from Flow nodes
  let (default_nodes, flow_nodes): (Vec<_>, Vec<_>) = ui
    .nodes
    .into_iter()
    .partition(|n| n.group == ory_kratos_client_wasm::models::ui_node::GroupEnum::Default);

  if default_nodes.is_empty() {
    error!("Returned schema missing 'Default' group. CSRF protection compromised.");
    return rsx! {};
  }

  // Scripts are page side-effects, not form fields: hoist them out of the
  // group bucketing so they mount once rather than per method form, and so
  // a method whose only node is its script (Kratos puts webauthn.js in the
  // webauthn group even when only passkeys are enabled) does not render as
  // a titled form with nothing in it.
  let (script_nodes, flow_nodes): (Vec<_>, Vec<_>) = flow_nodes
    .into_iter()
    .partition(|n| matches!(n.attributes.as_ref(), Script(_)));
  let script_attrs: Vec<_> = script_nodes
    .into_iter()
    .filter_map(|n| match *n.attributes {
      Script(script) => Some(*script),
      _ => None,
    })
    .collect();

  // 2. Safely bucket remaining nodes by group to prevent interleaving crashes
  let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
  for node in flow_nodes {
    groups.entry(node.group).or_default().push(node);
  }

  rsx! {
    for attrs in script_attrs {
      ScriptNode { attrs }
    }
    if groups.is_empty() {
      form { action: ui.action.clone(), method: ui.method.clone(),
        div { class: "my-2",
          fieldset { class: "fieldset bg-base-100 border border-base-300 rounded-box p-4",
            NodeBuilder {
              nodes: default_nodes,
              id_suffix: "default".to_string(),
              in_settings,
            }
          }
        }
      }
    } else if let Some(prefix) = section_prefix {
      for (group_enum , group_nodes) in groups {
        section { id: "{prefix}-{group_slug(group_enum)}", class: "mb-10",
          h2 { class: "text-lg font-semibold mb-3", {group_title(group_enum)} }
          form {
            class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-4",
            action: ui.action.clone(),
            method: ui.method.clone(),
            // Namespace the IDs with the specific flow name to prevent collisions
            // if Kratos demands multiple forms (e.g., Password and Webauthn)
            NodeBuilder {
              nodes: default_nodes.clone(),
              id_suffix: format!("{group_enum:?}").to_lowercase(),
              in_settings,
            }
            NodeBuilder {
              nodes: group_nodes,
              id_suffix: format!("{group_enum:?}").to_lowercase(),
              in_settings,
            }
          }
        }
      }
    } else {
      for (group_enum , group_nodes) in groups {
        form { action: ui.action.clone(), method: ui.method.clone(),
          div { class: "my-2",
            fieldset { class: "fieldset bg-base-100 border border-base-300 rounded-box p-4",
              legend { class: "fieldset-legend text-xl", {group_title(group_enum)} }
              // Namespace the IDs with the specific flow name to prevent collisions
              // if Kratos demands multiple forms (e.g., Password and Webauthn)
              NodeBuilder {
                nodes: default_nodes.clone(),
                id_suffix: format!("{group_enum:?}").to_lowercase(),
                in_settings,
              }
              NodeBuilder {
                nodes: group_nodes,
                id_suffix: format!("{group_enum:?}").to_lowercase(),
                in_settings,
              }
            }
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{TEL_REGEX, checkbox_helper, is_retired_node, strip_separators};

  /// The number that was refused at registration, in the shapes people
  /// actually type it, all landing on the same digits.
  #[test]
  fn every_way_of_writing_one_number_strips_to_the_same_digits() {
    for written in [
      "860.777.5695",
      "860-777-5695",
      "860 777 5695",
      "(860) 777-5695",
      "(860).777 5695",
      "8607775695",
    ] {
      assert_eq!(strip_separators(written), "8607775695", "{written}");
    }
  }

  #[test]
  fn the_country_code_survives_and_nothing_is_invented() {
    assert_eq!(strip_separators("+1 (860) 777-5695"), "+18607775695");
    assert_eq!(strip_separators(""), "");
    assert_eq!(strip_separators("   "), "");
    // Not a separator, so it reaches the pattern and is rejected there
    // rather than being quietly deleted into a different number.
    assert_eq!(strip_separators("860x7775695"), "860x7775695");
  }

  /// The stripped value has to clear the pattern the input enforces, or
  /// normalising only moves where the rejection happens. The bare national
  /// number is the case that matters: the browser used to wave it through and
  /// Kratos, which parses `format: tel` with no default region, refused it.
  #[test]
  fn the_pattern_accepts_what_the_server_accepts() {
    let anchored = String::from("^(?:") + TEL_REGEX + ")$";
    let re = regex::Regex::new(&anchored).expect("TEL_REGEX is a valid pattern");
    for accepted in ["+1 (860) 555-1234", "+1.860.555.1234", "+44 20 7946 0958"] {
      assert!(re.is_match(&strip_separators(accepted)), "{accepted}");
    }
    for refused in ["860.555.1234", "020 7946 0958", "+1 860 555 12x4"] {
      assert!(!re.is_match(&strip_separators(refused)), "{refused}");
    }
  }

  #[test]
  fn the_consent_box_explains_itself_differently_in_each_flow() {
    let registration = checkbox_helper(capsules::MARKETING_CONSENT_NODE, false)
      .expect("the consent box carries a helper line wherever it appears");
    let settings = checkbox_helper(capsules::MARKETING_CONSENT_NODE, true)
      .expect("the consent box carries a helper line wherever it appears");
    assert_eq!(registration, capsules::MARKETING_CONSENT_HELPER);
    assert_eq!(settings, capsules::MARKETING_CONSENT_WITHDRAWAL);
    // Article 7(3) wants the withdrawal right stated before consent is
    // given, not only where the withdrawal happens.
    assert!(registration.contains("turn it off"));
  }

  #[test]
  fn no_other_checkbox_grows_a_helper_line() {
    assert!(checkbox_helper("traits.subscribed", false).is_none());
    assert!(checkbox_helper("traits.subscribed", true).is_none());
    assert!(checkbox_helper("remember_me", false).is_none());
  }

  /// The hide-rule clears the traits it names, because a field the form
  /// does not post is a field the next profile save drops. That makes a
  /// filter which caught a real field actively destructive, not merely
  /// cosmetic.
  #[test]
  fn only_the_retired_traits_are_hidden() {
    assert!(is_retired_node("traits.subscribed"));
    assert!(is_retired_node("traits.marketing_consent.granted"));
    for kept in [
      capsules::MARKETING_CONSENT_NODE,
      "traits.email",
      "traits.name.first",
      "csrf_token",
      "password",
    ] {
      assert!(!is_retired_node(kept), "{kept} must still render");
    }
  }
}
