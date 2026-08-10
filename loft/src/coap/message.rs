//! Transport-agnostic CoAP message core: codes, options, and the shared
//! options+payload encoding used by both the RFC 7252 UDP datagram format
//! (`super::udp`) and the RFC 8323 TCP framing (`super::tcp`).
//!
//! Hand-rolled rather than `coap-lite`: that crate only speaks the 4-byte
//! UDP header format (no RFC 8323 framing at all, confirmed against its
//! docs), and the sibling `~/pigeon` Zephyr client already speaks a
//! hand-rolled 8323 framing this must interoperate with byte-for-byte.
//! The shared middle (option delta encoding) is small and fully unit-tested
//! here.

/// CoAP code as one byte: 3-bit class, 5-bit detail (RFC 7252 section 3).
/// Deliberately the complete RFC table, not just the codes currently
/// emitted -- hence the module-level dead_code allowance.
#[allow(dead_code)]
pub mod code {
  pub const EMPTY: u8 = 0x00;
  pub const GET: u8 = 0x01;
  pub const POST: u8 = 0x02;
  pub const PUT: u8 = 0x03;
  pub const DELETE: u8 = 0x04;

  pub const CREATED: u8 = resp(2, 1);
  pub const DELETED: u8 = resp(2, 2);
  pub const VALID: u8 = resp(2, 3);
  pub const CHANGED: u8 = resp(2, 4);
  pub const CONTENT: u8 = resp(2, 5);
  /// 2.31 Continue (RFC 7959, Block1 intermediate ack).
  pub const CONTINUE: u8 = resp(2, 31);

  pub const BAD_REQUEST: u8 = resp(4, 0);
  pub const UNAUTHORIZED: u8 = resp(4, 1);
  pub const BAD_OPTION: u8 = resp(4, 2);
  pub const FORBIDDEN: u8 = resp(4, 3);
  pub const NOT_FOUND: u8 = resp(4, 4);
  pub const METHOD_NOT_ALLOWED: u8 = resp(4, 5);
  /// 4.08 Request Entity Incomplete (RFC 7959, broken Block1 sequence).
  pub const REQUEST_ENTITY_INCOMPLETE: u8 = resp(4, 8);
  pub const REQUEST_ENTITY_TOO_LARGE: u8 = resp(4, 13);

  pub const INTERNAL_SERVER_ERROR: u8 = resp(5, 0);
  pub const BAD_GATEWAY: u8 = resp(5, 2);
  pub const SERVICE_UNAVAILABLE: u8 = resp(5, 3);
  pub const GATEWAY_TIMEOUT: u8 = resp(5, 4);

  // RFC 8323 signaling codes (7.xx) -- TCP transport only.
  pub const CSM: u8 = resp(7, 1);
  pub const PING: u8 = resp(7, 2);
  pub const PONG: u8 = resp(7, 3);
  pub const RELEASE: u8 = resp(7, 4);
  pub const ABORT: u8 = resp(7, 5);

  pub const fn resp(class: u8, detail: u8) -> u8 {
    (class << 5) | detail
  }

  pub const fn class(code: u8) -> u8 {
    code >> 5
  }

  pub fn is_request(code: u8) -> bool {
    class(code) == 0 && code != EMPTY
  }

  pub fn is_signaling(code: u8) -> bool {
    class(code) == 7
  }

  /// "c.dd" display form for logs (e.g. 2.05, 4.04).
  pub fn dotted(code: u8) -> String {
    format!("{}.{:02}", class(code), code & 0x1F)
  }
}

/// Option numbers this terminator understands (RFC 7252 section 12.2 +
/// RFC 7959 + RFC 7252 ETag).
pub mod option {
  pub const ETAG: u16 = 4;
  pub const OBSERVE: u16 = 6;
  pub const URI_PORT: u16 = 7;
  pub const URI_PATH: u16 = 11;
  pub const CONTENT_FORMAT: u16 = 12;
  pub const URI_QUERY: u16 = 15;
  pub const BLOCK2: u16 = 23;
  pub const BLOCK1: u16 = 27;
  pub const SIZE2: u16 = 28;
  pub const URI_HOST: u16 = 3;
  pub const SIZE1: u16 = 60;

  /// RFC 7252 section 5.4.6: odd option numbers are critical -- a server
  /// receiving an unrecognized critical option in a request MUST reject
  /// with 4.02 Bad Option. Even numbers are elective and ignorable.
  pub fn is_critical(number: u16) -> bool {
    number & 1 == 1
  }
}

/// Content-Format values (RFC 7252 section 12.3).
pub mod content_format {
  pub const OCTET_STREAM: u16 = 42;
  pub const JSON: u16 = 50;
}

pub const MAX_TOKEN_LEN: usize = 8;

/// The transport-independent part of a CoAP message. UDP adds
/// version/type/message-id around this (see `super::udp::Datagram`); the
/// RFC 8323 TCP framing adds only a length prefix (see `super::tcp`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
  pub code: u8,
  pub token: Vec<u8>,
  /// (number, value) pairs. Kept in the order they appeared on decode;
  /// `encode_options_and_payload` sorts a copy by number as the delta
  /// encoding requires, so callers may push in any order.
  pub options: Vec<(u16, Vec<u8>)>,
  pub payload: Vec<u8>,
}

impl Message {
  pub fn response(code: u8, request: &Message) -> Message {
    Message {
      code,
      token: request.token.clone(),
      options: Vec::new(),
      payload: Vec::new(),
    }
  }

  pub fn option_values(&self, number: u16) -> impl Iterator<Item = &[u8]> {
    self
      .options
      .iter()
      .filter(move |(n, _)| *n == number)
      .map(|(_, v)| v.as_slice())
  }

  pub fn first_option(&self, number: u16) -> Option<&[u8]> {
    self.option_values(number).next()
  }

  /// Option value as a CoAP uint (RFC 7252 section 3.2: big-endian,
  /// minimal length, empty means 0). None if longer than 4 bytes.
  pub fn option_uint(&self, number: u16) -> Option<u32> {
    let v = self.first_option(number)?;
    if v.len() > 4 {
      return None;
    }
    Some(v.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b)))
  }

  pub fn set_option_uint(&mut self, number: u16, value: u32) {
    self.options.retain(|(n, _)| *n != number);
    self.options.push((number, encode_uint(value)));
  }

  pub fn push_option(&mut self, number: u16, value: Vec<u8>) {
    self.options.push((number, value));
  }

  /// Uri-Path segments in order.
  pub fn path_segments(&self) -> Vec<String> {
    self
      .option_values(option::URI_PATH)
      .map(|v| String::from_utf8_lossy(v).into_owned())
      .collect()
  }

  /// First unrecognized critical option in a request, if any -- the
  /// caller turns this into 4.02 Bad Option.
  pub fn unknown_critical_option(&self, known: &[u16]) -> Option<u16> {
    self
      .options
      .iter()
      .map(|(n, _)| *n)
      .find(|n| option::is_critical(*n) && !known.contains(n))
  }
}

/// Minimal-length big-endian uint encoding (RFC 7252 section 3.2).
pub fn encode_uint(value: u32) -> Vec<u8> {
  let bytes = value.to_be_bytes();
  let skip = bytes.iter().take_while(|b| **b == 0).count();
  bytes[skip..].to_vec()
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
  Truncated,
  BadOptionEncoding,
  BadTokenLength,
  PayloadMarkerNoPayload,
}

impl std::fmt::Display for DecodeError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let s = match self {
      DecodeError::Truncated => "truncated message",
      DecodeError::BadOptionEncoding => "bad option encoding",
      DecodeError::BadTokenLength => "token length > 8",
      DecodeError::PayloadMarkerNoPayload => "payload marker with empty payload",
    };
    f.write_str(s)
  }
}

impl std::error::Error for DecodeError {}

/// Encodes `options` (sorted by number) and `payload` (preceded by 0xFF if
/// non-empty) -- the byte region both framings share.
pub fn encode_options_and_payload(msg: &Message, out: &mut Vec<u8>) {
  let mut opts: Vec<&(u16, Vec<u8>)> = msg.options.iter().collect();
  opts.sort_by_key(|(n, _)| *n);

  let mut prev: u16 = 0;
  for (number, value) in opts {
    let delta = number - prev;
    prev = *number;

    let (delta_nibble, delta_ext) = nibble_ext(u32::from(delta));
    let (len_nibble, len_ext) = nibble_ext(value.len() as u32);

    out.push((delta_nibble << 4) | len_nibble);
    out.extend_from_slice(&delta_ext);
    out.extend_from_slice(&len_ext);
    out.extend_from_slice(value);
  }

  if !msg.payload.is_empty() {
    out.push(0xFF);
    out.extend_from_slice(&msg.payload);
  }
}

/// Nibble + extension bytes for option delta/length (RFC 7252 section 3.1).
/// 15 (0xF) is reserved for the payload marker and never produced here:
/// values >= 269 use nibble 14 with a 2-byte extension.
fn nibble_ext(value: u32) -> (u8, Vec<u8>) {
  if value < 13 {
    (value as u8, Vec::new())
  } else if value < 269 {
    (13, vec![(value - 13) as u8])
  } else {
    (14, ((value - 269) as u16).to_be_bytes().to_vec())
  }
}

/// Decodes the shared options+payload region into `msg`.
pub fn decode_options_and_payload(mut buf: &[u8], msg: &mut Message) -> Result<(), DecodeError> {
  let mut number: u16 = 0;

  while !buf.is_empty() {
    if buf[0] == 0xFF {
      if buf.len() == 1 {
        return Err(DecodeError::PayloadMarkerNoPayload);
      }
      msg.payload = buf[1..].to_vec();
      return Ok(());
    }

    let byte = buf[0];
    buf = &buf[1..];

    let (delta, rest) = read_nibble_ext(byte >> 4, buf)?;
    buf = rest;
    let (len, rest) = read_nibble_ext(byte & 0x0F, buf)?;
    buf = rest;

    let len = len as usize;
    if buf.len() < len {
      return Err(DecodeError::Truncated);
    }

    number = number
      .checked_add(delta as u16)
      .ok_or(DecodeError::BadOptionEncoding)?;
    msg.options.push((number, buf[..len].to_vec()));
    buf = &buf[len..];
  }

  Ok(())
}

fn read_nibble_ext(nibble: u8, buf: &[u8]) -> Result<(u32, &[u8]), DecodeError> {
  match nibble {
    0..=12 => Ok((u32::from(nibble), buf)),
    13 => {
      let [first, rest @ ..] = buf else {
        return Err(DecodeError::Truncated);
      };
      Ok((13 + u32::from(*first), rest))
    }
    14 => {
      let [a, b, rest @ ..] = buf else {
        return Err(DecodeError::Truncated);
      };
      Ok((269 + u32::from(u16::from_be_bytes([*a, *b])), rest))
    }
    _ => Err(DecodeError::BadOptionEncoding),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn roundtrip(msg: &Message) -> Message {
    let mut buf = Vec::new();
    encode_options_and_payload(msg, &mut buf);
    let mut out = Message {
      code: msg.code,
      token: msg.token.clone(),
      ..Default::default()
    };
    decode_options_and_payload(&buf, &mut out).expect("decode");
    out
  }

  #[test]
  fn roundtrip_options_out_of_order_and_payload() {
    let mut msg = Message {
      code: code::GET,
      token: vec![1, 2, 3],
      ..Default::default()
    };
    // Push out of order; encoder must sort.
    msg.push_option(option::URI_QUERY, b"auth=tok".to_vec());
    msg.push_option(option::URI_PATH, b"device".to_vec());
    msg.push_option(option::URI_PATH, b"pigeons".to_vec());
    msg.set_option_uint(option::CONTENT_FORMAT, u32::from(content_format::JSON));
    msg.payload = b"{\"a\":1}".to_vec();

    let out = roundtrip(&msg);
    assert_eq!(out.path_segments(), vec!["device", "pigeons"]);
    assert_eq!(
      out.option_uint(option::CONTENT_FORMAT),
      Some(u32::from(content_format::JSON))
    );
    assert_eq!(
      out.first_option(option::URI_QUERY),
      Some(b"auth=tok".as_slice())
    );
    assert_eq!(out.payload, b"{\"a\":1}");
  }

  #[test]
  fn roundtrip_extended_deltas_and_lengths() {
    let mut msg = Message::default();
    // Delta 4, then delta 269-4 (2-byte ext), then a 300-byte value
    // (2-byte length ext) and a 20-byte value (1-byte length ext).
    msg.push_option(option::ETAG, vec![0xAB; 8]);
    msg.push_option(269, vec![0xCD; 300]);
    msg.push_option(270, vec![0xEF; 20]);

    let out = roundtrip(&msg);
    assert_eq!(out.options.len(), 3);
    assert_eq!(out.options[0], (option::ETAG, vec![0xAB; 8]));
    assert_eq!(out.options[1], (269, vec![0xCD; 300]));
    assert_eq!(out.options[2], (270, vec![0xEF; 20]));
  }

  #[test]
  fn uint_encoding_is_minimal() {
    assert_eq!(encode_uint(0), Vec::<u8>::new());
    assert_eq!(encode_uint(50), vec![50]);
    assert_eq!(encode_uint(0x0102), vec![1, 2]);
    assert_eq!(encode_uint(0x01020304), vec![1, 2, 3, 4]);

    let mut msg = Message::default();
    msg.set_option_uint(option::SIZE2, 0);
    assert_eq!(msg.option_uint(option::SIZE2), Some(0));
    msg.set_option_uint(option::SIZE2, 512_000);
    assert_eq!(msg.option_uint(option::SIZE2), Some(512_000));
  }

  #[test]
  fn payload_marker_without_payload_is_an_error() {
    let mut msg = Message::default();
    assert_eq!(
      decode_options_and_payload(&[0xFF], &mut msg),
      Err(DecodeError::PayloadMarkerNoPayload)
    );
  }

  #[test]
  fn truncated_option_value_is_an_error() {
    // Option with declared length 5 but only 2 bytes present.
    let mut msg = Message::default();
    assert_eq!(
      decode_options_and_payload(&[0x45, 1, 2], &mut msg),
      Err(DecodeError::Truncated)
    );
  }

  #[test]
  fn reserved_nibble_15_rejected() {
    let mut msg = Message::default();
    assert_eq!(
      decode_options_and_payload(&[0xF4, 0, 0, 0, 0], &mut msg),
      Err(DecodeError::BadOptionEncoding)
    );
  }

  #[test]
  fn unknown_critical_option_detection() {
    let mut msg = Message::default();
    msg.push_option(option::URI_PATH, b"x".to_vec());
    assert_eq!(msg.unknown_critical_option(&[option::URI_PATH]), None);
    // If-Match (1) is critical and unknown to us.
    msg.push_option(1, Vec::new());
    assert_eq!(msg.unknown_critical_option(&[option::URI_PATH]), Some(1));
    // Observe (6) is elective; must NOT trip the check.
    let mut msg2 = Message::default();
    msg2.push_option(option::OBSERVE, Vec::new());
    assert_eq!(msg2.unknown_critical_option(&[option::URI_PATH]), None);
  }

  #[test]
  fn dotted_codes() {
    assert_eq!(code::dotted(code::CONTENT), "2.05");
    assert_eq!(code::dotted(code::NOT_FOUND), "4.04");
    assert_eq!(code::dotted(code::CSM), "7.01");
  }
}
