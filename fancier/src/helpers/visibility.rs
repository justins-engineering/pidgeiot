use crate::helpers::browser::window;

/// True while the page is backgrounded (a hidden/minimized tab, switched
/// away from) per the Page Visibility API. Polling loops (`GraphCard`,
/// `LogViewer`) check this before each fetch so an idle dashboard tab stops
/// hitting the Durable Object instead of polling at full cadence forever in
/// the background; the loop keeps running and simply resumes fetching on
/// its own next tick once the tab is visible again, no listener needed.
pub fn is_page_hidden() -> bool {
  let window = window!();
  match window.document() {
    Some(doc) => doc.hidden(),
    // No document to ask -- fail open (treat as visible) rather than
    // stalling a polling loop over an environment that can't answer.
    None => false,
  }
}
