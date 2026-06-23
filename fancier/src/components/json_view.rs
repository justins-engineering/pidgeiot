use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdCopy;

#[component]
pub fn JsonViewer(json: capsules::JsonString, title: Option<String>) -> Element {
  let mut copied = use_signal(|| false);

  // Formatting target for copy-to-clipboard functionality
  let raw_json_str = json.to_string();
  let pretty_json = json.to_pretty();

  rsx! {
    div { class: "relative w-full rounded-xl border border-stone-800/40 bg-base-300/40 backdrop-blur-md shadow-2xl overflow-hidden",
      div { class: "flex items-center justify-between px-4 py-2 border-b border-stone-800/30 bg-base-200/30 text-xs font-mono tracking-wider text-base-content/70",
        if let Some(heading) = title {
          span { "{heading}" }
        }
        button {
          class: "btn btn-square btn-ghost btn-xs text-base-content/50 hover:text-primary hover:bg-base-100/50 transition-all",
          title: "Copy JSON",
          onclick: move |_| {
              #[cfg(feature = "web")]
              if let Some(window) = web_sys::window() {
                  let clipboard = window.navigator().clipboard();
                  let _ = clipboard.write_text(&raw_json_str);
                  copied.set(true);
              }
          },
          if json.to_string().is_empty() {

          } else if *copied.read() {
            span { class: "text-success font-sans text-[10px]", "Copied!" }
          } else {
            Icon { icon: LdCopy }
          }
        }
      }

      // Main Code Window
      div { class: "mockup-code bg-transparent before:content-none p-0 my-2 w-full overflow-x-auto select-text scrollbar-thin text-sm font-mono",
        pre { class: "px-5 py-2 text-base-content leading-relaxed before:mr-0!",
          code { "{pretty_json}" }
        }
      }
    }
  }
}
