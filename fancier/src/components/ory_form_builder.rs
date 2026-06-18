use crate::{
  components::Alert,
  helpers::{parse_json_bool, parse_json_string},
  models::AlertVariant,
};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::models::UiNodeAttributes::{A, Div, Img, Input, Script, Text};
use std::collections::BTreeMap;

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
        autocomplete: attrs.autocomplete.map(|a| format!("{a:?}").to_lowercase()),
        placeholder: label_text,
        r#type: format!("{:?}", attrs.r#type).to_lowercase(),
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

  rsx! {
    button {
      id: "{input_id}",
      disabled: attrs.disabled,
      class: "btn btn-primary w-full my-4",
      name: attrs.name,
      r#type: format!("{:?}", attrs.r#type).to_lowercase(),
      value: parse_json_string(&attrs.value),
      if let Some(ref label) = meta {
        {label.text.to_string()}
      }
    }
  }
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
          r#type: format!("{:?}", attrs.r#type).to_lowercase(),
          value: parse_json_string(&attrs.value),
        }
      }
    } else {
      input {
        id: "{input_id}",
        disabled: attrs.disabled,
        class: "input w-full",
        name: attrs.name,
        r#type: format!("{:?}", attrs.r#type).to_lowercase(),
        value: parse_json_string(&attrs.value),
      }
    }
  }
}

#[component]
fn InputCheckBoxNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  id_suffix: String,
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
        r#type: format!("{:?}", attrs.r#type).to_lowercase(),
        checked: parse_json_bool(&attrs.value),
        value: node_value,
      }
      span { class: "ml-4", "{label_text}" }
    }
  }
}

// --- Static Media / Structural Nodes ---

#[component]
fn ImageNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeImageAttributes,
) -> Element {
  rsx! {
    if let Some(ref label) = meta {
      label { id: label.id, class: "text-lg mb-4",
        {label.text.clone()}
        img {
          height: attrs.height,
          id: attrs.id,
          src: attrs.src,
          width: attrs.width,
          alt: label.text.to_owned(),
        }
      }
    } else {
      img {
        height: attrs.height,
        id: attrs.id,
        src: attrs.src,
        width: attrs.width,
      }
    }
  }
}

#[component]
fn TextNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeTextAttributes,
) -> Element {
  rsx! {
    if let Some(ref label) = meta {
      label { r#for: attrs.id.clone(), id: label.id, class: "text-lg",
        {label.text.to_owned()}
      }
    }
    p { id: attrs.id, class: "", {attrs.text.text} }
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
  rsx! {
    script {
      r#async: attrs.r#async,
      crossorigin: attrs.crossorigin,
      id: attrs.id,
      integrity: attrs.integrity,
      nonce: attrs.nonce,
      referrerpolicy: attrs.referrerpolicy,
      src: attrs.src,
      r#type: attrs.r#type,
    }
  }
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
fn NodeBuilder(nodes: Vec<ory_kratos_client_wasm::models::UiNode>, id_suffix: String) -> Element {
  rsx! {
    for node in nodes {
      match *node.attributes {
          Input(i) => {
              match i.r#type {
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
                      rsx! {
                        InputCheckBoxNode { meta: node.meta.label, attrs: *i, id_suffix: id_suffix.clone() }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Hidden => {
                      rsx! {
                        input {
                          id: format!("{}_{}", i.name, id_suffix),
                          autocomplete: i.autocomplete.map(|a| format!("{a:?}").to_lowercase()),
                          disabled: i.disabled,
                          name: i.name,
                          r#type: format!("{:?}", i.r#type).to_lowercase(),
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

#[component]
pub fn FormBuilder(
  ui: ory_kratos_client_wasm::models::UiContainer,
  on_submit: EventHandler<Event<FormData>>,
) -> Element {
  // 1. O(N) Stable Partition: Separate CSRF/Default nodes from Flow nodes
  let (default_nodes, flow_nodes): (Vec<_>, Vec<_>) = ui
    .nodes
    .into_iter()
    .partition(|n| n.group == ory_kratos_client_wasm::models::ui_node::GroupEnum::Default);

  if default_nodes.is_empty() {
    error!("Returned schema missing 'Default' group. CSRF protection compromised.");
    return rsx! {};
  }

  // 2. Safely bucket remaining nodes by group to prevent interleaving crashes
  let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
  for node in flow_nodes {
    groups.entry(node.group).or_default().push(node);
  }

  rsx! {
    if groups.is_empty() {
      form { action: ui.action.clone(), method: ui.method.clone(),
        div { class: "my-2",
          fieldset { class: "fieldset bg-base-100 border border-base-300 rounded-box p-4",
            NodeBuilder {
              nodes: default_nodes,
              id_suffix: "default".to_string(),
            }
          }
        }
      }
    } else {
      for (group_enum , group_nodes) in groups {
        form {
          action: ui.action.clone(),
          method: ui.method.clone(),
          onsubmit: move |ev| {
              ev.prevent_default();
              on_submit.call(ev);
          },
          div { class: "my-2",
            fieldset { class: "fieldset bg-base-100 border border-base-300 rounded-box p-4",
              legend { class: "fieldset-legend text-xl",
                {
                    match group_enum {
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Password => "Password",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Oidc => "OIDC",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Profile => "Profile",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Code => "Code",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Totp => "TOTP",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::LookupSecret => {
                            "Recovery"
                        }
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Webauthn => {
                            "Web Authentication"
                        }
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Passkey => "Passkey",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Captcha => "Captcha",
                        ory_kratos_client_wasm::models::ui_node::GroupEnum::Saml => "SAML",
                        _ => "",
                    }
                }
              }
              // Namespace the IDs with the specific flow name to prevent collisions
              // if Kratos demands multiple forms (e.g., Password and Webauthn)
              NodeBuilder {
                nodes: default_nodes.clone(),
                id_suffix: format!("{group_enum:?}").to_lowercase(),
              }
              NodeBuilder {
                nodes: group_nodes,
                id_suffix: format!("{group_enum:?}").to_lowercase(),
              }
            }
          }
        }
      }
    }
  }
}
