// Task #23's dovecote routes (docs/api.md's "Firmware" section) and
// task #25 Part B's dashboard UI (components/firmware_modal.rs). Firmware
// images are catalogued per-flock, not per-pigeon (see capsules::FirmwareImage's
// doc comment) -- assigning one to a pigeon is a separate concern handled
// through the existing `api::pigeons::update_shadow`, not here.
use crate::api::{fetch_bytes, fetch_json};
use capsules::FirmwareImage;
use uuid::Uuid;
use wasm_bindgen_futures::JsFuture;

/// `POST /flocks/:flock_id/firmware?version=<version>&board=<board>` -- the
/// request body **is** the image (raw bytes), not JSON. `size`/`sha256` in
/// the response are always computed server-side from the uploaded bytes;
/// the caller should compare its own client-side `helpers::sha256_hex`
/// result against the returned `sha256` rather than trust its own
/// computation blindly. `version`/`board` are percent-encoded via the
/// browser's own `encodeURIComponent` (`js_sys::encode_uri_component`)
/// since both are free-text and may contain `+`/`&`/spaces/`/` etc. that
/// would otherwise corrupt the query string. `board` (task #20, phase 1)
/// is required by dovecote as of commit 5a54948 -- an upload with an empty
/// board 400s -- it's the Zephyr `CONFIG_BOARD_TARGET` this image was
/// built for, matched against a pigeon's own `board` before a shadow
/// assignment is allowed (see `components::FirmwareModal`'s fail-closed
/// assign gating).
pub async fn upload(
  flock_id: Uuid,
  version: &str,
  board: &str,
  bytes: &[u8],
) -> Option<FirmwareImage> {
  let encoded_version = js_sys::encode_uri_component(version);
  let encoded_board = js_sys::encode_uri_component(board);
  let path = format!("/flocks/{flock_id}/firmware?version={encoded_version}&board={encoded_board}");

  let response = fetch_bytes("POST", &path, bytes).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value::<FirmwareImage>(json).ok()
}

/// `GET /flocks/:flock_id/firmware` -- every image uploaded for this flock,
/// newest first.
pub async fn list(flock_id: Uuid) -> Option<Vec<FirmwareImage>> {
  let path = format!("/flocks/{flock_id}/firmware");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value::<Vec<FirmwareImage>>(json).ok()
}
