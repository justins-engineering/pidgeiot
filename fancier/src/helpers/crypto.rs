use dioxus::logger::tracing::error;
use wasm_bindgen_futures::JsFuture;

/// Client-side SHA-256 via the browser's native `SubtleCrypto`, so the
/// caller can compare against the server's own hash. No sha2 crate in this
/// dependency tree -- hashing a whole firmware image up to
/// `capsules::MAX_FIRMWARE_BYTES` is worth reaching for the browser's
/// hardware-backed implementation over a pure-Rust one compiled to wasm.
/// Returns lowercase hex, matching `capsules::FirmwareTarget::sha256`'s
/// wire format.
pub async fn sha256_hex(bytes: &[u8]) -> Option<String> {
  let crypto = web_sys::window()?
    .crypto()
    .inspect_err(|err| error!("No SubtleCrypto available: {err:?}"))
    .ok()?;
  let promise = crypto
    .subtle()
    .digest_with_str_and_u8_array("SHA-256", bytes)
    .inspect_err(|err| error!("SubtleCrypto.digest() failed to start: {err:?}"))
    .ok()?;
  let digest = JsFuture::from(promise)
    .await
    .inspect_err(|err| error!("SubtleCrypto.digest() rejected: {err:?}"))
    .ok()?;
  let bytes = js_sys::Uint8Array::new(&digest).to_vec();

  let mut hex = String::with_capacity(bytes.len() * 2);
  for byte in bytes {
    hex.push_str(&format!("{byte:02x}"));
  }
  Some(hex)
}
