use crate::api::orgs::{error_text, parse, to_body};
use crate::api::{fetch_bytes, fetch_json, fetch_json_any_status};
use capsules::{
  Connector, LogDictionaryInfo, Pigeon, PigeonCreateRequest, PigeonDetail,
  PigeonFlockUpdateRequest, PigeonLogChunk, PigeonShadow, PigeonShadowUpdateRequest,
  PigeonTelemetryEndpointUpdateRequest, PigeonUpdateRequest, TelemetryEndpoint,
};
use dioxus::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;
use wasm_bindgen_futures::JsFuture;

// dovecote's POST /pigeons/batch caps a single request at 48 ids
// (dovecote/src/lib.rs -- `pigeon_ids.len() > 48` -- a Workers subrequest
// fan-out limit, not a business rule), so a flock past that size has to be
// split into multiple requests here or every id past #48 is silently
// dropped from the flock's pigeon list.
const BATCH_LIMIT: usize = 48;

pub async fn list(pigeon_ids: &[String]) -> Option<()> {
  let mut fetched: HashMap<String, Pigeon> = HashMap::with_capacity(pigeon_ids.len());

  for chunk in pigeon_ids.chunks(BATCH_LIMIT) {
    let body = serde_json::to_string(chunk).ok()?;
    let body = serde_wasm_bindgen::to_value(&body).ok()?;
    let response = fetch_json("POST", "/pigeons/batch", Some(&body)).await?;
    let json = JsFuture::from(response.json().ok()?).await.ok()?;
    let pigeons_array = serde_wasm_bindgen::from_value::<Vec<Pigeon>>(json).ok()?;
    fetched.extend(
      pigeons_array
        .into_iter()
        .map(|pigeon| (pigeon.id.clone(), pigeon)),
    );
  }

  // Only merge into the shared cache once every chunk has succeeded --
  // writing after a partial failure would show a subset of the flock as
  // if it were the complete list, which is indistinguishable from a
  // genuinely smaller flock and is exactly the silent-failure shape this
  // function exists to avoid.
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.extend(fetched);
  pigeon_list.write();
  Some(())
}

pub async fn get(pigeon_id: &str) -> Option<String> {
  let mut path = String::with_capacity(73);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  let pigeon = serde_wasm_bindgen::from_value::<Pigeon>(json).ok()?;
  let id = pigeon.id.clone();
  pigeon_list.insert(id.clone(), pigeon);
  pigeon_list.write();
  Some(id)
}

pub async fn get_detail(pigeon_id: &str) -> Option<PigeonDetail> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/detail");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let detail = serde_wasm_bindgen::from_value::<PigeonDetail>(json).ok()?;

  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.insert(detail.pigeon.id.clone(), detail.pigeon.clone());
  pigeon_list.write();

  Some(detail)
}

pub async fn update(pigeon_id: &str, pur: &PigeonUpdateRequest) -> Option<String> {
  let mut path = String::with_capacity(73);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);

  let body = serde_json::to_string(pur).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  let pigeon = serde_wasm_bindgen::from_value::<Pigeon>(json).ok()?;
  let id = pigeon.id.clone();
  pigeon_list.insert(id.clone(), pigeon);
  pigeon_list.write();
  Some(id)
}

/// Returns the new pigeon's id and the connector exactly as minted --
/// with the credentials the create response carries once and no read
/// route ever returns again, which is why the caller gets the whole
/// connector rather than just its token: a PSK-bearing connector has a
/// second secret in it, and both have to reach the reveal.
pub async fn create(pigeon: &PigeonCreateRequest) -> Option<(String, Connector)> {
  let body = serde_json::to_string(pigeon).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("POST", "/flock/pigeons", Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  let detail = serde_wasm_bindgen::from_value::<PigeonDetail>(json).ok()?;
  let id = detail.pigeon.id.clone();

  // Cache the pigeon (token is stripped on subsequent GETs)
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.insert(id.clone(), detail.pigeon.clone());
  pigeon_list.write();

  Some((id, detail.pigeon.connector.clone()))
}

/// Moves a pigeon into another flock, keeping both flocks' cached
/// `pigeon_ids` in step with the pigeon's own new `flock_id` -- the pigeon
/// list fetches by the flock's id list, so leaving it stale would show the
/// pigeon in neither flock until the next sign-in. `Err` carries the
/// server's message: a cross-owner move and an unmanaged destination are
/// each a distinct refusal the user needs to read.
pub async fn move_to_flock(pigeon_id: &str, flock_id: Uuid) -> Result<Pigeon, String> {
  let mut path = String::with_capacity(79);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/flock");

  let Some(body) = to_body(&PigeonFlockUpdateRequest { flock_id }) else {
    return Err("Failed to encode request".to_string());
  };
  let Some(response) = fetch_json_any_status("PUT", &path, Some(&body)).await else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  let Some(pigeon) = parse::<Pigeon>(response).await else {
    return Err("Failed to parse response".to_string());
  };

  let local_session = consume_context::<crate::LocalSession>();
  let mut pigeon_list = local_session.pigeons;
  let previous_flock = pigeon_list
    .read()
    .get(pigeon_id)
    .map(|p| p.flock_id)
    .unwrap_or(flock_id);
  pigeon_list.insert(pigeon_id.to_string(), pigeon.clone());
  pigeon_list.write();

  let mut flock_list = local_session.flocks;
  {
    let mut flocks = flock_list.write();
    if let Some(source) = flocks.get_mut(&previous_flock) {
      source.pigeon_ids.retain(|id| id != pigeon_id);
    }
    if let Some(destination) = flocks.get_mut(&flock_id)
      && !destination.pigeon_ids.iter().any(|id| id == pigeon_id)
    {
      destination.pigeon_ids.push(pigeon_id.to_string());
    }
  }

  Ok(pigeon)
}

pub async fn delete(pigeon_id: &str) -> Option<String> {
  let mut path = String::with_capacity(73);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);

  let _response = fetch_json("DELETE", &path, None).await?;
  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  {
    let mut pigeons = pigeon_list.write();
    pigeons.remove(pigeon_id);
  }
  Some(pigeon_id.to_string())
}

/// Rotates this pigeon's credentials, returning the connector as minted.
/// Whole connector for the same reason `create` returns one: a refresh
/// rotates the PSK alongside the token, and this response is the only
/// place either is readable.
pub async fn refresh_token(pigeon_id: &str) -> Option<Connector> {
  let mut path = String::with_capacity(87);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/token/refresh");

  let response = fetch_json("POST", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  let pigeon = serde_wasm_bindgen::from_value::<Pigeon>(json).ok()?;
  let connector = pigeon.connector.clone();

  let mut pigeon_list = consume_context::<crate::LocalSession>().pigeons;
  pigeon_list.insert(pigeon_id.to_string(), pigeon);
  pigeon_list.write();

  Some(connector)
}

pub async fn update_shadow(
  pigeon_id: &str,
  psur: &PigeonShadowUpdateRequest,
) -> Option<PigeonShadow> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/shadow");

  let json_string = serde_json::to_string(psur).ok()?;
  let body = serde_wasm_bindgen::to_value(&json_string).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  serde_wasm_bindgen::from_value::<PigeonShadow>(json).ok()
}

// GET /pigeons/:id/logs -- every currently-stored device log chunk for
// this pigeon, oldest first, as
// base64-encoded binary (docs/api.md's "Logs" section). This is the only
// pigeon route this crate never mirrors into `LocalSession`: log chunks
// aren't part of `Pigeon`/`PigeonDetail`, so there's no cached field for
// them to update -- callers just hold the returned `Vec` in local component
// state (see `LogViewer` in components/log_viewer.rs).
pub async fn get_logs(pigeon_id: &str) -> Option<Vec<PigeonLogChunk>> {
  let mut path = String::with_capacity(77);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/logs");

  let response = fetch_json("GET", &path, None).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value::<Vec<PigeonLogChunk>>(json).ok()
}

// --- Log dictionary (docs/api.md's "Log dictionary" section) ---
// Like get_logs above, none of these touch `LocalSession`: the dictionary
// isn't part of `Pigeon`/`PigeonDetail`, so `LogViewer` holds it in local
// component state.

/// GET /pigeons/:id/log-dictionary. Outer `None` = request failed; inner
/// `None` = no dictionary uploaded for this pigeon (404 -- the state the
/// viewer shows an upload affordance for, distinct from a real failure).
/// `Some(Some(text))` is the raw `log_dictionary.json` document, handed to
/// `helpers::dict_log::LogDictionary::parse` by the caller.
pub async fn get_log_dictionary(pigeon_id: &str) -> Option<Option<String>> {
  let mut path = String::with_capacity(88);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/log-dictionary");

  let response = fetch_json_any_status("GET", &path, None).await?;
  if response.status() == 404 {
    return Some(None);
  }
  if !response.ok() {
    dioxus::logger::tracing::error!("GET {path} failed with status: {}", response.status());
    return None;
  }

  let text = JsFuture::from(response.text().ok()?).await.ok()?;
  Some(Some(text.as_string()?))
}

/// PUT /pigeons/:id/log-dictionary -- body is the raw log_dictionary.json
/// bytes (same raw-bytes convention as the firmware upload). Returns the
/// server's `LogDictionaryInfo` (size + the build_id/version it found).
pub async fn put_log_dictionary(pigeon_id: &str, bytes: &[u8]) -> Option<LogDictionaryInfo> {
  let mut path = String::with_capacity(88);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/log-dictionary");

  let response = fetch_bytes("PUT", &path, bytes).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;
  serde_wasm_bindgen::from_value::<LogDictionaryInfo>(json).ok()
}

/// DELETE /pigeons/:id/log-dictionary (idempotent server-side).
pub async fn delete_log_dictionary(pigeon_id: &str) -> Option<()> {
  let mut path = String::with_capacity(88);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/log-dictionary");

  fetch_json("DELETE", &path, None).await?;
  Some(())
}

// PUT /pigeons/:id/telemetry-endpoint, mirroring the other per-pigeon PUT
// routes above (e.g. /shadow). Unlike those, the route responds with the
// bare `Option<TelemetryEndpoint>` it just wrote (`Response::from_json(&endpoint)`
// in dovecote's lib.rs) rather than a full `Pigeon` — deserializing this as
// `Pigeon` would fail on every required field and silently collapse every
// call to `None`. Outer `Option` is request success; inner `Option` is "is
// an endpoint configured now" (`None` after clearing). No full `Pigeon`
// comes back, so there's nothing here to refresh `LocalSession` with —
// callers update their own `telemetry_endpoint` field from the return
// value instead (see `TelemetryEndpointModal`'s `on_saved` usage in
// views/pigeon.rs).
pub async fn update_telemetry_endpoint(
  pigeon_id: &str,
  petur: &PigeonTelemetryEndpointUpdateRequest,
) -> Option<Option<TelemetryEndpoint>> {
  let mut path = String::with_capacity(93);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/telemetry-endpoint");

  let body = serde_json::to_string(petur).ok()?;
  let body = serde_wasm_bindgen::to_value(&body).ok()?;
  let response = fetch_json("PUT", &path, Some(&body)).await?;
  let json = JsFuture::from(response.json().ok()?).await.ok()?;

  serde_wasm_bindgen::from_value::<Option<TelemetryEndpoint>>(json).ok()
}

// POST /pigeons/:id/shell -- not a capsules type, deliberately:
// dovecote's own gateway route (`lib.rs`) never deserializes this body
// either, it's a pure passthrough into `proxy_to_pigeon_do`, so there was
// never a shared-crate request/response type to reuse. `ShellExecuteRequest`
// mirrors the JSON body documented in docs/api.md; `ShellExecuteResponse`
// mirrors the DO's `ShellExecuteResponse` (dovecote's
// `objects/pigeons.rs`) field-for-field.
#[derive(serde::Serialize)]
struct ShellExecuteRequest<'a> {
  cmd: &'a str,
  #[serde(skip_serializing_if = "Option::is_none")]
  timeout_ms: Option<u32>,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct ShellExecuteResponse {
  pub output: String,
  pub exit_code: i32,
  pub truncated: bool,
}

/// Every distinct outcome dovecote's `POST /pigeons/:id/shell` can return
/// (docs/api.md's "Shell" section) besides plain success, each with its own
/// user-facing meaning -- this exists specifically so the UI can show a
/// tailored message per case instead of one generic "request failed",
/// which is why this route goes through `fetch_json_any_status` rather
/// than the ok-collapsing `fetch_json` every other function in this file
/// uses. The two 409 cases share a status code but are distinguished by
/// dovecote's plain-text error body (see `execute_shell` below).
#[derive(Debug, Clone, PartialEq)]
pub enum ShellError {
  /// 401 -- the dashboard session died mid-page; not actually reachable
  /// from this owner-gated UI under normal use, but the route can still
  /// 401 before it ever reaches the owner check.
  Unauthenticated,
  /// 403 -- caller isn't this pigeon's owner anymore (e.g. ACL changed in
  /// another tab).
  NotOwner,
  /// 400 -- empty/whitespace-only command. The UI already disables the
  /// Run button for an empty input, so this is a defense-in-depth case,
  /// not the primary guard.
  EmptyCommand,
  /// 409 -- this pigeon has no open device WebSocket right now (offline,
  /// HTTPS/CoAP-only, or just not connected at this moment).
  NotConnected,
  /// 409 -- a previous shell command on this pigeon is still awaiting a
  /// reply (v1 allows one in flight at a time).
  AlreadyInFlight,
  /// 504 -- the device didn't reply within `timeout_ms`.
  Timeout,
  /// 502 -- the device's socket dropped while the command was in flight.
  DeviceDisconnected,
  /// Anything else, including a transport-level failure (offline, CORS,
  /// DNS) where there's no HTTP status at all (`status` is 0 in that
  /// case).
  Unknown { status: u16, body: String },
}

impl std::fmt::Display for ShellError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ShellError::Unauthenticated => {
        write!(f, "Your session has expired. Please log in again.")
      }
      ShellError::NotOwner => {
        write!(f, "You no longer have owner access to this pigeon.")
      }
      ShellError::EmptyCommand => write!(f, "Command cannot be empty."),
      ShellError::NotConnected => write!(
        f,
        "This pigeon has no open device connection right now. A command can only be relayed while the device is connected."
      ),
      ShellError::AlreadyInFlight => write!(
        f,
        "A shell command is already running for this pigeon. Wait for it to finish before running another."
      ),
      ShellError::Timeout => write!(f, "The device did not reply before the timeout elapsed."),
      ShellError::DeviceDisconnected => write!(
        f,
        "The device disconnected while the command was in flight, so no output was returned."
      ),
      ShellError::Unknown { status: 0, .. } => {
        write!(f, "Request failed. Check your connection and try again.")
      }
      ShellError::Unknown { status, body } if body.is_empty() => {
        write!(f, "Request failed (status {status}).")
      }
      ShellError::Unknown { status, body } => write!(f, "Request failed (status {status}): {body}"),
    }
  }
}

pub async fn execute_shell(
  pigeon_id: &str,
  cmd: &str,
  timeout_ms: Option<u32>,
) -> Result<ShellExecuteResponse, ShellError> {
  let mut path = String::with_capacity(80);
  path.push_str("/pigeons/");
  path.push_str(pigeon_id);
  path.push_str("/shell");

  let unknown_transport = || ShellError::Unknown {
    status: 0,
    body: String::new(),
  };

  let json_string = serde_json::to_string(&ShellExecuteRequest { cmd, timeout_ms })
    .map_err(|_| unknown_transport())?;
  let body = serde_wasm_bindgen::to_value(&json_string).map_err(|_| unknown_transport())?;

  let response = fetch_json_any_status("POST", &path, Some(&body))
    .await
    .ok_or_else(unknown_transport)?;

  if response.ok() {
    let json = JsFuture::from(response.json().map_err(|_| unknown_transport())?)
      .await
      .map_err(|_| unknown_transport())?;
    return serde_wasm_bindgen::from_value::<ShellExecuteResponse>(json)
      .map_err(|_| unknown_transport());
  }

  let status = response.status();
  let body_text = match response.text() {
    Ok(promise) => JsFuture::from(promise)
      .await
      .ok()
      .and_then(|v| v.as_string())
      .unwrap_or_default(),
    Err(_) => String::new(),
  };

  Err(match status {
    401 => ShellError::Unauthenticated,
    403 => ShellError::NotOwner,
    400 => ShellError::EmptyCommand,
    409 if body_text.contains("already in flight") => ShellError::AlreadyInFlight,
    409 => ShellError::NotConnected,
    502 => ShellError::DeviceDisconnected,
    504 => ShellError::Timeout,
    _ => ShellError::Unknown {
      status,
      body: body_text,
    },
  })
}
