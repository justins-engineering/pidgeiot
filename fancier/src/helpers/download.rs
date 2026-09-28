use dioxus::logger::tracing::error;
use wasm_bindgen::JsCast;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

/// Decodes a base64 string (as returned by `capsules::PigeonLogChunk::data`)
/// into raw bytes via the browser's native `atob`, byte-for-byte. There's no
/// base64 crate anywhere in this dependency tree, and pulling one in just
/// for this one call site would duplicate what every browser already ships
/// -- `atob` yields a JS "binary string" where each UTF-16 code unit is one
/// decoded byte (0..=255), which is exactly what `data` was base64-encoded
/// from on the dovecote side.
pub fn decode_base64(data: &str) -> Option<Vec<u8>> {
  let binary = web_sys::window()?.atob(data).ok()?;
  Some(binary.chars().map(|c| c as u8).collect())
}

/// Saves `bytes` to disk as `filename` via a throwaway `Blob` + object URL +
/// synthetic anchor click -- the standard way to hand a browser-derived byte
/// buffer to the user without a server round-trip.
pub fn download_bytes(bytes: &[u8], filename: &str, mime_type: &str) -> Option<()> {
  let array = js_sys::Uint8Array::from(bytes);
  let parts = js_sys::Array::new();
  parts.push(&array);

  let options = BlobPropertyBag::new();
  options.set_type(mime_type);
  let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
    .inspect_err(|err| error!("Failed to construct download Blob: {err:?}"))
    .ok()?;
  save_blob(&blob, filename)
}

/// Saves a file made of JS string `parts`, in order, as `filename`. Text built in pages goes
/// straight from each page into the Blob, so the whole file never sits in wasm memory, which
/// never shrinks.
pub fn download_text_parts(parts: &js_sys::Array, filename: &str, mime_type: &str) -> Option<()> {
  let options = BlobPropertyBag::new();
  options.set_type(mime_type);
  let blob = Blob::new_with_str_sequence_and_options(parts, &options)
    .inspect_err(|err| error!("Failed to construct download Blob: {err:?}"))
    .ok()?;
  save_blob(&blob, filename)
}

/// The object URL is revoked right after the click; the download keeps the Blob's own data
/// alive, not the URL.
fn save_blob(blob: &Blob, filename: &str) -> Option<()> {
  let url = Url::create_object_url_with_blob(blob)
    .inspect_err(|err| error!("Failed to create object URL for download: {err:?}"))
    .ok()?;

  let document = web_sys::window()?.document()?;
  let anchor = document
    .create_element("a")
    .inspect_err(|err| error!("Failed to create download anchor: {err:?}"))
    .ok()?
    .dyn_into::<HtmlAnchorElement>()
    .ok()?;
  anchor.set_href(&url);
  anchor.set_download(filename);
  anchor.click();

  let _ = Url::revoke_object_url(&url);
  Some(())
}
