use crate::helpers::browser::window;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Resolves after `ms` milliseconds -- a tiny `setTimeout` wrapper so a
/// polling component (currently just the public demo page, `views/demo.rs`)
/// can `.await` a delay between refreshes without pulling in gloo-timers as
/// a new direct dependency; `web-sys`'s `Window` and `wasm-bindgen-futures`
/// are already dependencies everything else here uses.
pub async fn sleep_ms(ms: i32) {
  let promise = js_sys::Promise::new(&mut |resolve, _reject| {
    let window = window!();
    // Only fails if `resolve` isn't callable, which it always is here --
    // nothing meaningful to recover into on error, so best-effort like the
    // rest of this crate's browser-API wrappers.
    let _ =
      window.set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(), ms);
  });
  let _ = JsFuture::from(promise).await;
}
