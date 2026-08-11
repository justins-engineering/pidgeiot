//! Connection admission shared by both listeners: a global ceiling plus a
//! per-source-IP fair share, so no single address can hold the whole
//! table. Admission hands back an RAII permit -- both counts release on
//! drop, so every teardown path (handshake failure, deadline abort, idle
//! close, even a failed thread spawn dropping its closure) settles the
//! books without each path having to remember to.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// Ceiling on concurrent connections per listener, comfortably inside the
/// thread-per-connection model's range on a small VPS.
pub const MAX_CONNECTIONS: usize = 4096;
/// Fair share for one source IP: generous enough for a fleet behind a
/// single carrier-grade NAT address, small enough that filling the table
/// takes at least 16 distinct addresses.
pub const MAX_CONNECTIONS_PER_IP: usize = 256;

#[derive(Clone)]
pub struct ConnQuota {
  max_total: usize,
  max_per_ip: usize,
  counts: Arc<Mutex<Counts>>,
}

#[derive(Default)]
struct Counts {
  total: usize,
  per_ip: HashMap<IpAddr, usize>,
}

impl ConnQuota {
  pub fn new(max_total: usize, max_per_ip: usize) -> ConnQuota {
    ConnQuota {
      max_total,
      max_per_ip,
      counts: Arc::new(Mutex::new(Counts::default())),
    }
  }

  /// Cheap probe for a listener's pre-work fast path; admission itself is
  /// always decided by `try_acquire`.
  pub fn is_full(&self) -> bool {
    let counts = self.counts.lock().expect("quota lock");
    counts.total >= self.max_total
  }

  /// Admits `ip` unless the table or the address's fair share is
  /// exhausted. Dropping the permit is the only release path.
  pub fn try_acquire(&self, ip: IpAddr) -> Option<ConnPermit> {
    let mut counts = self.counts.lock().expect("quota lock");
    if counts.total >= self.max_total {
      return None;
    }
    // Read-then-insert rather than entry(): a refused address must not
    // leave a zero-count entry behind, or address churn grows the map
    // without bound.
    let held = counts.per_ip.get(&ip).copied().unwrap_or(0);
    if held >= self.max_per_ip {
      return None;
    }
    counts.per_ip.insert(ip, held + 1);
    counts.total += 1;
    drop(counts);
    Some(ConnPermit {
      counts: Arc::clone(&self.counts),
      ip,
    })
  }
}

/// One admitted connection's claim on the table. Held by the connection's
/// thread (or by the closure that would have become one), so its Drop runs
/// on every exit path.
pub struct ConnPermit {
  counts: Arc<Mutex<Counts>>,
  ip: IpAddr,
}

impl Drop for ConnPermit {
  fn drop(&mut self) {
    let mut counts = self.counts.lock().expect("quota lock");
    counts.total -= 1;
    if let Some(held) = counts.per_ip.get_mut(&self.ip) {
      *held -= 1;
      if *held == 0 {
        counts.per_ip.remove(&self.ip);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::net::Ipv4Addr;

  fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
  }

  #[test]
  fn per_ip_share_refuses_one_source_but_not_others() {
    let quota = ConnQuota::new(8, 2);
    let _a = quota.try_acquire(ip(1)).expect("first");
    let _b = quota.try_acquire(ip(1)).expect("second");
    assert!(quota.try_acquire(ip(1)).is_none(), "fair share exhausted");
    assert!(
      quota.try_acquire(ip(2)).is_some(),
      "other sources unaffected"
    );
  }

  #[test]
  fn global_ceiling_refuses_even_fresh_sources() {
    let quota = ConnQuota::new(2, 2);
    let _a = quota.try_acquire(ip(1)).expect("first");
    let _b = quota.try_acquire(ip(2)).expect("second");
    assert!(quota.is_full());
    assert!(quota.try_acquire(ip(3)).is_none());
  }

  #[test]
  fn dropping_a_permit_releases_both_counts() {
    let quota = ConnQuota::new(1, 1);
    let permit = quota.try_acquire(ip(1)).expect("first");
    assert!(quota.is_full());
    assert!(quota.try_acquire(ip(1)).is_none());
    drop(permit);
    assert!(!quota.is_full());
    assert!(
      quota.try_acquire(ip(1)).is_some(),
      "released slots are reusable"
    );
  }

  #[test]
  fn refusals_and_releases_leave_no_per_ip_residue() {
    let quota = ConnQuota::new(1, 1);
    let permit = quota.try_acquire(ip(1)).expect("admit");
    // A refusal of a never-seen address must not create an entry for it.
    assert!(quota.try_acquire(ip(2)).is_none(), "table full");
    drop(permit);
    let counts = quota.counts.lock().expect("quota lock");
    assert!(
      counts.per_ip.is_empty(),
      "zero-count entries must not accumulate"
    );
    assert_eq!(counts.total, 0);
  }
}
