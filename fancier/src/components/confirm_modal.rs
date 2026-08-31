//! Confirm step for destructive actions that carry no other guard.

use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

/// Yes/no confirm, matching `DeleteAlertModal`'s shape. Render it from a
/// parent signal so each open remounts fresh, and give `id` the label the
/// dialog is described by. The action itself stays with the caller, which
/// already owns where its errors and progress show.
///
/// `confirm_value` adds `DeletePigeonModal`'s stricter step: the name has to
/// be typed back before the button enables. Reserve it for what cannot be
/// undone, or it becomes friction people learn to type through.
#[component]
pub fn ConfirmModal(
  id: &'static str,
  title: &'static str,
  confirm_label: &'static str,
  confirm_value: Option<String>,
  on_confirm: EventHandler<()>,
  on_close: EventHandler<()>,
  children: Element,
) -> Element {
  let mut typed = use_signal(String::new);
  let is_confirmed = match &confirm_value {
    Some(value) => typed() == *value,
    None => true,
  };

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
        if let Some(value) = confirm_value.as_ref() {
          label { class: "fieldset-legend text-xs font-semibold mb-1 block",
            "Type "
            span { class: "font-mono bg-base-200 rounded px-1", "{value}" }
            " to confirm"
          }
          input {
            class: "input input-bordered w-full text-sm font-mono",
            r#type: "text",
            autocomplete: "off",
            value: "{typed}",
            oninput: move |e| typed.set(e.value()),
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
          }
        }
        div { class: "modal-action",
          button {
            class: "btn btn-ghost",
            onclick: move |_| on_close.call(()),
            "Cancel"
          }
          button {
            class: "btn btn-error",
            disabled: !is_confirmed,
            onclick: move |_| on_confirm.call(()),
            "{confirm_label}"
          }
        }
      }
    }
  }
}
