use crate::models::AlertVariant;
use ory_kratos_client_wasm::models::{UiContainer, ui_text::TypeEnum};

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

pub fn extract_expired_message(content: &str) -> String {
  serde_json::from_str::<serde_json::Value>(content)
    .ok()
    .and_then(|val| {
      val
        .pointer("/error/message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
    })
    .unwrap_or_else(|| {
      "The registration flow expired. A fresh form has been loaded automatically.".to_string()
    })
}
