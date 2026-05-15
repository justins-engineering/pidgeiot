use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::models::UiNodeAttributes::{A, Div, Img, Input, Script, Text};

const TEL_REGEX: &str = "\\+?(9[976]\\d|8[987530]\\d|6[987]\\d|5[90]\\d|42\\d|3[875]\\d|2[98654321]\\d|9[8543210]|8[6421]|6[6543210]|5[87654321]|4[987654310]|3[9643210]|2[70]|7|1)\\d{1,14}";

#[component]
fn InputFieldNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
  validate: bool,
  pattern: Option<String>,
  hint: Option<Element>,
) -> Element {
  rsx! {
    label { class: "floating-label my-4",
      span {
        {
            if let Some(ref label) = meta {
                label.text.clone()
            } else {
                format!("{:?}", attrs.r#type)
            }
        }
      }
      input {
        required: if let Some(r) = attrs.required { r },
        autocomplete: if let Some(a) = attrs.autocomplete { format!("{a:?}").to_lowercase() },
        class: "input w-full",
        class: if validate { "validator" },
        disabled: attrs.disabled,
        id: if let Some(ref label) = meta { label.id.to_string() },
        name: attrs.name,
        placeholder: if let Some(ref label) = meta { label.text.clone() } else { format!("{:?}", attrs.r#type) },
        r#type: format!("{:?}", attrs.r#type).to_lowercase(),
        pattern: if let Some(pattern) = pattern { pattern },
        value: if let Some(v) = attrs.value { if let Some(serde_json::Value::String(s)) = v { s } else { "".to_string() } },
      }
      if validate {
        div { class: "validator-hint hidden",
          if let Some(hint) = hint {
            {hint}
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
) -> Element {
  rsx! {
    button {
      disabled: attrs.disabled,
      class: "btn btn-primary w-full my-4",
      id: if let Some(ref label) = meta { label.id.to_string() },
      name: attrs.name,
      r#type: format!("{:?}", attrs.r#type).to_lowercase(),

      value: if let Some(v) = attrs.value { if let Some(t) = v {
          match t {
              serde_json::Value::String(s) => s,
              _ => "".to_string(),
          }
      } else {
          "".to_string()
      } },

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
) -> Element {
  rsx! {
    if let Some(ref label) = meta {
      label { id: label.id, class: "w-full",
        {label.text.to_owned()}
        input {
          disabled: attrs.disabled,
          class: "input w-full",
          name: attrs.name,
          r#type: format!("{:?}", attrs.r#type).to_lowercase(),
          value: if let Some(v) = attrs.value { if let Some(t) = v {
              match t {
                  serde_json::Value::String(s) => s,
                  serde_json::Value::Number(n) => n.to_string(),
                  serde_json::Value::Bool(b) => b.to_string(),
                  serde_json::Value::Array(a) => format!("{a:?}"),
                  _ => "".to_string(),
              }
          } else {
              "".to_string()
          } },
        }
      }
    } else {
      input {
        disabled: attrs.disabled,
        class: "input w-full",
        name: attrs.name,
        r#type: format!("{:?}", attrs.r#type).to_lowercase(),

        value: if let Some(v) = attrs.value { if let Some(t) = v {
            match t {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Array(a) => format!("{a:?}"),
                _ => "".to_string(),
            }
        } else {
            "".to_string()
        } },
      }
    }
  }
}

#[component]
fn InputCheckBoxNode(
  meta: Option<Box<ory_kratos_client_wasm::models::UiText>>,
  attrs: ory_kratos_client_wasm::models::UiNodeInputAttributes,
) -> Element {
  rsx! {
    if let Some(ref label) = meta {
      label { id: label.id, class: "w-full",
        input {
          disabled: attrs.disabled,
          class: "checkbox",
          name: attrs.name,
          r#type: format!("{:?}", attrs.r#type).to_lowercase(),
          checked: if let Some(v) = attrs.value { if let Some(serde_json::Value::Bool(b)) = v { b } else { false } },
          value: true,
        }
        span { class: "ml-4", {label.text.to_owned()} }
      }
    } else {
      label { class: "w-full",
        input {
          disabled: attrs.disabled,
          class: "checkbox",
          name: attrs.name,
          r#type: format!("{:?}", attrs.r#type).to_lowercase(),
          checked: if let Some(v) = attrs.value { if let Some(serde_json::Value::Bool(b)) = v { b } else { false } },
          value: true,
        }
        span { class: "ml-4", {attrs.name.to_owned()} }
      }
    }
  }
}

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
      class: {
          match message.r#type {
              ory_kratos_client_wasm::models::ui_text::TypeEnum::Error => {
                  "alert alert-error"
              }
              ory_kratos_client_wasm::models::ui_text::TypeEnum::Info => "alert alert-info",
              ory_kratos_client_wasm::models::ui_text::TypeEnum::Success => {
                  "alert alert-success"
              }
          }
      },
      span { {message.text} }
    }
  }
}

#[component]
fn NodeBuilder(nodes: Vec<ory_kratos_client_wasm::models::UiNode>) -> Element {
  rsx! {
    for node in nodes {
      match *node.attributes {
          Input(i) => {
              match i.r#type {
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Text => {
                      rsx! {
                        InputFieldNode { meta: node.meta.label, attrs: *i, validate: false }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Password => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
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
                          // title: "Must be more than 8 characters, including number, lowercase letter, uppercase letter"
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Number => {
                      rsx! {
                        InputOtherNode { meta: node.meta.label, attrs: *i }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Checkbox => {
                      rsx! {
                        InputCheckBoxNode { meta: node.meta.label, attrs: *i }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Hidden => {
                      rsx! {
                        input {
                          autocomplete: if let Some(a) = i.autocomplete { format!("{a:?}").to_lowercase() },
                          disabled: i.disabled,
                          name: i.name,
                          id: if let Some(ref label) = node.meta.label { format!("{}", label.id) },
                          r#type: format!("{:?}", i.r#type).to_lowercase(),
                          value: if let Some(v) = i.value { if let Some(t) = v {
                              match t {
                                  serde_json::Value::String(s) => s,
                                  _ => "".to_string(),
                              }
                          } else {
                              "".to_string()
                          } },
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Email => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
                          validate: true,
                          hint: rsx! {
                            p { "Please enter a valid email address" }
                          },
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Tel => {
                      rsx! {
                        InputFieldNode {
                          meta: node.meta.label,
                          attrs: *i,
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
                        }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Submit => {
                      rsx! {
                        InputButtonNode { meta: node.meta.label, attrs: *i }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Button => {
                      rsx! {
                        InputButtonNode { meta: node.meta.label, attrs: *i }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::DatetimeLocal => {
                      rsx! {
                        InputOtherNode { meta: node.meta.label, attrs: *i }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Date => {
                      rsx! {
                        InputOtherNode { meta: node.meta.label, attrs: *i }
                      }
                  }
                  ory_kratos_client_wasm::models::ui_node_input_attributes::TypeEnum::Url => {
                      rsx! {
                        InputOtherNode { meta: node.meta.label, attrs: *i }
                      }
                  }
              }
          }
          Text(text) => {
              rsx! {
                TextNode { meta: node.meta.label, attrs: *text }
              }
          }
          Img(img) => {
              rsx! {
                ImageNode { meta: node.meta.label, attrs: *img }
              }
          }
          A(link) => {
              rsx! {
                LinkNode { meta: node.meta.label, attrs: *link }
              }
          }
          Div(div) => {
              rsx! {
                DivNode { attrs: *div }
              }
          }
          Script(script) => {
              rsx! {
                ScriptNode { attrs: *script }
              }
          }
      }
    }
  }
}

#[component]
pub fn FormBuilder(ui: ory_kratos_client_wasm::models::UiContainer) -> Element {
  let default = ui
    .nodes
    .extract_if(.., |n| {
      n.group == ory_kratos_client_wasm::models::ui_node::GroupEnum::Default
    })
    .collect::<Vec<_>>();

  if default.is_empty() {
    error!("Returned schema missing 'Default' group!");
    return rsx! {};
  }

  let node_groups = ui.nodes.chunk_by(|a, b| a.group == b.group);

  let node_groups = node_groups
    .map(|g| g.to_vec())
    .collect::<Vec<_>>()
    .to_owned();

  if node_groups.is_empty() {
    rsx! {
      if let Some(messages) = ui.messages {
        for message in messages {
          MessageNode { message }
        }
      }
      form { action: ui.action.clone(), method: ui.method.clone(),
        div { class: "my-2",
          fieldset { class: "fieldset bg-base-100 border border-base-300 rounded-box p-4",
            NodeBuilder { nodes: default }
          }
        }
      }
    }
  } else {
    rsx! {
      if let Some(messages) = ui.messages {
        for message in messages {
          MessageNode { message }
        }
      }
      for node_group in node_groups {
        form { action: ui.action.clone(), method: ui.method.clone(),
          div { class: "my-2",
            fieldset { class: "fieldset bg-base-100 border border-base-300 rounded-box p-4",
              legend { class: "fieldset-legend text-xl",
                {
                    match node_group[0].group {
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
              NodeBuilder { nodes: default.clone() }
              NodeBuilder { nodes: node_group }
            }
          }
        }
      }
    }
  }
}
