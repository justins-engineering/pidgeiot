use crate::helpers::sleep_ms;
use dioxus::logger::tracing::error;
use wasm_bindgen::{JsCast, JsValue};

/// Call one of the Ory-provided WebAuthn globals (`window.oryPasskeyLogin`
/// and friends) by name.
///
/// The defining script arrives as a Kratos script UI node and loads
/// asynchronously, so a trigger can fire -- a button click, or a mount for
/// the passkey-autofill initializer -- before the function exists yet.
/// Polling briefly turns that race into a short wait; a script that never
/// loads still ends in a logged error instead of a silently dead button.
pub async fn invoke_webauthn_trigger(name: &'static str) {
  // 5s total: generous for a same-host script fetch, short enough that a
  // hard failure surfaces while the user is still looking at the button.
  const ATTEMPTS: u32 = 50;
  const RETRY_MS: i32 = 100;

  for _ in 0..ATTEMPTS {
    let global = js_sys::global();
    if let Ok(value) = js_sys::Reflect::get(&global, &JsValue::from_str(name))
      && let Some(function) = value.dyn_ref::<js_sys::Function>()
    {
      if let Err(err) = function.call0(&global) {
        error!("Ory WebAuthn trigger {name} failed: {err:?}");
      }
      return;
    }
    sleep_ms(RETRY_MS).await;
  }
  error!("Ory WebAuthn trigger {name} is not defined; its script never loaded");
}
