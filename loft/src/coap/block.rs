//! RFC 7959 block-wise transfer: the Block1/Block2 option value
//! (`num << 4 | m << 3 | szx`) and the Block2 -> HTTP Range mapping used
//! by the firmware download path.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
  pub num: u32,
  pub more: bool,
  /// Block size exponent: size = 2^(szx + 4), szx in 0..=6. 7 (BERT,
  /// RFC 8323) is normalized to 6 by `decode` -- we never emit BERT.
  pub szx: u8,
}

/// Largest block size we serve (szx 6). Matches the HTTP Range chunk sizes
/// constrained FOTA clients already use against dovecote.
pub const MAX_SZX: u8 = 6;

impl Block {
  pub fn size(&self) -> usize {
    1 << (self.szx + 4)
  }

  pub fn offset(&self) -> u64 {
    u64::from(self.num) * self.size() as u64
  }

  /// Decodes a Block option uint. `None` for a malformed value (RFC 7959:
  /// szx 7 is reserved in CoAP-over-UDP; over TCP it means BERT, which we
  /// down-negotiate to szx 6 -- the client then continues at 1024-byte
  /// blocks, which RFC 7959 permits the server to impose).
  pub fn decode(value: u32) -> Option<Block> {
    let szx = (value & 0x7) as u8;
    Some(Block {
      num: value >> 4,
      more: value & 0x8 != 0,
      szx: szx.min(MAX_SZX),
    })
  }

  pub fn encode(&self) -> u32 {
    (self.num << 4) | (u32::from(self.more) << 3) | u32::from(self.szx.min(MAX_SZX))
  }

  /// The inclusive HTTP byte range covering this block.
  pub fn byte_range(&self) -> (u64, u64) {
    let start = self.offset();
    (start, start + self.size() as u64 - 1)
  }
}

/// Given the total resource size and the block a client asked for, whether
/// any bytes remain AFTER this block (the Block2 `m` bit of the response).
pub fn more_after(block: &Block, total: u64) -> bool {
  block.offset() + (block.size() as u64) < total
}

/// Parses an HTTP `Content-Range: bytes <start>-<end>/<total>` header.
pub fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
  let rest = value.trim().strip_prefix("bytes ")?;
  let (range, total) = rest.split_once('/')?;
  let (start, end) = range.split_once('-')?;
  Some((
    start.trim().parse().ok()?,
    end.trim().parse().ok()?,
    total.trim().parse().ok()?,
  ))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn encode_decode_roundtrip() {
    for (num, more, szx) in [
      (0, false, 6),
      (1, true, 4),
      (511, true, 0),
      (100_000, false, 2),
    ] {
      let b = Block { num, more, szx };
      assert_eq!(Block::decode(b.encode()), Some(b));
    }
  }

  #[test]
  fn known_wire_values() {
    // RFC 7959 examples: num=0 m=1 szx=2 (64 bytes) -> 0x0A.
    let b = Block::decode(0x0A).unwrap();
    assert_eq!((b.num, b.more, b.size()), (0, true, 64));
    // num=3, m=0, szx=6 (1024) -> 0x36.
    let b = Block::decode(0x36).unwrap();
    assert_eq!((b.num, b.more, b.size()), (3, false, 1024));
    assert_eq!(b.byte_range(), (3072, 4095));
  }

  #[test]
  fn bert_szx_normalized() {
    let b = Block::decode(0x0F).unwrap();
    assert_eq!(b.szx, MAX_SZX);
    assert_eq!(b.size(), 1024);
  }

  #[test]
  fn more_bit_from_total() {
    let b = |num| Block {
      num,
      more: false,
      szx: MAX_SZX,
    };
    // 2500-byte image at 1024-byte blocks: blocks 0,1 have more, 2 is last.
    assert!(more_after(&b(0), 2500));
    assert!(more_after(&b(1), 2500));
    assert!(!more_after(&b(2), 2500));
    // Exact multiple: 2048 bytes -> block 1 is last.
    assert!(more_after(&b(0), 2048));
    assert!(!more_after(&b(1), 2048));
  }

  #[test]
  fn content_range_parsing() {
    assert_eq!(
      parse_content_range("bytes 1024-2047/523776"),
      Some((1024, 2047, 523776))
    );
    assert_eq!(parse_content_range("bytes 0-0/1"), Some((0, 0, 1)));
    assert_eq!(parse_content_range("bytes */523776"), None);
    assert_eq!(parse_content_range("garbage"), None);
  }
}
