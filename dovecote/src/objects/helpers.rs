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

/// Mints a fresh TLS-PSK secret for the connectors that take one (`Coap`,
/// `Mqtt`): 16 random bytes as 32 lowercase hex chars. Deliberately short
/// and hex-only -- RFC 4279 only obliges peers to support PSKs up to 64
/// bytes, mbedTLS's default MBEDTLS_PSK_MAX_LEN is 32, and libcoap's
/// client caps at 64, so the 92-char bearer token cannot serve as the PSK
/// on the constrained stacks these transports exist for. 128 bits of
/// entropy matches the AES-128 PSK ciphersuites it keys. One mint per
/// create/refresh, so a pigeon's PSK and token always rotate together.
pub fn mint_device_psk() -> worker::Result<String> {
  let mut bytes = [0u8; 16];
  getrandom::getrandom(&mut bytes)
    .map_err(|e| worker::Error::RustError(format!("RNG error: {e}")))?;
  let mut hex = String::with_capacity(32);
  for b in bytes {
    hex.push_str(&format!("{b:02x}"));
  }
  Ok(hex)
}

/// The device token an `Authorization` header carries: `Bearer <token>`, or, only where
/// `allow_basic`, `Basic <base64 of user:token>`. The user name is ignored because the path already
/// names the pigeon, and the password is everything after the first colon, which a base64url token
/// never contains. `None` for any other form; whether the token is good is
/// [`verify_device_token`]'s question.
pub fn device_token(header: &str, allow_basic: bool) -> Option<String> {
  if let Some(token) = header.strip_prefix("Bearer ") {
    return Some(token.to_string());
  }
  if !allow_basic {
    return None;
  }
  let encoded = header.strip_prefix("Basic ")?;
  let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
  let (_, password) = decoded.split_once(':')?;
  (!password.is_empty()).then(|| password.to_string())
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

#[cfg(test)]
mod tests {
  use super::*;

  fn basic(pair: &str) -> String {
    let encoded = STANDARD.encode(pair);
    let mut header = String::with_capacity(6 + encoded.len());
    header.push_str("Basic ");
    header.push_str(&encoded);
    header
  }

  #[test]
  fn bearer_is_read_whether_or_not_basic_is_allowed() {
    assert_eq!(
      device_token("Bearer abc-_1", false).as_deref(),
      Some("abc-_1")
    );
    assert_eq!(
      device_token("Bearer abc-_1", true).as_deref(),
      Some("abc-_1")
    );
  }

  #[test]
  fn bearer_without_its_space_is_refused() {
    assert_eq!(device_token("Bearer", true), None);
    assert_eq!(device_token("Bearerabc", true), None);
    assert_eq!(device_token("bearer abc", true), None);
  }

  #[test]
  fn basic_is_refused_where_it_is_not_allowed() {
    assert_eq!(device_token(&basic("sign:abc"), false), None);
  }

  #[test]
  fn basic_takes_the_password_under_any_user_name() {
    assert_eq!(
      device_token(&basic("sign:abc-_1"), true).as_deref(),
      Some("abc-_1")
    );
    assert_eq!(
      device_token(&basic("anyone else:abc"), true).as_deref(),
      Some("abc")
    );
    assert_eq!(device_token(&basic(":abc"), true).as_deref(), Some("abc"));
  }

  #[test]
  fn basic_splits_at_the_first_colon() {
    assert_eq!(
      device_token(&basic("sign:a:b"), true).as_deref(),
      Some("a:b")
    );
  }

  #[test]
  fn malformed_basic_is_refused() {
    assert_eq!(device_token("Basic not base64!", true), None);
    assert_eq!(device_token(&basic("no colon here"), true), None);
    assert_eq!(device_token(&basic("sign:"), true), None);
    assert_eq!(device_token("Basic ", true), None);
    let mut not_utf8 = String::from("Basic ");
    not_utf8.push_str(&STANDARD.encode([b's', b':', 0xff, 0xfe]));
    assert_eq!(device_token(&not_utf8, true), None);
  }

  #[test]
  fn a_minted_token_verifies_when_sent_as_a_basic_password() {
    let (public_key, token, _) = mint_device_credential().expect("minted a credential");
    let mut pair = String::from("sign:");
    pair.push_str(&token);
    let read = device_token(&basic(&pair), true).expect("read the Basic password");
    assert!(verify_device_token(&read, &public_key));
  }
}
