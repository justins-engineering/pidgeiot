use crate::Route;
use crate::components::{FeedbackForm, FeedbackModal, Footer, Navbar};
use dioxus::prelude::*;

#[component]
pub fn Wrapper() -> Element {
  // Feedback modal open-state (task #13) lives here -- the one ancestor
  // both the Footer link and the Navbar menu items share -- and is handed
  // down via context (`FeedbackForm`). Conditionally rendered rather than
  // a native <dialog> so every open remounts the form fresh (it holds
  // reset-sensitive state; see FeedbackModal's doc comment).
  let mut feedback_open = use_context_provider(|| FeedbackForm(Signal::new(false))).0;

  rsx! {
    Navbar {}
    main { Outlet::<Route> {} }
    Footer {}
    if feedback_open() {
      FeedbackModal { on_close: move |_| feedback_open.set(false) }
    }
  }
}
