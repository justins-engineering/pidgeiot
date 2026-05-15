use crate::partials::{CodeShowcase, Connectivity, Cta, Features, Home, Infrastructure};
use dioxus::prelude::*;

#[component]
pub fn Index() -> Element {
  rsx! {
    Home {}
    Features {}
    CodeShowcase {}
    Connectivity {}
    Infrastructure {}
    Cta {}
  }
}
