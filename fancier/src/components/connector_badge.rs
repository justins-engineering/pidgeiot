use capsules::Connector;
use dioxus::prelude::*;

#[component]
pub fn ConnectorBadge(connector: Connector) -> Element {
  match connector {
    Connector::Https(_) => rsx! {
      div { class: "badge badge-primary badge-outline badge-sm", "HTTPS" }
    },
    Connector::Coap(_) => rsx! {
      div { class: "badge badge-secondary badge-outline badge-sm", "CoAP" }
    },
  }
}
