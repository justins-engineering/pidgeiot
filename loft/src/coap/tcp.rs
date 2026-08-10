//! RFC 8323 CoAP-over-TCP message framing: a length-prefixed frame with no
//! version/type/message-id -- `[Len|TKL] [ExtLen 0-4] [Code] [Token]
//! [Options] [0xFF Payload]`, where Len covers options+payload only
//! (exactly the framing `~/pigeon`'s `pigeon_coap.c` implements).

use super::message::{self, DecodeError, MAX_TOKEN_LEN, Message};

/// Hard cap on a single decoded frame's options+payload region. Generous
/// enough for any real device surface response (firmware moves via Block2,
/// logs are capped at 16 KiB per chunk upstream) while bounding what one
/// connection can make us buffer.
pub const MAX_FRAME_BODY: usize = 256 * 1024;

pub fn encode_frame(msg: &Message) -> Vec<u8> {
  let mut body = Vec::new();
  message::encode_options_and_payload(msg, &mut body);

  let tkl = msg.token.len().min(MAX_TOKEN_LEN) as u8;
  let mut out = Vec::with_capacity(body.len() + msg.token.len() + 6);

  let (len_nibble, ext): (u8, Vec<u8>) = if body.len() < 13 {
    (body.len() as u8, Vec::new())
  } else if body.len() < 269 {
    (13, vec![(body.len() - 13) as u8])
  } else if body.len() < 65805 {
    (14, ((body.len() - 269) as u16).to_be_bytes().to_vec())
  } else {
    (15, ((body.len() - 65805) as u32).to_be_bytes().to_vec())
  };

  out.push((len_nibble << 4) | tkl);
  out.extend_from_slice(&ext);
  out.push(msg.code);
  out.extend_from_slice(&msg.token[..tkl as usize]);
  out.extend_from_slice(&body);
  out
}

/// Incremental frame decoder over a byte buffer (append bytes, then call
/// `next_frame` until it returns `Ok(None)`).
#[derive(Default)]
pub struct FrameDecoder {
  buf: Vec<u8>,
}

impl FrameDecoder {
  pub fn extend(&mut self, bytes: &[u8]) {
    self.buf.extend_from_slice(bytes);
  }

  #[cfg(test)]
  pub fn buffered(&self) -> usize {
    self.buf.len()
  }

  /// Decodes and removes one complete frame from the buffer, if present.
  pub fn next_frame(&mut self) -> Result<Option<Message>, DecodeError> {
    let Some((msg, consumed)) = try_decode_frame(&self.buf)? else {
      return Ok(None);
    };
    self.buf.drain(..consumed);
    Ok(Some(msg))
  }
}

/// Returns `Ok(None)` when more bytes are needed, `Ok(Some((msg, consumed)))`
/// for one complete frame at the head of `buf`.
fn try_decode_frame(buf: &[u8]) -> Result<Option<(Message, usize)>, DecodeError> {
  let [first, rest @ ..] = buf else {
    return Ok(None);
  };

  let len_nibble = first >> 4;
  let tkl = (first & 0x0F) as usize;
  if tkl > MAX_TOKEN_LEN {
    return Err(DecodeError::BadTokenLength);
  }

  let (body_len, ext_bytes): (usize, usize) = match len_nibble {
    0..=12 => (usize::from(len_nibble), 0),
    13 => {
      let [ext, ..] = rest else { return Ok(None) };
      (13 + usize::from(*ext), 1)
    }
    14 => {
      let [a, b, ..] = rest else { return Ok(None) };
      (269 + usize::from(u16::from_be_bytes([*a, *b])), 2)
    }
    _ => {
      let [a, b, c, d, ..] = rest else {
        return Ok(None);
      };
      (65805 + u32::from_be_bytes([*a, *b, *c, *d]) as usize, 4)
    }
  };

  if body_len > MAX_FRAME_BODY {
    return Err(DecodeError::Truncated);
  }

  let total = 1 + ext_bytes + 1 + tkl + body_len;
  if buf.len() < total {
    return Ok(None);
  }

  let code = buf[1 + ext_bytes];
  let token_start = 1 + ext_bytes + 1;
  let token = buf[token_start..token_start + tkl].to_vec();

  let mut msg = Message {
    code,
    token,
    ..Default::default()
  };
  message::decode_options_and_payload(&buf[token_start + tkl..total], &mut msg)?;

  Ok(Some((msg, total)))
}

#[cfg(test)]
mod tests {
  use super::super::message::{code, option};
  use super::*;

  fn sample(payload_len: usize) -> Message {
    let mut msg = Message {
      code: code::POST,
      token: vec![1, 2, 3, 4],
      ..Default::default()
    };
    msg.push_option(option::URI_PATH, b"telemetry".to_vec());
    msg.payload = vec![0x5A; payload_len];
    msg
  }

  #[test]
  fn roundtrip_all_length_encodings() {
    // < 13, 13..268 (1-byte ext), 269..65804 (2-byte ext), >= 65805 (4-byte).
    for len in [0usize, 5, 100, 5_000, 70_000] {
      let msg = sample(len);
      let frame = encode_frame(&msg);
      let mut dec = FrameDecoder::default();
      dec.extend(&frame);
      let out = dec.next_frame().unwrap().expect("complete frame");
      assert_eq!(out, msg, "len {len}");
      assert_eq!(dec.buffered(), 0);
    }
  }

  #[test]
  fn incremental_delivery_and_pipelining() {
    let a = sample(300);
    let b = sample(2);
    let mut wire = encode_frame(&a);
    wire.extend_from_slice(&encode_frame(&b));

    let mut dec = FrameDecoder::default();
    // Feed one byte at a time; must yield exactly two frames at the end.
    let mut got = Vec::new();
    for byte in wire {
      dec.extend(&[byte]);
      while let Some(m) = dec.next_frame().unwrap() {
        got.push(m);
      }
    }
    assert_eq!(got, vec![a, b]);
  }

  #[test]
  fn matches_pigeon_client_header_layout() {
    // A frame the ~/pigeon client would send: body_len < 13 -> single
    // header byte (len<<4 | tkl), then code, then token.
    let mut msg = Message {
      code: code::GET,
      token: vec![0xAA, 0xBB],
      ..Default::default()
    };
    msg.push_option(option::URI_PATH, b"s".to_vec());
    let frame = encode_frame(&msg);
    // Body = 1 option header byte + 1 value byte = 3? No: option header
    // (delta 11, len 1) is one byte + 1 value byte = 2 bytes total.
    assert_eq!(frame[0], (2 << 4) | 2);
    assert_eq!(frame[1], code::GET);
    assert_eq!(&frame[2..4], &[0xAA, 0xBB]);
  }

  #[test]
  fn oversized_frame_rejected() {
    // Declare a body far over MAX_FRAME_BODY via the 4-byte extended form.
    let mut dec = FrameDecoder::default();
    let huge: u32 = (MAX_FRAME_BODY as u32) + 1;
    let mut hdr = vec![0xF0];
    hdr.extend_from_slice(&(huge - 65805).to_be_bytes());
    dec.extend(&hdr);
    assert!(dec.next_frame().is_err());
  }

  #[test]
  fn signaling_codes_frame() {
    let csm = Message {
      code: code::CSM,
      ..Default::default()
    };
    let frame = encode_frame(&csm);
    let mut dec = FrameDecoder::default();
    dec.extend(&frame);
    let out = dec.next_frame().unwrap().unwrap();
    assert!(code::is_signaling(out.code));
    assert_eq!(out.code, code::CSM);
  }
}
