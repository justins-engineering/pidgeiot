use crate::views::Flocks;
use dioxus::prelude::*;

#[component]
pub fn Dashboard() -> Element {
  rsx! {
    Flocks {}
  }
}
