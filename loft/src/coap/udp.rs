//! RFC 7252 UDP datagram format: the 4-byte fixed header
//! (version/type/TKL, code, message id) around the shared options+payload
//! region from `super::message`.

use super::message::{self, DecodeError, MAX_TOKEN_LEN, Message};

pub const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
  Confirmable,
  NonConfirmable,
  Acknowledgement,
  Reset,
}

impl MessageType {
  fn from_bits(bits: u8) -> MessageType {
    match bits & 0b11 {
      0 => MessageType::Confirmable,
      1 => MessageType::NonConfirmable,
      2 => MessageType::Acknowledgement,
      _ => MessageType::Reset,
    }
  }

  fn bits(self) -> u8 {
    match self {
      MessageType::Confirmable => 0,
      MessageType::NonConfirmable => 1,
      MessageType::Acknowledgement => 2,
      MessageType::Reset => 3,
    }
  }
}

/// One UDP CoAP datagram: transport header + transport-agnostic message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
  pub message_type: MessageType,
  pub message_id: u16,
  pub message: Message,
}

impl Datagram {
  pub fn encode(&self) -> Vec<u8> {
    let tkl = self.message.token.len().min(MAX_TOKEN_LEN) as u8;
    let mut out = Vec::with_capacity(4 + self.message.payload.len() + 32);
    out.push((VERSION << 6) | (self.message_type.bits() << 4) | tkl);
    out.push(self.message.code);
    out.extend_from_slice(&self.message_id.to_be_bytes());
    out.extend_from_slice(&self.message.token[..tkl as usize]);
    message::encode_options_and_payload(&self.message, &mut out);
    out
  }

  pub fn decode(buf: &[u8]) -> Result<Datagram, DecodeError> {
    if buf.len() < 4 {
      return Err(DecodeError::Truncated);
    }
    if buf[0] >> 6 != VERSION {
      return Err(DecodeError::BadOptionEncoding);
    }

    let message_type = MessageType::from_bits(buf[0] >> 4);
    let tkl = (buf[0] & 0x0F) as usize;
    if tkl > MAX_TOKEN_LEN {
      return Err(DecodeError::BadTokenLength);
    }

    let code = buf[1];
    let message_id = u16::from_be_bytes([buf[2], buf[3]]);

    if buf.len() < 4 + tkl {
      return Err(DecodeError::Truncated);
    }
    let token = buf[4..4 + tkl].to_vec();

    let mut message = Message {
      code,
      token,
      ..Default::default()
    };
    message::decode_options_and_payload(&buf[4 + tkl..], &mut message)?;

    Ok(Datagram {
      message_type,
      message_id,
      message,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::super::message::{code, option};
  use super::*;

  #[test]
  fn roundtrip_con_get() {
    let mut message = Message {
      code: code::GET,
      token: vec![0xDE, 0xAD],
      ..Default::default()
    };
    message.push_option(option::URI_PATH, b"shadow".to_vec());

    let dg = Datagram {
      message_type: MessageType::Confirmable,
      message_id: 0x1234,
      message,
    };

    let bytes = dg.encode();
    // Header: ver=1, type=CON(0), tkl=2 -> 0x42; code 0.01; mid 0x1234.
    assert_eq!(&bytes[..4], &[0x42, 0x01, 0x12, 0x34]);
    assert_eq!(Datagram::decode(&bytes).unwrap(), dg);
  }

  #[test]
  fn roundtrip_ack_with_payload() {
    let message = Message {
      code: code::CONTENT,
      token: vec![7],
      options: vec![(option::CONTENT_FORMAT, vec![50])],
      payload: b"{}".to_vec(),
    };
    let dg = Datagram {
      message_type: MessageType::Acknowledgement,
      message_id: 1,
      message,
    };
    assert_eq!(Datagram::decode(&dg.encode()).unwrap(), dg);
  }

  #[test]
  fn rejects_wrong_version_and_short_input() {
    assert_eq!(Datagram::decode(&[0x02, 0x01]), Err(DecodeError::Truncated));
    // Version 0.
    assert_eq!(
      Datagram::decode(&[0x00, 0x01, 0, 0]),
      Err(DecodeError::BadOptionEncoding)
    );
    // TKL 9 is invalid.
    assert_eq!(
      Datagram::decode(&[0x49, 0x01, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
      Err(DecodeError::BadTokenLength)
    );
  }

  #[test]
  fn empty_con_ping_decodes() {
    // CoAP "ping": empty CON message (RFC 7252 section 4.3).
    let dg = Datagram::decode(&[0x40, 0x00, 0xBE, 0xEF]).unwrap();
    assert_eq!(dg.message_type, MessageType::Confirmable);
    assert_eq!(dg.message.code, code::EMPTY);
    assert_eq!(dg.message_id, 0xBEEF);
  }
}
