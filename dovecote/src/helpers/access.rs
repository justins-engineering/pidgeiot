// Staging's Cloudflare Access gate is designed but not yet wired into
// `main()` (staging rollout is paused pending a worker-naming decision —
// see git history). Silence the resulting dead-code warnings rather than
// leaving them as noise; drop this once `verify_cf_access` is called.
#![allow(dead_code)]

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use js_sys::{Array, Object, Reflect};
use serde::Deserialize;
use std::cell::RefCell;
use time::OffsetDateTime;
use wasm_bindgen::JsCast;
use web_sys::{CryptoKey, SubtleCrypto, WorkerGlobalScope};
use worker::{Env, Fetch, Request, Url};

// Cloudflare Access rotates its signing keys infrequently. Caching the
// JWKS per-isolate avoids a network round-trip on every staging request,
// while `find_jwk` below still forces a fresh fetch whenever a token's
// `kid` isn't in the cached set, so a rotation never locks staging out
// for longer than one request.
const JWKS_CACHE_TTL_SECS: i64 = 300;

// Small allowance for clock skew between Access's token issuance and this
// isolate's clock — both run on Cloudflare's network, so skew should be
// negligible, but a strict `exp` check with zero leeway risks spurious
// rejections right at the boundary.
const EXP_LEEWAY_SECS: i64 = 60;

#[derive(Deserialize)]
struct JwtHeader {
  kid: String,
  alg: String,
}

// Cloudflare Access issues `aud` as a single-element array, but the JWT
// spec allows a bare string too — accept both rather than being brittle.
#[derive(Deserialize)]
#[serde(untagged)]
enum AudClaim {
  Single(String),
  Many(Vec<String>),
}

impl AudClaim {
  fn contains(&self, expected: &str) -> bool {
    match self {
      AudClaim::Single(s) => s == expected,
      AudClaim::Many(v) => v.iter().any(|s| s == expected),
    }
  }
}

#[derive(Deserialize)]
struct JwtClaims {
  aud: AudClaim,
  exp: i64,
}

#[derive(Deserialize, Clone)]
struct Jwks {
  keys: Vec<serde_json::Value>,
}

thread_local! {
  static JWKS_CACHE: RefCell<Option<(i64, Jwks)>> = const { RefCell::new(None) };
}

/// Enforces Cloudflare Access on `req`, but only when staging config
/// (`CF_ACCESS_AUD`/`CF_ACCESS_CERTS_URL`) is present in this Worker's
/// vars — dev and production don't set these, so this is a no-op there.
/// On success the request is Access-authenticated; on failure the caller
/// should 403 before routing any further.
pub async fn verify_cf_access(req: &Request, env: &Env) -> Result<(), String> {
  let Ok(aud) = env.var("CF_ACCESS_AUD").map(|v| v.to_string()) else {
    return Ok(());
  };
  let Ok(certs_url) = env.var("CF_ACCESS_CERTS_URL").map(|v| v.to_string()) else {
    return Ok(());
  };

  let Ok(Some(assertion)) = req.headers().get("Cf-Access-Jwt-Assertion") else {
    return Err("Missing Cf-Access-Jwt-Assertion header".into());
  };

  validate_jwt(&assertion, &aud, &certs_url).await
}

async fn validate_jwt(token: &str, expected_aud: &str, certs_url: &str) -> Result<(), String> {
  let mut parts = token.split('.');
  let (Some(header_b64), Some(payload_b64), Some(sig_b64), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return Err("Malformed JWT: expected exactly 3 dot-separated segments".into());
  };

  let header_bytes = URL_SAFE_NO_PAD
    .decode(header_b64)
    .map_err(|e| format!("Bad JWT header encoding: {e}"))?;
  let header: JwtHeader =
    serde_json::from_slice(&header_bytes).map_err(|e| format!("Bad JWT header JSON: {e}"))?;

  if header.alg != "RS256" {
    return Err(format!(
      "Unsupported JWT alg '{}': only RS256 is verified",
      header.alg
    ));
  }

  let payload_bytes = URL_SAFE_NO_PAD
    .decode(payload_b64)
    .map_err(|e| format!("Bad JWT payload encoding: {e}"))?;
  let claims: JwtClaims =
    serde_json::from_slice(&payload_bytes).map_err(|e| format!("Bad JWT payload JSON: {e}"))?;

  if !claims.aud.contains(expected_aud) {
    return Err("aud claim does not match this application".into());
  }

  if claims.exp + EXP_LEEWAY_SECS <= OffsetDateTime::now_utc().unix_timestamp() {
    return Err("Token expired".into());
  }

  let signature = URL_SAFE_NO_PAD
    .decode(sig_b64)
    .map_err(|e| format!("Bad JWT signature encoding: {e}"))?;

  let jwk = find_jwk(&header.kid, certs_url).await?;
  let signing_input = format!("{header_b64}.{payload_b64}");

  verify_rs256(&jwk, &signature, signing_input.as_bytes()).await
}

/// Looks up `kid` in the cached JWKS (if fresh), otherwise re-fetches —
/// this both refreshes on TTL expiry and transparently handles key
/// rotation for a `kid` the cache doesn't know about yet.
async fn find_jwk(kid: &str, certs_url: &str) -> Result<serde_json::Value, String> {
  let now = OffsetDateTime::now_utc().unix_timestamp();

  let cached = JWKS_CACHE.with(|cache| {
    cache.borrow().as_ref().and_then(|(fetched_at, jwks)| {
      if now - fetched_at < JWKS_CACHE_TTL_SECS {
        find_kid(jwks, kid)
      } else {
        None
      }
    })
  });

  if let Some(jwk) = cached {
    return Ok(jwk);
  }

  let jwks = fetch_jwks(certs_url).await?;
  let found = find_kid(&jwks, kid);
  JWKS_CACHE.with(|cache| *cache.borrow_mut() = Some((now, jwks)));

  found.ok_or_else(|| format!("No JWKS key matching kid '{kid}'"))
}

fn find_kid(jwks: &Jwks, kid: &str) -> Option<serde_json::Value> {
  jwks
    .keys
    .iter()
    .find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid))
    .cloned()
}

async fn fetch_jwks(certs_url: &str) -> Result<Jwks, String> {
  let url = Url::parse(certs_url).map_err(|e| format!("Bad CF_ACCESS_CERTS_URL: {e}"))?;
  let mut resp = Fetch::Url(url)
    .send()
    .await
    .map_err(|e| format!("JWKS fetch failed: {e}"))?;

  if resp.status_code() >= 400 {
    return Err(format!(
      "JWKS endpoint returned HTTP {}",
      resp.status_code()
    ));
  }

  resp
    .json::<Jwks>()
    .await
    .map_err(|e| format!("JWKS JSON parse error: {e}"))
}

async fn verify_rs256(
  jwk: &serde_json::Value,
  signature: &[u8],
  data: &[u8],
) -> Result<(), String> {
  let subtle = subtle_crypto()?;

  let key_data: Object = serde_wasm_bindgen::to_value(jwk)
    .map_err(|e| format!("JWK serialization error: {e:?}"))?
    .unchecked_into();

  let algorithm = Object::new();
  Reflect::set(&algorithm, &"name".into(), &"RSASSA-PKCS1-v1_5".into())
    .map_err(|e| format!("building import algorithm failed: {e:?}"))?;
  Reflect::set(&algorithm, &"hash".into(), &"SHA-256".into())
    .map_err(|e| format!("building import algorithm failed: {e:?}"))?;

  let usages = Array::new();
  usages.push(&"verify".into());

  let key_promise = subtle
    .import_key_with_object("jwk", &key_data, &algorithm, false, &usages)
    .map_err(|e| format!("importKey call failed: {e:?}"))?;
  let key_value = wasm_bindgen_futures::JsFuture::from(key_promise)
    .await
    .map_err(|e| format!("importKey rejected: {e:?}"))?;
  let key: CryptoKey = key_value.unchecked_into();

  let verify_promise = subtle
    .verify_with_str_and_u8_array_and_u8_array("RSASSA-PKCS1-v1_5", &key, signature, data)
    .map_err(|e| format!("verify call failed: {e:?}"))?;
  let verified = wasm_bindgen_futures::JsFuture::from(verify_promise)
    .await
    .map_err(|e| format!("verify rejected: {e:?}"))?;

  if verified.as_bool() == Some(true) {
    Ok(())
  } else {
    Err("Signature verification failed".into())
  }
}

fn subtle_crypto() -> Result<SubtleCrypto, String> {
  let global: WorkerGlobalScope = js_sys::global().unchecked_into();
  let crypto = global
    .crypto()
    .map_err(|e| format!("crypto unavailable in this isolate: {e:?}"))?;
  Ok(crypto.subtle())
}
