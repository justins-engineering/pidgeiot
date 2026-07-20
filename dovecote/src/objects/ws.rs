use capsules::PigeonShadow;
use serde::{Deserialize, Serialize};

/// Max size of a single WebSocket text frame from a device (task #32) --
/// generous headroom over a typical telemetry/shadow-report payload, not a
/// tuned limit; mirrors `capsules::MAX_LOG_CHUNK_BYTES`'s sizing rationale.
/// Not itself exported via `capsules` (unlike that constant) because the
/// current device-side counterpart is Zephyr/C firmware (see `~/pigeon`),
/// which can't consume a Rust crate anyway -- the value is documented in
/// `docs/api.md` instead, for whoever implements the device-side client
/// (task #33).
pub const MAX_WS_FRAME_BYTES: usize = 16 * 1024;

/// Sliding-window frame-flood limit enforced per socket (task #32). State
/// lives in the socket's own hibernation attachment (see
/// `check_rate_limit` below), not this DO's SQLite or an in-memory struct
/// field -- neither survives a hibernation eviction between messages, but
/// the attachment does (that's exactly what it's for).
pub const WS_RATE_LIMIT_WINDOW_MS: i64 = 10_000;
pub const WS_RATE_LIMIT_MAX_FRAMES: u32 = 50;

/// Tag applied to every device-class socket accepted by
/// `accept_websocket_device` (`objects/pigeons.rs`). Scoping "close the old
/// socket"/"broadcast a shadow push" to this tag (via
/// `State::get_websockets_with_tag`), rather than every socket on the DO,
/// is what keeps this extensible for a second, differently-tagged socket
/// class -- the remote-shell relay (task #34) -- without either class
/// accidentally closing or receiving the other's frames.
pub const WS_DEVICE_TAG: &str = "device";

/// Frames a device may send over its WebSocket (task #32). `#[serde(tag =
/// "type", rename_all = "snake_case")]` produces exactly the wire shapes
/// documented in `docs/api.md`: `Telemetry` <-> `{"type":"telemetry",...}`,
/// `ShadowReport` <-> `{"type":"shadow_report",...}`, etc.
#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsInboundFrame {
  Telemetry {
    metrics: std::collections::HashMap<String, String>,
  },
  ShadowReport {
    current_version: i32,
    current_config: serde_json::Value,
  },
  Ping,
  Pong,
  // Reply to a server-sent `ShellCmd` (task #34, v1 -- request/response
  // only, no interactive/streaming shell, see the design doc). `exit_code`
  // is always present (`shell_execute_cmd()`'s own return value is never
  // null). `truncated` compensates for `shell_dummy`'s output buffer
  // silently dropping overflow bytes with no signal of its own -- the
  // device sets this when its local output buffer filled, so an operator
  // reading `output` knows it might be incomplete.
  ShellOutput {
    request_id: String,
    output: String,
    exit_code: i32,
    truncated: bool,
  },
}

/// Frames the server may push to a connected device (task #32).
/// `ShadowUpdate` is the headline win this endpoint exists for -- pushed
/// immediately from `update_shadow` whenever a dashboard `PUT` lands,
/// instead of the device having to poll for it.
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsOutboundFrame {
  ShadowUpdate {
    shadow: PigeonShadow,
  },
  // Kept for protocol symmetry with `WsInboundFrame::Ping` and documented
  // in `docs/api.md`, but nothing constructs it yet -- this DO has no
  // alarm-driven periodic keepalive; today the server only ever *responds*
  // to a device-initiated `ping` with `pong`, never initiates one itself.
  #[allow(dead_code)]
  Ping,
  Pong,
  // Triggers a diagnostic shell command on the device (task #34, v1),
  // sent from `execute_shell_command` (`objects/pigeons.rs`) in response
  // to a dashboard `POST /pigeons/:id/shell`. `request_id` is a plain
  // correlation token, not a security boundary -- the auth gate is the
  // owner-only check before this frame is ever sent, not the request_id's
  // guessability. Devices without shell support compiled in
  // (`CONFIG_PIGEON_SHELL`) silently ignore this frame type via the
  // existing forward-compat fallthrough in the device's own frame
  // dispatch, so old/unsupporting firmware in the field is unaffected.
  ShellCmd {
    request_id: String,
    cmd: String,
  },
}

#[derive(Serialize, Deserialize, Default)]
struct WsRateLimitState {
  window_start_ms: i64,
  frame_count: u32,
}

/// Sliding-window flood check for one inbound frame on `ws`. Reads/writes
/// the socket's hibernation attachment (`serialize_attachment`/
/// `deserialize_attachment`) rather than any state on the `Pigeons` struct
/// itself, since the DO can be evicted and re-woken between any two
/// messages on a hibernating socket -- ordinary struct fields don't survive
/// that, the attachment does. Returns `false` once `frame_count` exceeds
/// `WS_RATE_LIMIT_MAX_FRAMES` within the current `WS_RATE_LIMIT_WINDOW_MS`
/// window; the caller is expected to close the connection when this
/// happens.
pub fn check_rate_limit(ws: &worker::WebSocket) -> bool {
  let now = worker::Date::now().as_millis() as i64;

  let mut state = ws
    .deserialize_attachment::<WsRateLimitState>()
    .ok()
    .flatten()
    .unwrap_or_default();

  if now - state.window_start_ms > WS_RATE_LIMIT_WINDOW_MS {
    state.window_start_ms = now;
    state.frame_count = 0;
  }

  state.frame_count += 1;
  let within_limit = state.frame_count <= WS_RATE_LIMIT_MAX_FRAMES;

  // Best-effort: if this fails the next call just starts a fresh window,
  // which only makes the limit more lenient, never less -- not worth
  // failing the frame over.
  let _ = ws.serialize_attachment(&state);

  within_limit
}
