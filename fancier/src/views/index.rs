use crate::partials::{Cta, DashboardPreview, Home, OpenSource, RouteStops, UseCaseStrip, Why};
use dioxus::prelude::*;

// Section order follows the chosen homepage design: hook, then the product
// itself, then how it gets there, then who it's for, then the licence.
// `Why` is the quieter investors/incubators section and must stay below all
// of the user-facing sections, just above the closing CTA.
#[component]
pub fn Index() -> Element {
  rsx! {
    Home {}
    DashboardPreview {}
    RouteStops {}
    UseCaseStrip {}
    OpenSource {}
    Why {}
    Cta {}
  }
}
