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

const TEL_REGEX: &str = "\\+?(9[976]\\d|8[987530]\\d|6[987]\\d|5[90]\\d|42\\d|3[875]\\d|2[98654321]\\d|9[8543210]|8[6421]|6[6543210]|5[87654321]|4[987654310]|3[9643210]|2[70]|7|1)\\d{1,14}";

// --- Input Node Components ---

#[component]
fn InputFieldNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  messages: Vec<ory_kratos_client_wasm::models::UiText>,
  validate: bool,
  pattern: Option<String>,
  hint: Option<Element>,
  id_suffix: String,
) -> Element {
  let input_id = format!("{}_{}", attrs.name, id_suffix);
  let label_text = meta
    .as_ref()
    .map(|m| m.text.clone())
    .unwrap_or_else(|| format!("{:?}", attrs.r#type));

  rsx! {
    label { class: "floating-label my-4", r#for: "{input_id}",
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
        r#type: input_type_token(attrs.r#type),
        pattern,
        value: parse_json_string(&attrs.value),
      }
      if validate {
        div { class: "validator-hint hidden", {hint} }
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
/// `subscribed` is the bare boolean `marketing_consent` replaces. It stays
/// declared in the identity schema so that no existing identity can be
/// invalidated mid-deprecation, and staying declared is exactly why Kratos
/// keeps emitting a node for it -- which would put two subscribe-shaped
/// checkboxes on the registration form, the opposite of the clarity a
/// consent request needs. Delete this together with the trait; the
/// procedure is in docs/consent.md.
fn is_retired_node(name: &str) -> bool {
  name == "traits.subscribed"
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
                          hint: rsx! {
                            p { "Please enter a valid phone number without:" }
                            ul { class: "list-disc list-inside",
                              li { "Characters" }
                              li { "Spaces" }
                              li { "Hyphens -" }
                              li { "Parenthesis ()" }
                            }
                          },
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
  use super::{checkbox_helper, is_retired_node};

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

  /// The whole point of the hide-rule is that exactly one node disappears.
  /// A filter that caught a real field would silently drop it from the
  /// form, and a trait that never posts is a trait Kratos clears.
  #[test]
  fn only_the_deprecated_trait_is_hidden() {
    assert!(is_retired_node("traits.subscribed"));
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
