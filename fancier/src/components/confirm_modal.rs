//! Confirm step for destructive actions that carry no other guard.

use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

/// Plain yes/no confirm, matching `DeleteAlertModal`'s shape. Render it from
/// a parent signal so each open remounts fresh, and give `id` the label the
/// dialog is described by. The action itself stays with the caller, which
/// already owns where its errors and progress show.
#[component]
pub fn ConfirmModal(
  id: &'static str,
  title: &'static str,
  confirm_label: &'static str,
  on_confirm: EventHandler<()>,
  on_close: EventHandler<()>,
  children: Element,
) -> Element {
  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": id,
      onkeydown: move |e| {
          if e.key() == Key::Escape {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-sm",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 { class: "text-lg font-bold text-error", id, "{title}" }
        p { class: "py-4 text-sm text-base-content/80", {children} }
        div { class: "modal-action",
          button {
            class: "btn btn-ghost",
            onclick: move |_| on_close.call(()),
            "Cancel"
          }
          button {
            class: "btn btn-error",
            onclick: move |_| on_confirm.call(()),
            "{confirm_label}"
          }
        }
      }
    }
  }
}
