//! Upstream HTTP client for dovecote's device routes. After a PSK
//! handshake, the terminator acts as an ordinary device-side HTTP client:
//! `Authorization: Bearer <token>` on `/device/pigeons/:id/*`, where
//! `token` is the pigeon's device bearer token -- a distinct credential
//! from the PSK secret that keyed the handshake (minted together, rotated
//! together; see `capsules::CoapConfig`). The PSK only proves the peer is
//! this pigeon; the bearer token is what actually authorizes each
//! upstream call, and the owning Durable Object verifies it per-request
//! exactly as it does for direct HTTPS devices. Nothing here weakens or
//! bypasses device auth.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
  Get,
  Post,
}

/// The subset of an upstream HTTP response the CoAP layer needs.
#[derive(Debug, Default, Clone)]
pub struct UpstreamResponse {
  pub status: u16,
  pub body: Vec<u8>,
  /// `Content-Range` (firmware 206s).
  pub content_range: Option<String>,
  /// `X-Firmware-Size` (total image bytes).
  pub firmware_size: Option<u64>,
  /// `X-Firmware-Sha256` (hex).
  pub firmware_sha256: Option<String>,
}

/// Async upstream abstraction so the handler is unit-testable without a
/// network. The single implementation is `Dovecote` below.
pub trait Upstream: Send + Sync {
  fn device_request(
    &self,
    method: Method,
    pigeon_id: &str,
    leaf: &str,
    bearer: &str,
    range: Option<(u64, u64)>,
    body: Option<(Vec<u8>, &'static str)>,
  ) -> impl std::future::Future<Output = Result<UpstreamResponse, String>> + Send;
}

pub struct Dovecote {
  client: reqwest::Client,
  base_url: String,
}

impl Dovecote {
  pub fn new(base_url: &str) -> Result<Dovecote, String> {
    let client = reqwest::Client::builder()
      // Distinctive UA -- docs/api.md's device-auth troubleshooting note:
      // default library UAs can trip edge bot heuristics into HTML 403s.
      .user_agent(concat!("loft/", env!("CARGO_PKG_VERSION")))
      .timeout(Duration::from_secs(30))
      .connect_timeout(Duration::from_secs(10))
      .build()
      .map_err(|e| format!("upstream client build: {e}"))?;
    Ok(Dovecote {
      client,
      base_url: base_url.trim_end_matches('/').to_string(),
    })
  }
}

impl Upstream for Dovecote {
  async fn device_request(
    &self,
    method: Method,
    pigeon_id: &str,
    leaf: &str,
    bearer: &str,
    range: Option<(u64, u64)>,
    body: Option<(Vec<u8>, &'static str)>,
  ) -> Result<UpstreamResponse, String> {
    let url = format!("{}/device/pigeons/{}/{}", self.base_url, pigeon_id, leaf);

    let mut req = match method {
      Method::Get => self.client.get(&url),
      Method::Post => self.client.post(&url),
    };
    req = req.header("Authorization", format!("Bearer {bearer}"));
    if let Some((start, end)) = range {
      req = req.header("Range", format!("bytes={start}-{end}"));
    }
    if let Some((bytes, content_type)) = body {
      req = req.header("Content-Type", content_type).body(bytes);
    }

    let resp = req.send().await.map_err(|e| format!("upstream: {e}"))?;

    let status = resp.status().as_u16();
    let header = |name: &str| {
      resp
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
    };
    let content_range = header("Content-Range");
    let firmware_size = header("X-Firmware-Size").and_then(|v| v.parse().ok());
    let firmware_sha256 = header("X-Firmware-Sha256");

    let body = resp
      .bytes()
      .await
      .map_err(|e| format!("upstream body: {e}"))?
      .to_vec();

    Ok(UpstreamResponse {
      status,
      body,
      content_range,
      firmware_size,
      firmware_sha256,
    })
  }
}
