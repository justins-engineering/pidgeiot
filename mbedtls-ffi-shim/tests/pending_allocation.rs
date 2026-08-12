//! The stateless-pending invariant, measured: driving garbage through the
//! reset/re-arm/step cycle allocates nothing on the Rust side, no matter
//! how many datagrams a flood delivers. (mbedTLS's own C-side churn is
//! bounded by session_reset and invisible here by construction -- this
//! guard is about the shim and its caller never accreting per-source
//! state.) Kept alone in its own test binary: the counting allocator is
//! process-global and other tests' threads would pollute it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
  unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
    ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    unsafe { System.alloc(layout) }
  }
  unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
    unsafe { System.dealloc(ptr, layout) }
  }
  unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    unsafe { System.realloc(ptr, layout, new_size) }
  }
}

#[global_allocator]
static COUNTER: CountingAlloc = CountingAlloc;

use mbedtls_ffi_shim::{
  CID_LEN, Config, HandshakeStatus, MbedIo, RecvOutcome, ResolvedPsk, SendOutcome, Session,
  TimerState,
};
use std::sync::Arc;

/// Feed transport that lends out one datagram per attempt and never
/// surrenders its buffer, so the measured loop can rotate a fixed corpus
/// through it with `mem::swap` and zero allocation.
struct FeedIo {
  dgram: Vec<u8>,
  consumed: bool,
  sends: usize,
}

impl MbedIo for FeedIo {
  fn send(&mut self, buf: &[u8]) -> SendOutcome {
    self.sends += 1;
    SendOutcome::Sent(buf.len())
  }
  fn recv(&mut self, buf: &mut [u8], _timer: &TimerState) -> RecvOutcome {
    if self.consumed {
      return RecvOutcome::WantRead;
    }
    self.consumed = true;
    if self.dgram.len() > buf.len() {
      // Whole-datagram drop, never truncation.
      return RecvOutcome::WantRead;
    }
    buf[..self.dgram.len()].copy_from_slice(&self.dgram);
    RecvOutcome::Data(self.dgram.len())
  }
}

#[test]
fn garbage_flood_allocates_nothing_rust_side() {
  let resolver = Box::new(|_identity: &[u8]| -> Option<ResolvedPsk> { None });
  let config = Arc::new(Config::server(resolver).expect("server config"));
  let mut server = Session::new(
    &config,
    FeedIo {
      dgram: Vec::new(),
      consumed: true,
      sends: 0,
    },
  )
  .expect("server session");
  let cid = [7u8; CID_LEN];
  let transport_id: &[u8] = b"198.51.100.99:40000";

  // Pre-built outside the measured window; rotated in by swap thereafter.
  let mut corpus: Vec<Vec<u8>> = vec![
    vec![0x16, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 40],
    vec![25, 0xfe, 0xfd, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 9, 9],
    vec![0xff; 64],
    vec![0x17; 1400],
  ];

  let cycle = |server: &mut Session<FeedIo>, dgram: &mut Vec<u8>| {
    server.reset().expect("reset");
    server.set_mtu(1400);
    server
      .set_client_transport_id(transport_id)
      .expect("transport id");
    server.set_own_cid(&cid).expect("own cid");
    config.clear_cookie_verified();
    let io = server.io_mut();
    std::mem::swap(&mut io.dgram, dgram);
    io.consumed = false;
    match server.handshake() {
      HandshakeStatus::Done | HandshakeStatus::HelloVerifyRequired => {
        panic!("garbage must not progress a handshake")
      }
      _ => {}
    }
    assert!(!config.take_cookie_verified());
    std::mem::swap(&mut server.io_mut().dgram, dgram);
  };

  // Warmup: first cycles may fault in lazy library state.
  for i in 0..corpus.len() {
    let mut d = std::mem::take(&mut corpus[i]);
    cycle(&mut server, &mut d);
    corpus[i] = d;
  }

  let before = ALLOCATIONS.load(Ordering::SeqCst);
  for _ in 0..2_500 {
    for i in 0..corpus.len() {
      let mut d = std::mem::take(&mut corpus[i]);
      cycle(&mut server, &mut d);
      corpus[i] = d;
    }
  }
  let allocated = ALLOCATIONS.load(Ordering::SeqCst) - before;
  assert_eq!(
    allocated, 0,
    "the garbage/reset cycle must not allocate per-datagram Rust-side"
  );
}
