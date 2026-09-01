//! Bottom-of-page section collecting a detail view's destructive actions.

use dioxus::prelude::*;

/// Error-tinted section a detail page ends with. `id` carries the
/// page-prefixed section id, e.g. `pigeon-danger-zone`.
///
/// The tint, border and solid button already carry the warning, so the words
/// stay `base-content` rather than compete with them.
#[component]
pub fn DangerZone(id: &'static str, children: Element) -> Element {
  rsx! {
    section {
      id,
      class: "w-full flex flex-col gap-4 bg-error/5 p-6 rounded-box border border-error/60 shadow-sm",
      h2 { class: "text-lg font-bold", "Danger Zone" }
      {children}
    }
  }
}

/// One action inside a [`DangerZone`]: what it does on the left, its trigger
/// on the right.
#[component]
pub fn DangerAction(
  title: &'static str,
  description: &'static str,
  label: &'static str,
  onclick: EventHandler<()>,
) -> Element {
  rsx! {
    div { class: "flex flex-col gap-3 md:flex-row md:items-center md:justify-between md:gap-8",
      div {
        p { class: "font-semibold", "{title}" }
        p { class: "text-sm text-base-content/70", "{description}" }
      }
      button {
        class: "btn btn-error md:min-w-44",
        onclick: move |_| onclick.call(()),
        "{label}"
      }
    }
  }
}
