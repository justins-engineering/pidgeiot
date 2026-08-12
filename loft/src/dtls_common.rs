//! RFC 7252 messaging-layer semantics shared by the DTLS listeners:
//! piggybacked ACKs for CON, NON for NON, RST for an empty CON "ping", and
//! duplicate detection with response replay. Lives outside `dtls.rs` so a
//! second DTLS implementation is a transport swap only -- everything above
//! the decrypted-datagram boundary stays this one code path.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::coap::message::{Message, code};
use crate::coap::udp::{Datagram, MessageType};
use crate::handler::{DeviceSession, Handler, Transport};
use crate::upstream::Dovecote;

/// Handles one decrypted datagram; returns the encoded reply datagram, if
/// any. A retransmitted CON must get the same ACK back, not a re-executed
/// request.
pub fn process_datagram(
  bytes: &[u8],
  session: &DeviceSession,
  handler: &Handler<Dovecote>,
  rt: &tokio::runtime::Handle,
  dedup: &mut DedupCache,
  next_mid: &mut u16,
) -> Option<Vec<u8>> {
  let dgram = match Datagram::decode(bytes) {
    Ok(d) => d,
    Err(e) => {
      tracing::debug!(error = %e, "undecodable CoAP datagram");
      return None;
    }
  };

  // CoAP ping: empty CON -> RST echoing the message id (RFC 7252 4.3).
  if dgram.message.code == code::EMPTY {
    if dgram.message_type == MessageType::Confirmable {
      return Some(
        Datagram {
          message_type: MessageType::Reset,
          message_id: dgram.message_id,
          message: Message::default(),
        }
        .encode(),
      );
    }
    return None;
  }

  if !code::is_request(dgram.message.code) {
    // A stray response/signaling code over UDP -- ignore.
    return None;
  }

  if let Some(cached) = dedup.get(dgram.message_id) {
    tracing::debug!(
      mid = dgram.message_id,
      "duplicate request, replaying response"
    );
    return Some(cached.to_vec());
  }

  let response = rt.block_on(handler.handle(&dgram.message, session, Transport::Udp));

  let (message_type, message_id) = match dgram.message_type {
    MessageType::Confirmable => (MessageType::Acknowledgement, dgram.message_id),
    _ => {
      *next_mid = next_mid.wrapping_add(1);
      (MessageType::NonConfirmable, *next_mid)
    }
  };

  let encoded = Datagram {
    message_type,
    message_id,
    message: response,
  }
  .encode();

  dedup.insert(dgram.message_id, encoded.clone());
  Some(encoded)
}

/// Seed for the NON-response message id counter (which isn't tied to a
/// request mid the way ACKs are).
pub fn rand_u16() -> u16 {
  let mut b = [0u8; 2];
  let _ = openssl::rand::rand_bytes(&mut b);
  u16::from_be_bytes(b)
}

/// Duplicate-detection cache, per connection: message id -> the encoded
/// response already sent. RFC 7252's EXCHANGE_LIFETIME is ~247s; entries
/// live a bounded 150s (a client still retransmitting a mid after that has
/// long since given up per default transmission parameters), capped to
/// keep a hostile peer from ballooning memory.
pub struct DedupCache {
  entries: HashMap<u16, (Vec<u8>, Instant)>,
  ttl: Duration,
  cap: usize,
}

impl DedupCache {
  pub fn new() -> DedupCache {
    DedupCache {
      entries: HashMap::new(),
      ttl: Duration::from_secs(150),
      cap: 256,
    }
  }

  pub fn get(&mut self, mid: u16) -> Option<&[u8]> {
    let now = Instant::now();
    let ttl = self.ttl;
    self
      .entries
      .retain(|_, (_, at)| now.duration_since(*at) < ttl);
    self.entries.get(&mid).map(|(bytes, _)| bytes.as_slice())
  }

  pub fn insert(&mut self, mid: u16, response: Vec<u8>) {
    if self.entries.len() >= self.cap {
      // Evict the oldest entry rather than refusing to record.
      if let Some(oldest) = self
        .entries
        .iter()
        .min_by_key(|(_, (_, at))| *at)
        .map(|(k, _)| *k)
      {
        self.entries.remove(&oldest);
      }
    }
    self.entries.insert(mid, (response, Instant::now()));
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn dedup_replays_within_ttl() {
    let mut cache = DedupCache::new();
    cache.insert(7, vec![1, 2, 3]);
    assert_eq!(cache.get(7), Some([1, 2, 3].as_slice()));
    assert_eq!(cache.get(8), None);
  }

  #[test]
  fn dedup_expires() {
    let mut cache = DedupCache {
      entries: HashMap::new(),
      ttl: Duration::ZERO,
      cap: 256,
    };
    cache.insert(7, vec![1]);
    assert_eq!(cache.get(7), None);
  }

  #[test]
  fn dedup_evicts_oldest_at_cap() {
    let mut cache = DedupCache {
      entries: HashMap::new(),
      ttl: Duration::from_secs(150),
      cap: 2,
    };
    cache.insert(1, vec![1]);
    std::thread::sleep(Duration::from_millis(5));
    cache.insert(2, vec![2]);
    std::thread::sleep(Duration::from_millis(5));
    cache.insert(3, vec![3]);
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.get(1), None, "oldest entry evicted");
    assert!(cache.get(2).is_some());
    assert!(cache.get(3).is_some());
  }
}
