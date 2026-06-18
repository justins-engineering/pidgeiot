use crate::models::AlertVariant;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCircleAlert, LdCircleCheck, LdInfo, LdTriangleAlert, LdX,
};

#[component]
pub fn Alert(
  #[props(default)] variant: AlertVariant,
  #[props(default = false)] persistent: bool,
  children: Element,
) -> Element {
  let mut dismissed = use_signal(|| false);

  // Early return if dismissed avoids rendering the DOM node entirely,
  // which is more efficient than using a "hidden" CSS class.
  if dismissed() {
    return rsx! {};
  }

  let alert_theme = variant.theme_classes();
  let btn_theme = variant.btn_classes();

  rsx! {
    div {
      role: "alert",
      class: "sticky justify-items-center-safe top-16 sm:top-18 z-25 alert alert-soft {alert_theme}",
      // class: "sm:px-4 md:px-8 lg:px-16 xl:px-32 2xl:px-64",
      // Render the contextual icon
      match variant {
          AlertVariant::Info => rsx! {
            Icon { icon: LdInfo, title: "Info" }
          },
          AlertVariant::Success => rsx! {
            Icon { icon: LdCircleCheck, title: "Success" }
          },
          AlertVariant::Warning => rsx! {
            Icon { icon: LdTriangleAlert, title: "Warning" }
          },
          AlertVariant::Error => rsx! {
            Icon { icon: LdCircleAlert, title: "Error" }
          },
      }

      // This acts as your `yield`
      span { {children} }

      // Conditionally render the close button
      if !persistent {
        button {
          class: "btn btn-sm btn-square btn-soft {btn_theme}",
          aria_label: "Close alert",
          onclick: move |_| dismissed.set(true),
          Icon { icon: LdX, title: "Dismiss" }
        }
      }
    }
  }
}
