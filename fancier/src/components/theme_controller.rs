use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdMoon, LdSun};
use wasm_theme::theme_toggle;

#[component]
pub fn ThemeController() -> Element {
  use_effect(theme_toggle);

  rsx! {
    label {
      class: "btn btn-ghost btn-circle swap swap-rotate",
      aria_label: "Dark/light theme toggle",
      input { name: "theme-toggle", r#type: "checkbox", value: "dark" }
      Icon { class: "swap-on size-6", icon: LdSun, title: "Light Mode" }
      Icon { class: "swap-off size-6", icon: LdMoon, title: "Dark Mode" }
    }
  }
}
