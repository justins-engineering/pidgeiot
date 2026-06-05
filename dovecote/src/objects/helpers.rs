use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::pkcs8::DecodePrivateKey;
use jwt_simple::prelude::*;
use worker::Env;

fn load_signing_key(env: &Env) -> worker::Result<Ed25519KeyPair> {
  let der_b64 = env.secret("DEVICE_PRIVATE_KEY")?.to_string();
  let der = STANDARD
    .decode(der_b64)
    .map_err(|e| worker::Error::RustError(format!("Base64 decode error: {e}")))?;

  // Parse PKCS#8 DER to extract the 32-byte seed
  let sk = ed25519_dalek::SigningKey::from_pkcs8_der(&der)
    .map_err(|e| worker::Error::RustError(format!("PKCS#8 parse error: {e}")))?;

  // jwt-simple expects 64 bytes: seed || public_key
  let mut expanded = [0u8; 64];
  expanded[..32].copy_from_slice(sk.as_bytes());
  expanded[32..].copy_from_slice(sk.verifying_key().as_bytes());

  Ed25519KeyPair::from_bytes(&expanded)
    .map_err(|e| worker::Error::RustError(format!("Key load error: {e}")))
}

pub fn sign_device_token(pigeon_id: &str, env: &Env) -> worker::Result<String> {
  let keypair = load_signing_key(env)?;

  let claims = Claims::create(Duration::from_hours(24 * 365)).with_subject(pigeon_id);

  keypair
    .sign(claims)
    .map_err(|e| worker::Error::RustError(format!("JWT sign error: {e}")))
}
