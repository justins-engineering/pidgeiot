use base64::{
  Engine as _,
  engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use time::OffsetDateTime;

const TOKEN_VERSION: u8 = 1;
// version(1) + expires_at(4, u32 LE unix seconds) + signature(64)
const PAYLOAD_LEN: usize = 5;
const TOKEN_LEN: usize = PAYLOAD_LEN + 64;

/// Mints a fresh per-pigeon Ed25519 keypair and a compact binary bearer
/// token signed with it. The private key is never persisted or returned —
/// only the token (handed to the human once) and the public key (stored by
/// the caller for later verification) survive this call. Because each
/// mint generates a brand new keypair, calling this again to "refresh" a
/// token implicitly revokes every token signed under the previous keypair:
/// their signatures can never again be verified once the old public key is
/// overwritten in storage.
pub fn mint_device_credential() -> worker::Result<(String, String, OffsetDateTime)> {
  let mut seed = [0u8; 32];
  getrandom::getrandom(&mut seed)
    .map_err(|e| worker::Error::RustError(format!("RNG error: {e}")))?;

  let signing_key = SigningKey::from_bytes(&seed);
  let verifying_key = signing_key.verifying_key();

  let now = OffsetDateTime::now_utc();
  let expires_at = now.replace_year(now.year() + 1).map_err(|e| {
    worker::Error::RustError(format!("OffsetDateTime error setting expires_at: {e}"))
  })?;
  let expires_at_secs = u32::try_from(expires_at.unix_timestamp())
    .map_err(|e| worker::Error::RustError(format!("Expiry out of range: {e}")))?;

  let mut payload = [0u8; PAYLOAD_LEN];
  payload[0] = TOKEN_VERSION;
  payload[1..5].copy_from_slice(&expires_at_secs.to_le_bytes());

  let signature = signing_key.sign(&payload);

  let mut token_bytes = Vec::with_capacity(TOKEN_LEN);
  token_bytes.extend_from_slice(&payload);
  token_bytes.extend_from_slice(&signature.to_bytes());

  let token = URL_SAFE_NO_PAD.encode(token_bytes);
  let public_key = STANDARD.encode(verifying_key.to_bytes());

  Ok((public_key, token, expires_at))
}

/// Verifies a compact binary bearer token against a pigeon's stored public
/// key (base64, produced by `mint_device_credential`). Checks the
/// signature and the token's own embedded expiry; callers do not need to
/// separately track expiry for authorization purposes.
pub fn verify_device_token(token: &str, public_key_b64: &str) -> bool {
  let Ok(public_key_bytes) = STANDARD.decode(public_key_b64) else {
    return false;
  };
  let Ok(public_key_arr) = <[u8; 32]>::try_from(public_key_bytes.as_slice()) else {
    return false;
  };
  let Ok(verifying_key) = VerifyingKey::from_bytes(&public_key_arr) else {
    return false;
  };

  let Ok(token_bytes) = URL_SAFE_NO_PAD.decode(token) else {
    return false;
  };
  if token_bytes.len() != TOKEN_LEN {
    return false;
  }

  let (payload, signature_bytes) = token_bytes.split_at(PAYLOAD_LEN);
  if payload[0] != TOKEN_VERSION {
    return false;
  }

  let expires_at_secs = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
  let Ok(now_secs) = u32::try_from(OffsetDateTime::now_utc().unix_timestamp()) else {
    return false;
  };
  if now_secs >= expires_at_secs {
    return false;
  }

  let Ok(signature_arr) = <[u8; 64]>::try_from(signature_bytes) else {
    return false;
  };
  let signature = Signature::from_bytes(&signature_arr);

  verifying_key.verify(payload, &signature).is_ok()
}
