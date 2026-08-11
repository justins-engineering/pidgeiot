//! Transport-agnostic CoAP request handling: maps the CoAP device surface
//! 1:1 onto dovecote's `/device/pigeons/:id/*` HTTP routes (docs/api.md,
//! "CoAP device surface").
//!
//! Authorization model: by the time a request reaches this handler the
//! DTLS/TLS PSK handshake has already authenticated the peer as exactly one
//! pigeon (`DeviceSession.pigeon_id` = the PSK identity), and the session's
//! `token` is that pigeon's own bearer token (see src/psk.rs). Every
//! upstream call presents that bearer token, which the pigeon's Durable
//! Object verifies cryptographically -- this process adds exactly one check
//! of its own: the pigeon id embedded in the request's Uri-Path MUST equal
//! the handshake identity (a device can never address another pigeon's
//! resources, even with a syntactically valid path).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::coap::block::{self, Block, MAX_SZX};
use crate::coap::message::{Message, code, content_format, option};
use crate::quota::MAX_CONNECTIONS;
use crate::upstream::{Method, Upstream, UpstreamResponse};

/// Options we understand; any *critical* option outside this list gets a
/// 4.02 Bad Option (RFC 7252 section 5.4.1). Elective unknowns are ignored.
const KNOWN_OPTIONS: &[u16] = &[
  option::URI_HOST,
  option::ETAG,
  option::OBSERVE,
  option::URI_PORT,
  option::URI_PATH,
  option::CONTENT_FORMAT,
  option::URI_QUERY,
  option::BLOCK2,
  option::BLOCK1,
  option::SIZE2,
  option::SIZE1,
];

/// Above this, a UDP response body is spontaneously Block2-fragmented even
/// when the client didn't ask (szx 6 = 1024-byte blocks) -- large
/// datagrams mean IP fragmentation, which is exactly what block-wise
/// transfer exists to avoid. TCP responses (RFC 8323 frames) are sent
/// whole unless the client asked for Block2, matching the minimal
/// `~/pigeon` client, which reads one frame and speaks no Block2.
const UDP_SPONTANEOUS_BLOCK_THRESHOLD: usize = 1024;

/// Cap on a Block1-reassembled request body. Generous over the largest
/// legitimate device write (16 KiB log chunks).
const MAX_BLOCK1_BODY: usize = 64 * 1024;

/// A Block1 reassembly that saw no new block for this long is dropped.
const BLOCK1_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Hard ceiling on concurrent Block1 reassemblies, independent of how many
/// distinct peers or leaves ever start one. Tied to MAX_CONNECTIONS (the
/// same ceiling the listeners already enforce) so the worst case stays
/// easy to reason about: MAX_BLOCK1_ENTRIES * MAX_BLOCK1_BODY tops out
/// around 256MiB rather than growing with however many peer addresses or
/// leaf paths a connection's lifetime has touched.
const MAX_BLOCK1_ENTRIES: usize = MAX_CONNECTIONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
  Udp,
  Tcp,
}

/// The identity a PSK handshake established for one connection/association.
#[derive(Debug, Clone)]
pub struct DeviceSession {
  /// PSK identity == the pigeon's id (its Durable Object id).
  pub pigeon_id: String,
  /// This pigeon's device bearer token, presented on every upstream call.
  pub token: String,
  /// Peer address string; Block1 reassembly key component.
  pub peer: String,
}

struct Reassembly {
  data: Vec<u8>,
  next_num: u32,
  touched: Instant,
}

pub struct Handler<U> {
  upstream: U,
  block1: Mutex<HashMap<(String, String), Reassembly>>,
}

impl<U: Upstream> Handler<U> {
  pub fn new(upstream: U) -> Handler<U> {
    Handler {
      upstream,
      block1: Mutex::new(HashMap::new()),
    }
  }

  /// Handles one CoAP request, returning the response `Message`. The
  /// transport layer wraps it (ACK/NON for UDP, a plain frame for TCP).
  pub async fn handle(
    &self,
    req: &Message,
    session: &DeviceSession,
    transport: Transport,
  ) -> Message {
    // Reclaim idle Block1 reassemblies on every request, not only ones
    // that themselves carry Block1 -- otherwise a terminator that stops
    // seeing Block1 traffic (the common case once uploads finish) never
    // drains its own leftovers.
    self.sweep_block1();

    if !code::is_request(req.code) {
      return diagnostic(req, code::BAD_REQUEST, "not a request");
    }

    if let Some(number) = req.unknown_critical_option(KNOWN_OPTIONS) {
      tracing::debug!(number, "rejecting unknown critical option");
      return diagnostic(req, code::BAD_OPTION, "unrecognized critical option");
    }

    let segments = req.path_segments();
    let [seg_device, seg_pigeons, pigeon_id, leaf] = segments.as_slice() else {
      return diagnostic(req, code::NOT_FOUND, "no such resource");
    };
    if seg_device != "device" || seg_pigeons != "pigeons" {
      return diagnostic(req, code::NOT_FOUND, "no such resource");
    }

    if pigeon_id != &session.pigeon_id {
      tracing::warn!(
        identity = %session.pigeon_id,
        requested = %pigeon_id,
        "URI pigeon id does not match handshake identity"
      );
      return diagnostic(req, code::FORBIDDEN, "pigeon id does not match identity");
    }

    match (leaf.as_str(), req.code) {
      ("shadow", code::GET) => {
        let resp = self
          .upstream_call(session, Method::Get, "shadow", None, None)
          .await;
        self.json_response(req, resp, Method::Get, transport)
      }
      ("shadow", code::POST) => match self.assemble_body(req, session, "shadow") {
        BodyState::Complete(body) => {
          let resp = self
            .upstream_call(
              session,
              Method::Post,
              "shadow",
              None,
              Some((body, "application/json")),
            )
            .await;
          let mut out = self.json_response(req, resp, Method::Post, transport);
          echo_final_block1(req, &mut out);
          out
        }
        BodyState::Interim(msg) => msg,
      },
      ("telemetry", code::POST) => match self.assemble_body(req, session, "telemetry") {
        BodyState::Complete(body) => {
          let resp = self
            .upstream_call(
              session,
              Method::Post,
              "telemetry",
              None,
              Some((body, "application/json")),
            )
            .await;
          let mut out = self.json_response(req, resp, Method::Post, transport);
          echo_final_block1(req, &mut out);
          out
        }
        BodyState::Interim(msg) => msg,
      },
      ("logs", code::POST) => match self.assemble_body(req, session, "logs") {
        BodyState::Complete(body) => {
          let resp = self
            .upstream_call(
              session,
              Method::Post,
              "logs",
              None,
              Some((body, "application/octet-stream")),
            )
            .await;
          let mut out = match resp {
            Ok(r) if r.status < 300 => Message::response(code::CHANGED, req),
            other => self.error_response(req, other),
          };
          echo_final_block1(req, &mut out);
          out
        }
        BodyState::Interim(msg) => msg,
      },
      ("firmware", code::GET) => self.firmware(req, session).await,
      ("shadow" | "telemetry" | "logs" | "firmware", _) => {
        diagnostic(req, code::METHOD_NOT_ALLOWED, "method not allowed")
      }
      _ => diagnostic(req, code::NOT_FOUND, "no such resource"),
    }
  }

  async fn upstream_call(
    &self,
    session: &DeviceSession,
    method: Method,
    leaf: &str,
    range: Option<(u64, u64)>,
    body: Option<(Vec<u8>, &'static str)>,
  ) -> Result<UpstreamResponse, String> {
    self
      .upstream
      .device_request(
        method,
        &session.pigeon_id,
        leaf,
        &session.token,
        range,
        body,
      )
      .await
  }

  /// JSON-bodied routes: map the upstream status, attach the JSON payload,
  /// and apply response-side Block2 (explicit, or spontaneous on UDP).
  fn json_response(
    &self,
    req: &Message,
    resp: Result<UpstreamResponse, String>,
    method: Method,
    transport: Transport,
  ) -> Message {
    let resp = match resp {
      Ok(r) if r.status < 300 => r,
      other => return self.error_response(req, other),
    };

    let success = match (method, resp.status) {
      (_, 201) => code::CREATED,
      (Method::Get, _) => code::CONTENT,
      (Method::Post, _) => code::CHANGED,
    };

    let mut out = Message::response(success, req);
    let full = resp.body;

    let requested = req.option_uint(option::BLOCK2).and_then(Block::decode);

    let block = match requested {
      Some(b) => Some(b),
      None if transport == Transport::Udp && full.len() > UDP_SPONTANEOUS_BLOCK_THRESHOLD => {
        Some(Block {
          num: 0,
          more: false,
          szx: MAX_SZX,
        })
      }
      None => None,
    };

    match block {
      None => {
        out.set_option_uint(option::CONTENT_FORMAT, u32::from(content_format::JSON));
        out.payload = full;
      }
      Some(b) => {
        let start = b.offset() as usize;
        // num > 0 past the end is a client error; num == 0 against an
        // empty body legitimately yields an empty final block.
        if start >= full.len() && b.num > 0 {
          return diagnostic(req, code::BAD_REQUEST, "block out of range");
        }
        let end = (start + b.size()).min(full.len());
        out.set_option_uint(option::CONTENT_FORMAT, u32::from(content_format::JSON));
        out.set_option_uint(
          option::BLOCK2,
          Block {
            num: b.num,
            more: end < full.len(),
            szx: b.szx,
          }
          .encode(),
        );
        if b.num == 0 {
          out.set_option_uint(option::SIZE2, full.len() as u32);
        }
        out.payload = full.get(start..end).unwrap_or_default().to_vec();
      }
    }
    out
  }

  /// Firmware download: Block2 maps directly onto dovecote's HTTP Range
  /// support (each block is one ranged GET; nothing is buffered here), so
  /// a ~500 KiB image never transits this process as a whole. Serving is
  /// ALWAYS block-wise, on both transports -- a client that sent no Block2
  /// gets block 0 with the more-bit set (RFC 7959 "spontaneous" Block2)
  /// and continues from there.
  async fn firmware(&self, req: &Message, session: &DeviceSession) -> Message {
    let block = req
      .option_uint(option::BLOCK2)
      .and_then(Block::decode)
      .unwrap_or(Block {
        num: 0,
        more: false,
        szx: MAX_SZX,
      });

    let (start, end) = block.byte_range();
    let resp = match self
      .upstream_call(session, Method::Get, "firmware", Some((start, end)), None)
      .await
    {
      Ok(r) if r.status < 300 => r,
      other => return self.error_response(req, other),
    };

    // Total size: Content-Range's authoritative total, else the
    // X-Firmware-Size header (200 whole-image responses).
    let content_range = resp
      .content_range
      .as_deref()
      .and_then(block::parse_content_range);
    let total = content_range
      .map(|(_, _, total)| total)
      .or(resp.firmware_size)
      .unwrap_or(resp.body.len() as u64);

    if let Some((got_start, _, _)) = content_range
      && got_start != start
    {
      // Upstream clamped an out-of-range block rather than 416ing.
      return diagnostic(req, code::BAD_REQUEST, "block out of range");
    }
    if start >= total && total > 0 {
      return diagnostic(req, code::BAD_REQUEST, "block out of range");
    }

    // A 200 means the whole image came back (no Range honored) -- slice
    // the requested block out locally rather than shipping it all. Safe
    // slicing: an upstream whose body is shorter than its own declared
    // total must not be able to panic this connection's thread.
    let payload = if resp.status == 200 && resp.body.len() as u64 > (end - start + 1) {
      let s = (start as usize).min(resp.body.len());
      let e = ((end + 1) as usize).min(resp.body.len());
      resp.body.get(s..e).unwrap_or_default().to_vec()
    } else {
      resp.body
    };

    let mut out = Message::response(code::CONTENT, req);
    out.set_option_uint(
      option::CONTENT_FORMAT,
      u32::from(content_format::OCTET_STREAM),
    );
    out.set_option_uint(
      option::BLOCK2,
      Block {
        num: block.num,
        more: block::more_after(&block, total),
        szx: block.szx,
      }
      .encode(),
    );
    if block.num == 0 {
      out.set_option_uint(option::SIZE2, total as u32);
    }
    // ETag on EVERY block, not just the first: RFC 7959 keys
    // representation consistency on it, and libcoap aborts a transfer
    // whose later blocks drop the ETag the first block carried ("Not all
    // blocks have ETag option"). The image sha256's first 8 bytes is
    // stable and content-addressed.
    if let Some(etag) = resp
      .firmware_sha256
      .as_deref()
      .and_then(|hex| hex_prefix_bytes(hex, 8))
    {
      out.push_option(option::ETAG, etag);
    }
    out.payload = payload;
    out
  }

  /// Drops Block1 reassemblies that have seen no new block in
  /// `BLOCK1_IDLE_TIMEOUT`. Called on every request so this runs whether
  /// or not the request itself carries Block1.
  fn sweep_block1(&self) {
    let now = Instant::now();
    self
      .block1
      .lock()
      .expect("block1 lock")
      .retain(|_, r| now.duration_since(r.touched) < BLOCK1_IDLE_TIMEOUT);
  }

  /// Block1 (request-body) reassembly. Returns the complete body (the
  /// common no-Block1 case is just the request payload), or the interim
  /// 2.31 Continue / error response to send instead.
  fn assemble_body(&self, req: &Message, session: &DeviceSession, leaf: &str) -> BodyState {
    let Some(b) = req.option_uint(option::BLOCK1).and_then(Block::decode) else {
      return BodyState::Complete(req.payload.clone());
    };

    let key = (session.peer.clone(), leaf.to_string());
    let mut map = self.block1.lock().expect("block1 lock");
    let now = Instant::now();

    if b.num == 0 {
      // A restart of an already-tracked upload (same peer+leaf) just
      // overwrites its entry, so only a genuinely new key needs to clear
      // the cap -- otherwise a peer retrying its own upload could be
      // starved by unrelated entries that filled the table after it.
      if !map.contains_key(&key) && map.len() >= MAX_BLOCK1_ENTRIES {
        return BodyState::Interim(diagnostic(
          req,
          code::SERVICE_UNAVAILABLE,
          "block1 reassembly table full",
        ));
      }
      map.insert(
        key.clone(),
        Reassembly {
          data: Vec::new(),
          next_num: 0,
          touched: now,
        },
      );
    }

    let Some(entry) = map.get_mut(&key) else {
      return BodyState::Interim(diagnostic(
        req,
        code::REQUEST_ENTITY_INCOMPLETE,
        "no block context",
      ));
    };
    if entry.next_num != b.num {
      map.remove(&key);
      return BodyState::Interim(diagnostic(
        req,
        code::REQUEST_ENTITY_INCOMPLETE,
        "block out of sequence",
      ));
    }

    if entry.data.len() + req.payload.len() > MAX_BLOCK1_BODY {
      map.remove(&key);
      return BodyState::Interim(diagnostic(
        req,
        code::REQUEST_ENTITY_TOO_LARGE,
        "body too large",
      ));
    }

    entry.data.extend_from_slice(&req.payload);
    entry.next_num += 1;
    entry.touched = now;

    if b.more {
      let mut cont = Message::response(code::CONTINUE, req);
      cont.set_option_uint(option::BLOCK1, b.encode());
      BodyState::Interim(cont)
    } else {
      let body = map.remove(&key).map(|r| r.data).unwrap_or_default();
      BodyState::Complete(body)
    }
  }

  fn error_response(&self, req: &Message, resp: Result<UpstreamResponse, String>) -> Message {
    match resp {
      Ok(r) => {
        let coap_code = match r.status {
          400 => code::BAD_REQUEST,
          401 => code::UNAUTHORIZED,
          403 => code::FORBIDDEN,
          404 => code::NOT_FOUND,
          405 => code::METHOD_NOT_ALLOWED,
          413 => code::REQUEST_ENTITY_TOO_LARGE,
          500..=599 => code::BAD_GATEWAY,
          _ => code::BAD_REQUEST,
        };
        let mut out = Message::response(coap_code, req);
        // Diagnostic payload (RFC 7252 section 5.5.2): upstream's error
        // text, capped.
        out.payload = r.body.into_iter().take(128).collect();
        out
      }
      Err(e) => {
        tracing::error!(error = %e, "upstream request failed");
        diagnostic(req, code::GATEWAY_TIMEOUT, "upstream unreachable")
      }
    }
  }
}

enum BodyState {
  Complete(Vec<u8>),
  Interim(Message),
}

/// On the final response of a Block1-carried request, echo the final Block1
/// option back (RFC 7959 section 2.5).
fn echo_final_block1(req: &Message, out: &mut Message) {
  if let Some(v) = req.option_uint(option::BLOCK1)
    && let Some(b) = Block::decode(v)
    && !b.more
  {
    out.set_option_uint(option::BLOCK1, b.encode());
  }
}

fn diagnostic(req: &Message, coap_code: u8, text: &str) -> Message {
  let mut out = Message::response(coap_code, req);
  out.payload = text.as_bytes().to_vec();
  out
}

fn hex_prefix_bytes(hex: &str, n: usize) -> Option<Vec<u8>> {
  let take = hex.get(..n * 2)?;
  (0..take.len())
    .step_by(2)
    .map(|i| u8::from_str_radix(&take[i..i + 2], 16).ok())
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Mutex as StdMutex;

  #[derive(Debug, Clone, PartialEq)]
  struct Call {
    method: Method,
    pigeon_id: String,
    leaf: String,
    bearer: String,
    range: Option<(u64, u64)>,
    body: Option<(Vec<u8>, &'static str)>,
  }

  struct MockUpstream {
    calls: StdMutex<Vec<Call>>,
    respond: Box<dyn Fn(&Call) -> Result<UpstreamResponse, String> + Send + Sync>,
  }

  impl MockUpstream {
    fn new(
      respond: impl Fn(&Call) -> Result<UpstreamResponse, String> + Send + Sync + 'static,
    ) -> MockUpstream {
      MockUpstream {
        calls: StdMutex::new(Vec::new()),
        respond: Box::new(respond),
      }
    }
  }

  impl Upstream for &MockUpstream {
    async fn device_request(
      &self,
      method: Method,
      pigeon_id: &str,
      leaf: &str,
      bearer: &str,
      range: Option<(u64, u64)>,
      body: Option<(Vec<u8>, &'static str)>,
    ) -> Result<UpstreamResponse, String> {
      let call = Call {
        method,
        pigeon_id: pigeon_id.to_string(),
        leaf: leaf.to_string(),
        bearer: bearer.to_string(),
        range,
        body,
      };
      let result = (self.respond)(&call);
      self.calls.lock().unwrap().push(call);
      result
    }
  }

  fn session() -> DeviceSession {
    DeviceSession {
      pigeon_id: "pigeon-1".into(),
      token: "tok-1".into(),
      peer: "10.0.0.1:1234".into(),
    }
  }

  fn request(coap_code: u8, id: &str, leaf: &str) -> Message {
    let mut msg = Message {
      code: coap_code,
      token: vec![9],
      ..Default::default()
    };
    for seg in ["device", "pigeons", id, leaf] {
      msg.push_option(option::URI_PATH, seg.as_bytes().to_vec());
    }
    msg
  }

  fn json_ok(body: &str) -> UpstreamResponse {
    UpstreamResponse {
      status: 200,
      body: body.as_bytes().to_vec(),
      ..Default::default()
    }
  }

  #[tokio::test]
  async fn shadow_get_maps_to_content() {
    let mock = MockUpstream::new(|_| Ok(json_ok("{\"target_version\":3}")));
    let handler = Handler::new(&mock);

    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "shadow"),
        &session(),
        Transport::Tcp,
      )
      .await;

    assert_eq!(out.code, code::CONTENT);
    assert_eq!(out.payload, b"{\"target_version\":3}");
    assert_eq!(
      out.option_uint(option::CONTENT_FORMAT),
      Some(u32::from(content_format::JSON))
    );

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, Method::Get);
    assert_eq!(calls[0].leaf, "shadow");
    assert_eq!(calls[0].pigeon_id, "pigeon-1");
    assert_eq!(calls[0].bearer, "tok-1");
  }

  #[tokio::test]
  async fn uri_pigeon_id_must_match_handshake_identity() {
    let mock = MockUpstream::new(|_| Ok(json_ok("{}")));
    let handler = Handler::new(&mock);

    let out = handler
      .handle(
        &request(code::GET, "pigeon-OTHER", "shadow"),
        &session(),
        Transport::Tcp,
      )
      .await;

    assert_eq!(out.code, code::FORBIDDEN);
    assert!(mock.calls.lock().unwrap().is_empty(), "no upstream call");
  }

  #[tokio::test]
  async fn unknown_paths_and_methods() {
    let mock = MockUpstream::new(|_| Ok(json_ok("{}")));
    let handler = Handler::new(&mock);

    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "nonsense"),
        &session(),
        Transport::Tcp,
      )
      .await;
    assert_eq!(out.code, code::NOT_FOUND);

    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "telemetry"),
        &session(),
        Transport::Tcp,
      )
      .await;
    assert_eq!(out.code, code::METHOD_NOT_ALLOWED);

    let mut bad_prefix = Message {
      code: code::GET,
      ..Default::default()
    };
    for seg in ["evil", "pigeons", "pigeon-1", "shadow"] {
      bad_prefix.push_option(option::URI_PATH, seg.as_bytes().to_vec());
    }
    let out = handler
      .handle(&bad_prefix, &session(), Transport::Tcp)
      .await;
    assert_eq!(out.code, code::NOT_FOUND);

    assert!(mock.calls.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn unknown_critical_option_rejected() {
    let mock = MockUpstream::new(|_| Ok(json_ok("{}")));
    let handler = Handler::new(&mock);

    let mut req = request(code::GET, "pigeon-1", "shadow");
    req.push_option(35, b"coap://evil".to_vec()); // Proxy-Uri, critical
    let out = handler.handle(&req, &session(), Transport::Tcp).await;
    assert_eq!(out.code, code::BAD_OPTION);
    assert!(mock.calls.lock().unwrap().is_empty());
  }

  #[tokio::test]
  async fn upstream_401_maps_to_coap_unauthorized() {
    let mock = MockUpstream::new(|_| {
      Ok(UpstreamResponse {
        status: 401,
        body: b"Unauthorized".to_vec(),
        ..Default::default()
      })
    });
    let handler = Handler::new(&mock);
    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "shadow"),
        &session(),
        Transport::Tcp,
      )
      .await;
    assert_eq!(out.code, code::UNAUTHORIZED);
  }

  #[tokio::test]
  async fn upstream_unreachable_maps_to_gateway_timeout() {
    let mock = MockUpstream::new(|_| Err("connect refused".into()));
    let handler = Handler::new(&mock);
    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "shadow"),
        &session(),
        Transport::Tcp,
      )
      .await;
    assert_eq!(out.code, code::GATEWAY_TIMEOUT);
  }

  #[tokio::test]
  async fn telemetry_post_forwards_json_body() {
    let mock = MockUpstream::new(|_| {
      Ok(UpstreamResponse {
        status: 202,
        body: b"{}".to_vec(),
        ..Default::default()
      })
    });
    let handler = Handler::new(&mock);

    let mut req = request(code::POST, "pigeon-1", "telemetry");
    req.payload = b"{\"temp\":\"21.5\"}".to_vec();
    let out = handler.handle(&req, &session(), Transport::Udp).await;

    assert_eq!(out.code, code::CHANGED);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
      calls[0].body,
      Some((b"{\"temp\":\"21.5\"}".to_vec(), "application/json"))
    );
  }

  fn firmware_upstream(image: &'static [u8]) -> MockUpstream {
    MockUpstream::new(move |call: &Call| {
      let (start, end) = call.range.expect("firmware calls are always ranged");
      let start = start as usize;
      let end = (end as usize + 1).min(image.len());
      assert!(start < image.len(), "upstream got out-of-range start");
      Ok(UpstreamResponse {
        status: 206,
        body: image[start..end].to_vec(),
        content_range: Some(format!("bytes {}-{}/{}", start, end - 1, image.len())),
        firmware_size: Some(image.len() as u64),
        firmware_sha256: Some("aabbccddeeff00112233445566778899".repeat(2)),
        ..Default::default()
      })
    })
  }

  #[tokio::test]
  async fn firmware_block2_sequence() {
    // 2500-byte image -> 3 blocks at szx 6 (1024).
    static IMAGE: [u8; 2500] = [0x7E; 2500];
    let mock = firmware_upstream(&IMAGE);
    let handler = Handler::new(&mock);

    // Block 0 (explicit).
    let mut req = request(code::GET, "pigeon-1", "firmware");
    req.set_option_uint(
      option::BLOCK2,
      Block {
        num: 0,
        more: false,
        szx: 6,
      }
      .encode(),
    );
    let out = handler.handle(&req, &session(), Transport::Udp).await;
    assert_eq!(out.code, code::CONTENT);
    assert_eq!(out.payload.len(), 1024);
    let b = Block::decode(out.option_uint(option::BLOCK2).unwrap()).unwrap();
    assert!(b.more);
    assert_eq!(out.option_uint(option::SIZE2), Some(2500));
    assert_eq!(
      out.first_option(option::ETAG),
      Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11].as_slice())
    );

    // Block 2 (final, short).
    let mut req = request(code::GET, "pigeon-1", "firmware");
    req.set_option_uint(
      option::BLOCK2,
      Block {
        num: 2,
        more: false,
        szx: 6,
      }
      .encode(),
    );
    let out = handler.handle(&req, &session(), Transport::Udp).await;
    assert_eq!(out.payload.len(), 2500 - 2048);
    let b = Block::decode(out.option_uint(option::BLOCK2).unwrap()).unwrap();
    assert!(!b.more);
    // Size2 only rides on block 0; ETag rides on every block (libcoap
    // aborts a transfer whose later blocks drop it).
    assert_eq!(out.option_uint(option::SIZE2), None);
    assert!(out.first_option(option::ETAG).is_some());

    // Upstream saw exactly the two mapped ranges.
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls[0].range, Some((0, 1023)));
    assert_eq!(calls[1].range, Some((2048, 3071)));
  }

  #[tokio::test]
  async fn firmware_without_block2_starts_spontaneous_blockwise() {
    static IMAGE: [u8; 2500] = [0x11; 2500];
    let mock = firmware_upstream(&IMAGE);
    let handler = Handler::new(&mock);

    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "firmware"),
        &session(),
        Transport::Tcp,
      )
      .await;
    assert_eq!(out.code, code::CONTENT);
    assert_eq!(out.payload.len(), 1024);
    let b = Block::decode(out.option_uint(option::BLOCK2).unwrap()).unwrap();
    assert_eq!((b.num, b.more), (0, true));
  }

  #[tokio::test]
  async fn firmware_block_out_of_range() {
    static IMAGE: [u8; 100] = [0x22; 100];
    // Upstream clamps rather than 416s (dovecote behavior) -- mock that.
    let mock = MockUpstream::new(move |_| {
      Ok(UpstreamResponse {
        status: 206,
        body: IMAGE[99..].to_vec(),
        content_range: Some("bytes 99-99/100".into()),
        firmware_size: Some(100),
        ..Default::default()
      })
    });
    let handler = Handler::new(&mock);

    let mut req = request(code::GET, "pigeon-1", "firmware");
    req.set_option_uint(
      option::BLOCK2,
      Block {
        num: 5,
        more: false,
        szx: 6,
      }
      .encode(),
    );
    let out = handler.handle(&req, &session(), Transport::Udp).await;
    assert_eq!(out.code, code::BAD_REQUEST);
  }

  #[tokio::test]
  async fn big_json_udp_response_is_spontaneously_blocked() {
    let big = format!("{{\"blob\":\"{}\"}}", "x".repeat(4000));
    let big_clone = big.clone();
    let mock = MockUpstream::new(move |_| Ok(json_ok(&big_clone)));
    let handler = Handler::new(&mock);

    // UDP: fragmented.
    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "shadow"),
        &session(),
        Transport::Udp,
      )
      .await;
    assert_eq!(out.payload.len(), 1024);
    let b = Block::decode(out.option_uint(option::BLOCK2).unwrap()).unwrap();
    assert!((b.num, b.more) == (0, true));
    assert_eq!(out.option_uint(option::SIZE2), Some(big.len() as u32));

    // Client asks for block 3 explicitly -> final slice.
    let mut req = request(code::GET, "pigeon-1", "shadow");
    req.set_option_uint(
      option::BLOCK2,
      Block {
        num: 3,
        more: false,
        szx: 6,
      }
      .encode(),
    );
    let out = handler.handle(&req, &session(), Transport::Udp).await;
    assert_eq!(out.payload.len(), big.len() - 3072);
    let b = Block::decode(out.option_uint(option::BLOCK2).unwrap()).unwrap();
    assert!(!b.more);

    // TCP: whole frame, no Block2, matching the minimal ~/pigeon client.
    let out = handler
      .handle(
        &request(code::GET, "pigeon-1", "shadow"),
        &session(),
        Transport::Tcp,
      )
      .await;
    assert_eq!(out.payload.len(), big.len());
    assert_eq!(out.option_uint(option::BLOCK2), None);
  }

  #[tokio::test]
  async fn block1_reassembly_roundtrip() {
    let mock = MockUpstream::new(|_| {
      Ok(UpstreamResponse {
        status: 200,
        ..Default::default()
      })
    });
    let handler = Handler::new(&mock);
    let sess = session();

    let mut part = |num: u32, more: bool, payload: &[u8]| {
      let mut req = request(code::POST, "pigeon-1", "logs");
      req.set_option_uint(option::BLOCK1, Block { num, more, szx: 4 }.encode());
      req.payload = payload.to_vec();
      req
    };

    let out = handler
      .handle(&part(0, true, &[0xAA; 256]), &sess, Transport::Udp)
      .await;
    assert_eq!(out.code, code::CONTINUE);
    let out = handler
      .handle(&part(1, true, &[0xBB; 256]), &sess, Transport::Udp)
      .await;
    assert_eq!(out.code, code::CONTINUE);
    assert!(mock.calls.lock().unwrap().is_empty(), "not forwarded yet");

    let out = handler
      .handle(&part(2, false, &[0xCC; 100]), &sess, Transport::Udp)
      .await;
    assert_eq!(out.code, code::CHANGED);
    // Final response echoes the final Block1 (RFC 7959 section 2.5).
    let b = Block::decode(out.option_uint(option::BLOCK1).unwrap()).unwrap();
    assert_eq!((b.num, b.more), (2, false));

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (body, ct) = calls[0].body.clone().unwrap();
    assert_eq!(body.len(), 612);
    assert_eq!(&body[..256], &[0xAA; 256]);
    assert_eq!(&body[512..], &[0xCC; 100]);
    assert_eq!(ct, "application/octet-stream");
  }

  #[tokio::test]
  async fn block1_table_full_refuses_a_new_peer_but_not_one_already_admitted() {
    let mock = MockUpstream::new(|_| {
      Ok(UpstreamResponse {
        status: 200,
        ..Default::default()
      })
    });
    let handler = Handler::new(&mock);

    let block1_start = || {
      let mut req = request(code::POST, "pigeon-1", "logs");
      req.set_option_uint(
        option::BLOCK1,
        Block {
          num: 0,
          more: true,
          szx: 4,
        }
        .encode(),
      );
      req.payload = vec![0xAA; 16];
      req
    };
    let peer = |i: usize| DeviceSession {
      pigeon_id: "pigeon-1".into(),
      token: "tok-1".into(),
      peer: format!("10.0.0.1:{i}"),
    };

    // Fill the table: one in-progress reassembly per distinct peer.
    for i in 0..MAX_BLOCK1_ENTRIES {
      let out = handler
        .handle(&block1_start(), &peer(i), Transport::Udp)
        .await;
      assert_eq!(out.code, code::CONTINUE, "entry {i} should be admitted");
    }

    // A never-seen peer starting a new reassembly is refused -- the table
    // is at capacity, not because of anything wrong with this request.
    let out = handler
      .handle(&block1_start(), &peer(MAX_BLOCK1_ENTRIES), Transport::Udp)
      .await;
    assert_eq!(out.code, code::SERVICE_UNAVAILABLE);

    // A peer already holding a slot can still finish its own upload --
    // the cap only blocks new entries, not progress on admitted ones.
    let mut req = request(code::POST, "pigeon-1", "logs");
    req.set_option_uint(
      option::BLOCK1,
      Block {
        num: 1,
        more: false,
        szx: 4,
      }
      .encode(),
    );
    req.payload = vec![0xBB; 16];
    let out = handler.handle(&req, &peer(0), Transport::Udp).await;
    assert_eq!(out.code, code::CHANGED);
  }

  #[tokio::test]
  async fn block2_past_end_of_short_body_is_rejected_not_a_panic() {
    let mock = MockUpstream::new(|_| Ok(json_ok("{}")));
    let handler = Handler::new(&mock);

    // Block num 5 against a 2-byte body: must be a clean 4.00.
    let mut req = request(code::GET, "pigeon-1", "shadow");
    req.set_option_uint(
      option::BLOCK2,
      Block {
        num: 5,
        more: false,
        szx: 6,
      }
      .encode(),
    );
    let out = handler.handle(&req, &session(), Transport::Udp).await;
    assert_eq!(out.code, code::BAD_REQUEST);

    // Block 0 against an empty body: legitimate empty final block.
    let empty = MockUpstream::new(|_| {
      Ok(UpstreamResponse {
        status: 200,
        ..Default::default()
      })
    });
    let handler = Handler::new(&empty);
    let mut req = request(code::GET, "pigeon-1", "shadow");
    req.set_option_uint(
      option::BLOCK2,
      Block {
        num: 0,
        more: false,
        szx: 6,
      }
      .encode(),
    );
    let out = handler.handle(&req, &session(), Transport::Udp).await;
    assert_eq!(out.code, code::CONTENT);
    assert!(out.payload.is_empty());
  }

  #[tokio::test]
  async fn block1_out_of_sequence_is_4_08() {
    let mock = MockUpstream::new(|_| Ok(json_ok("{}")));
    let handler = Handler::new(&mock);
    let sess = session();

    let mut req = request(code::POST, "pigeon-1", "logs");
    req.set_option_uint(
      option::BLOCK1,
      Block {
        num: 2,
        more: true,
        szx: 4,
      }
      .encode(),
    );
    req.payload = vec![1; 16];
    let out = handler.handle(&req, &sess, Transport::Udp).await;
    assert_eq!(out.code, code::REQUEST_ENTITY_INCOMPLETE);
    assert!(mock.calls.lock().unwrap().is_empty());
  }
}
