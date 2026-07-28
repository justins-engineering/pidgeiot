use crate::models::AlertVariant;
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
