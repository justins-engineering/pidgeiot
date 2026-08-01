//! Minimal standard-alphabet base64 decoder for the dictionary database's
//! optional ELF string sections (`sections[*].data_b64`, written by Zephyr's
//! `database_gen.py` via Python's `base64.b64encode`).
//!
//! Deliberately not `web_sys` `atob` (which `helpers::decode_base64` wraps):
//! this module must run under plain `cargo test` on the host, where no
//! browser APIs exist -- and adding a whole base64 crate for one
//! fixed-alphabet decode would be overkill.

/// Decodes standard base64 (`+`/`/`, `=` padding, no whitespace tolerance
/// beyond ASCII whitespace, which Python never emits mid-stream anyway).
/// Returns `None` on any invalid character or truncated quantum.
pub fn decode(input: &str) -> Option<Vec<u8>> {
  fn val(c: u8) -> Option<u32> {
    match c {
      b'A'..=b'Z' => Some((c - b'A') as u32),
      b'a'..=b'z' => Some((c - b'a' + 26) as u32),
      b'0'..=b'9' => Some((c - b'0' + 52) as u32),
      b'+' => Some(62),
      b'/' => Some(63),
      _ => None,
    }
  }

  let bytes: Vec<u8> = input
    .bytes()
    .filter(|b| !b.is_ascii_whitespace())
    .collect();
  let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
  for quantum in bytes.chunks(4) {
    let pad = quantum.iter().rev().take_while(|&&b| b == b'=').count();
    if quantum.len() != 4 || pad > 2 {
      return None;
    }
    let mut acc = 0u32;
    for (i, &b) in quantum.iter().enumerate() {
      let v = if b == b'=' {
        // Padding is only valid at the end of the quantum.
        if i < 4 - pad {
          return None;
        }
        0
      } else {
        val(b)?
      };
      acc = (acc << 6) | v;
    }
    out.push((acc >> 16) as u8);
    if pad < 2 {
      out.push((acc >> 8) as u8);
    }
    if pad < 1 {
      out.push(acc as u8);
    }
  }
  Some(out)
}

#[cfg(test)]
mod tests {
  use super::decode;

  #[test]
  fn round_trips_known_vectors() {
    assert_eq!(decode(""), Some(vec![]));
    assert_eq!(decode("Zg=="), Some(b"f".to_vec()));
    assert_eq!(decode("Zm8="), Some(b"fo".to_vec()));
    assert_eq!(decode("Zm9v"), Some(b"foo".to_vec()));
    assert_eq!(decode("Zm9vYmFy"), Some(b"foobar".to_vec()));
    assert_eq!(decode("AAECA/8="), Some(vec![0, 1, 2, 3, 255]));
    assert_eq!(decode("+/8="), Some(vec![0xfb, 0xff]));
  }

  #[test]
  fn rejects_garbage() {
    assert_eq!(decode("Zm9!"), None);
    assert_eq!(decode("Zg"), None); // truncated quantum
    assert_eq!(decode("=Zm9"), None); // padding in the middle
  }
}
