use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::SigningKey;
use jwt_simple::prelude::*;
use worker::Env;

pub fn sign_device_token(pigeon_id: &str, env: &Env) -> worker::Result<String> {
  let seed_b64 = env.secret("DEVICE_SEED")?.to_string();
  let seed_bytes = STANDARD
    .decode(seed_b64)
    .map_err(|e| worker::Error::RustError(format!("Base64 decode error: {e}")))?;

  let seed: [u8; 32] = seed_bytes
    .try_into()
    .map_err(|_| worker::Error::RustError("Invalid seed length".into()))?;

  let sk = SigningKey::from_bytes(&seed);

  // jwt-simple expects 64 bytes: seed || pubkey
  let mut expanded = [0u8; 64];
  expanded[..32].copy_from_slice(&seed);
  expanded[32..].copy_from_slice(sk.verifying_key().as_bytes());

  let keypair = Ed25519KeyPair::from_bytes(&expanded)
    .map_err(|e| worker::Error::RustError(format!("Key load error: {e}")))?;

  let claims = Claims::create(Duration::from_hours(24 * 365)).with_subject(pigeon_id);

  keypair
    .sign(claims)
    .map_err(|e| worker::Error::RustError(format!("JWT sign error: {e}")))
}
