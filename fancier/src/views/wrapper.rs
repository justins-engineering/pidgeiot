use crate::components::{FeedbackForm, FeedbackModal, Footer, Navbar};
use crate::helpers::page_title;
use crate::{Route, Session};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdCircleAlert;

/// Keeps the browser tab named after the page you are actually on.
///
/// Nothing else does: the per-page <title> in a prerendered page is static
/// HTML, so once the app hydrates, every client-side navigation would leave
/// the tab reading whatever page happened to be loaded first. Its own
/// component rather than a line in `Wrapper` so that a navigation re-renders
/// six lines instead of the whole chrome.
#[component]
fn PageTitle() -> Element {
  let route = use_route::<Route>();
  let title = page_title(&route);

  // This component already reruns on every navigation, which makes it the
  // free place to feed the error reporter's trail (a no-op during the SSG
  // prerender, and deduped against rerenders without a route change).
  crate::helpers::error_report::breadcrumb_nav(&route.to_string());

  rsx! {
    document::Title { "{title}" }
  }
}

#[component]
pub fn Wrapper() -> Element {
  // Feedback modal open-state lives here -- the one ancestor
  // both the Footer link and the Navbar menu items share -- and is handed
  // down via context (`FeedbackForm`). Conditionally rendered rather than
  // a native <dialog> so every open remounts the form fresh (it holds
  // reset-sensitive state; see FeedbackModal's doc comment).
  let mut feedback_open = use_context_provider(|| FeedbackForm(Signal::new(false))).0;
  let session = use_context::<Session>();
  let mut problem_open = use_signal(|| false);

  rsx! {
    PageTitle {}
    Navbar {}
    main { Outlet::<Route> {} }
    // Signed-in only, by owner decision: maximum value where breakage
    // actually happens, no effect on the marketing pages' conversion
    // surface. Hanging off AuthState also makes it SSG-safe for free --
    // the prerender pass never leaves Pending, so no prerendered page
    // carries the button.
    if (session.state)().is_authenticated() {
      // Sticky in flow just above the footer rather than fixed to the
      // viewport: it floats while the footer is off screen and settles
      // into this slot once the footer scrolls in, so it can never cover
      // the footer's links. The strip spans the width, hence
      // pointer-events-none on it and auto on the button, so it doesn't
      // swallow clicks on whatever it floats over.
      div {
        class: "sticky bottom-4 z-40 mb-2 flex justify-end pe-4 pointer-events-none",
        button {
          id: "report-a-problem",
          class: "btn btn-primary btn-sm shadow-lg gap-2 rounded-full pointer-events-auto",
          r#type: "button",
          aria_label: "Report a problem",
          onclick: move |_| problem_open.set(true),
          Icon { icon: LdCircleAlert, class: "size-4" }
          "Report a problem"
        }
      }
    }
    Footer {}
    if feedback_open() {
      FeedbackModal { on_close: move |_| feedback_open.set(false) }
    }
    if problem_open() {
      FeedbackModal { on_close: move |_| problem_open.set(false), problem: true }
    }
  }
}
