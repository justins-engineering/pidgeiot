use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdTriangleAlert, LdX};

#[component]
pub fn Alert() -> Element {
  let mut dismissed: Signal<bool> = use_signal(|| false);
  rsx! {
    div {
      role: "alert",
      class: "sticky justify-items-center-safe top-16 sm:top-18 z-25 alert alert-warning alert-soft not-dark:text-warning-content sm:px-4 md:px-8 lg:px-16 xl:px-32 2xl:px-64",
      class: if dismissed() { "hidden" },
      Icon { icon: LdTriangleAlert, title: "Important" }
      span {
        "This website is under construction, it may not be fully functional and it will change!"
      }
      button {
        class: "btn btn-sm btn-square btn-warning btn-soft not-dark:text-warning-content",
        aria_label: "Close alert",
        onclick: move |_| {
            dismissed.set(true);
        },
        Icon { icon: LdX, title: "Dismiss" }
      }
    }
  }
}
