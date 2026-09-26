use capsules::Connector;
use dioxus::prelude::*;

/// A pigeon's transport as a small outline badge, one color per variant.
/// Never `badge-neutral`, whose text and fill match in both themes.
#[component]
pub fn ConnectorBadge(connector: Connector) -> Element {
  match connector {
    Connector::Https(_) => rsx! {
      div { class: "badge badge-primary badge-outline badge-sm", "HTTPS" }
    },
    Connector::Coap(_) => rsx! {
      div { class: "badge badge-secondary badge-outline badge-sm", "CoAP" }
    },
    Connector::Mqtt(_) => rsx! {
      div { class: "badge badge-accent badge-outline badge-sm", "MQTT" }
    },
    Connector::Nidd(_) => rsx! {
      div { class: "badge badge-info badge-outline badge-sm", "NIDD" }
    },
  }
}
