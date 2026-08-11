use crate::partials::{CodeShowcase, Connectivity, Cta, Features, Home, Infrastructure, Why};
use dioxus::prelude::*;

// Section order is the strategy (users first, investors second): everything
// through `Infrastructure` sells "you can start tonight" to an individual
// builder; `Why` is the quieter investors/incubators section and must stay
// below all of the user-facing sections, just above the closing CTA.
#[component]
pub fn Index() -> Element {
  rsx! {
    Home {}
    Features {}
    CodeShowcase {}
    Connectivity {}
    Infrastructure {}
    Why {}
    Cta {}
  }
}
