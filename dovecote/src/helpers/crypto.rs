/// Constant-time byte comparison for secret material -- a plain `==`
/// short-circuits on the first differing byte, which leaks prefix-match
/// timing to a caller probing the secret. Length still leaks (unavoidable
/// without padding) but reveals nothing useful on its own.
///
/// Used by the internal service-secret gate (`GET
/// /internal/coap-psk/:pigeon_id`, `lib.rs`) and by Stripe webhook
/// signature comparison (`helpers/stripe.rs`).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
  if a.len() != b.len() {
    return false;
  }
  a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
  use super::constant_time_eq;

  #[test]
  fn equal_slices_match() {
    assert!(constant_time_eq(b"", b""));
    assert!(constant_time_eq(b"secret", b"secret"));
  }

  #[test]
  fn differing_content_or_length_does_not_match() {
    assert!(!constant_time_eq(b"secret", b"secreT"));
    assert!(!constant_time_eq(b"secret", b"secret "));
    assert!(!constant_time_eq(b"", b"a"));
  }
}
