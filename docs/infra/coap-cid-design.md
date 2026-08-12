# loft: server-side RFC 9146 Connection ID — design

**Status: signed off. The design survived owner review intact; the review's three open
questions are resolved as the recorded decisions at the end of this document, and
implementation proceeds per the phased rollout below.**

This closes the first entry under "Known gaps and upgrade paths" in
[`coap-terminator.md`](./coap-terminator.md): no RFC 9146 Connection ID on the DTLS/UDP
listener. It is a design and decision record, at implementer detail, for swapping only that
listener onto Debian's system Mbed TLS 3.6 behind a first-party FFI shim — the direction
`coap-terminator.md` itself anticipated ("swap the DTLS listener behind the handler seam").
The TLS/TCP listener, the PSK provisioning path, the CoAP codecs, and the upstream HTTPS leg
are all untouched.

## Problem

A DTLS 1.2 association is identified by its 5-tuple. loft's demux (`loft/src/dtls.rs`)
routes datagrams to sessions by exact source `SocketAddr`, which is the only thing DTLS 1.2
without extensions gives a server to route on. PSM'd cellular devices — the transport's
design case — sleep through their NAT bindings on purpose; when the mapping expires or the
carrier renumbers, the device's next datagram arrives from a source address loft has never
seen. Today that datagram falls into the cookie path, the device's records go unanswered, it
burns its full retransmission window (~93s on the default DTLS backoff schedule as measured
on the real device stack), and only then re-handshakes. Every NAT rebind costs roughly a
minute and a half of deafness plus a fresh handshake and PSK lookup — for devices whose whole
duty cycle is "wake, send one reading, sleep".

RFC 9146 fixes exactly this: the server issues a Connection ID during the handshake, the
device then prefixes each encrypted record with it (content type 25, `tls12_cid`), and the
server routes by CID instead of 5-tuple. A rebind becomes one log line, not a re-handshake.

The device side is already done and wire-proven: the `~/pigeon` Zephyr library (mbedTLS
underneath) offers RFC 9146 final (extension 54) behind `CONFIG_PIGEON_COAP_DTLS_CID`,
verified against a CID-capable libcoap server including proxy-simulated rebind survival. The
device offers a **zero-length CID of its own** — meaning server→device records must carry
*no* CID and remain plain content type 23; the device silently discards CID-bearing records
(`MBEDTLS_SSL_UNEXPECTED_CID_IGNORE`). Only the server side is missing, and OpenSSL — loft's
current DTLS stack — has no RFC 9146 implementation at all.

Also worth stating plainly: the current fleet does not offer CID, and nothing here may
change its behavior. A client that offers nothing must be served bit-for-bit as today.

## Decision

**Port the DTLS/UDP listener to Debian trixie's system Mbed TLS 3.6 (dynamically linked,
apt-patched), through a new first-party FFI crate `mbedtls-ffi-shim` modeled on the existing
`dtls-ffi-shim`. Ship dual-stack behind an env-var switch with an in-process canary listener,
cut over once wire-proven, then delete the OpenSSL DTLS path.** The TLS/TCP listener stays on
OpenSSL; the upstream leg stays on rustls.

Three independent design passes (different lenses: protocol correctness, minimal owned patch
surface, device/server stack convergence) all converged on system mbedTLS 3.6 via dynamic FFI
— no candidate disagreement on the library, only on rollout and demux mechanics. Three
independent reviews then scored the passes against the real sources. This document is the
winning design plus the reviewers' consensus grafts:

- The winner was preferred for getting every load-bearing RFC 9146 subtlety right where the
  others each dropped one: the `by_cid` route must exist **before** the handshake completes
  (the client's Finished is an epoch-1, CID-bearing record — one competing design registered
  the CID only post-handshake and would have dropped its own happy path's Finished); the
  address-keyed route must be **retained** until the first authenticated CID-routed read
  (epoch-0 flight retransmits still route by address — another design dropped it at handshake
  completion); and mbedTLS's cookie outcome needs explicit disambiguation (below), which only
  the winner specified.
- Grafted from the runner-up: the phased rollout narrative (the winner's biggest gap — it had
  work items but no cutover story), the two-lever rollback posture, and the PSK
  handshake-failure alert-parity check with its cheap normalization fallback.
- Grafted from the third design: the runtime-stage `nm` symbol gate in the Dockerfile (the
  moral successor of the existing `openssl ciphers` PSK gate), the startup library-version
  log line, and the observation that an on-VPS canary shares the host's egress address, so
  `COAP_SERVICE_ALLOWED_IPS` already admits its PSK lookups with zero dovecote changes.

Why now, and why this shape: OpenSSL will not grow RFC 9146 on any horizon (details under
"Library choice"), so *some* second DTLS implementation is unavoidable. Given that, the
smallest credible change is the one that touches only the DTLS listener behind the existing
`handler::Handler` seam, keeps every cap/deadline/quota semantic of the current listener,
vendors zero C code, and leaves both crypto libraries on apt's patch cadence — extending
verbatim the recorded rationale for dynamic-linking OpenSSL rather than owning static-linked
CVE rebuilds.

## Library choice

**Chosen: Mbed TLS 3.6 LTS as packaged by Debian trixie** (build: `libmbedtls-dev`
3.6.5-0.1~deb13u1; runtime: `libmbedtls21`, pulling `libmbedcrypto16`/`libmbedx509-7`).

- **CID maturity**: RFC 9146 *final* (extension 54), non-experimental and enabled by default
  since mbedTLS 3.3.0 — over three years fielded. Debian's `debian/rules` only *adds* config
  (`MBEDTLS_THREADING_PTHREAD`, CMAC, SRTP) and never touches
  `MBEDTLS_SSL_DTLS_CONNECTION_ID` or the PSK/CCM/GCM defaults, so the shipped shared library
  has server-side CID, PSK key exchange, all three suites loft pins, and HelloVerifyRequest
  cookies compiled in. (Verified from the package source; a runtime `nm`/handshake spike is
  the rollout's hard Phase 0 gate before any porting effort.)
- **Wire convergence**: the fleet *is* mbedTLS 3.x. Same implementation on both ends of the
  CID extension removes whole classes of cross-stack edge cases and leaves one upstream to
  track when DTLS 1.3 (RFC 9147, CID native) eventually lands in both.
- **Patch surface**: dynamic link against the apt-managed `.so`. Upstream 3.6 LTS is
  supported to at least March 2027 and Debian tracks it actively for trixie's lifetime. We
  own zero crypto code — the only compiled first-party C is a small non-crypto glue TU
  (below).
- **Licensing**: dual Apache-2.0 OR GPL-2.0-or-later; consumed under Apache-2.0. No copyleft
  obligations, nothing triggered by a future on-prem or distributed loft offering.

Epitaphs for the alternatives, one line each (all disqualifying facts source-verified during
the design survey):

- **OpenSSL (stay put)**: no RFC 9146 at all; the upstream feature request has been open
  since 2022 with zero PRs, and DTLS 1.3 is a one-person side branch — no path, ever.
- **GnuTLS**: no CID in any release; the tracking issue has sat open since 2019.
- **tinydtls**: no merged server-side CID.
- **fortanix/rust-mbedtls** (the old preference in `coap-terminator.md`, now overridden):
  maintenance mode, and it statically vendors mbedTLS C 2.28 whose CID is the draft-05 wire
  format — **incompatible with the fleet's RFC 9146 final** — the exact owned-CVE-rebuild
  surface this design exists to avoid, twice over.
- **wolfSSL**: three stacked costs — Debian ships 5.7.2 which *predates* CID (we'd vendor and
  own the patch cadence), core went GPLv3-only (commercial licensing on any future
  distribution), and the maintained Rust crate exposes no CID anyway.
- **Pure Rust**: no released crate implements RFC 9146 server-side.
- **Sidecar terminators** (pion/dtls in Go, Californium on the JVM): zero FFI, but a second
  runtime + binary + unit to patch on one small VPS, a split PSK-lookup trust boundary (a
  second holder of `COAP_SERVICE_SECRET` or a new loopback lookup hop), and split logs —
  strictly more owned operational surface. Priced as the fallback if the Phase 0 spike fails
  on a concrete fact; not otherwise pursued.

### Binding mechanics (why a C glue file exists)

`dtls-ffi-shim` gets away with hand-declared externs because `openssl-sys` already ships
opaque types. mbedTLS is different in two ways that force a small compiled TU:

1. Its contexts (`mbedtls_ssl_context`, `mbedtls_ssl_config`, `mbedtls_ssl_cookie_ctx`) are
   caller-allocated structs whose size depends on compile-time config. Rust must never embed
   those layouts — a Debian security update must not be able to skew an allocation. The TU
   provides `calloc`-plus-`mbedtls_*_init` allocator/free wrappers behind opaque pointers,
   sized at glue compile time against the same headers the runtime `.so` was built from (the
   build and runtime stages already move together on trixie by existing invariant, and the
   `libmbedtls.so.21` soname enforces ABI at load).
2. Several conf setters are `static inline` in the headers and don't exist as linkable
   symbols — `mbedtls_ssl_conf_min_tls_version`/`_max_tls_version`,
   `mbedtls_ssl_set_user_data_p`/`_get_user_data_p` among them. (One reviewer verified this
   directly: a design that hand-declared these as externs would not link.) The TU wraps them
   as real functions.

That's the whole TU: allocators, inline-setter wrappers, zero cryptography, on the order of
two hundred lines, compiled by `build.rs` + `cc` inside the `rust:1-trixie` build stage. It
also carries compile-time feature gates —

```c
#if !defined(MBEDTLS_SSL_DTLS_CONNECTION_ID) || !defined(MBEDTLS_KEY_EXCHANGE_PSK_ENABLED) \
  || !defined(MBEDTLS_SSL_DTLS_HELLO_VERIFY) || !defined(MBEDTLS_SSL_PROTO_DTLS)
#error "system mbedTLS lacks a feature loft's DTLS listener requires"
#endif
```

— the mbedTLS analogue of the Dockerfile's `openssl ciphers` PSK gate, so a Debian config
regression fails the image build loudly instead of failing handshakes quietly. Everything
else is hand-declared `extern "C"` against exported `libmbedtls` symbols with opaque pointer
types, the same discipline as `dtls_ffi.rs`, plus safe Rust wrappers that make each
`mbedtls_ssl_context` `Send` but deliberately `!Sync` — one thread owns a context for its
lifetime, which is the structural answer to the mbedTLS 3.x thread-safety concerns that
stalled the fortanix crate (Debian's `MBEDTLS_THREADING_PTHREAD` remains a backstop we don't
rely on). RNG is a Rust `extern "C"` `f_rng` over `getrandom(2)` — no mbedTLS
entropy/ctr_drbg contexts anywhere, so there is no shared crypto-context state to reason
about and nothing new for `PrivateDevices`/`@system-service` to object to.

## Architecture

New code: the `mbedtls-ffi-shim` workspace crate; a `loft/src/dtls_mbed.rs` listener; and
`loft/src/dtls_common.rs`, which receives the RFC 7252 messaging layer **moved, not
rewritten** out of `dtls.rs` (`process_datagram`, `DedupCache`, `DeviceSession`
construction) so both DTLS listeners share it during the dual-stack window. `dtls.rs`
(OpenSSL) stays compiled and selectable until cleanup. `tls_tcp.rs` is untouched.

### Socket and demux

Today's shape is kept exactly: one unconnected `UdpSocket`, a listener thread `recv_from`
loop, per-session `mpsc` channels (depth 32, full = drop, UDP semantics), writes pinned via
`send_to` on a `try_clone()`d handle. The channel payload changes from `Vec<u8>` to
`(Vec<u8>, SocketAddr)` — the session thread needs each datagram's source for migration.

`ConnMap` becomes two maps under the existing single mutex:

```rust
by_addr: HashMap<SocketAddr, SessionHandle>,
by_cid:  HashMap<[u8; 8], SessionHandle>,
// SessionHandle { tx: SyncSender<(Vec<u8>, SocketAddr)>, established: Arc<AtomicBool>, conn_id: u64 }
```

Demux order per datagram, inspecting only the first record's header:

1. **`byte[0] == 25`** (`tls12_cid`), `bytes[1..3] == 0xFEFD`, `len >= 21`: the server CID
   sits at a fixed offset — type(1) + version(2) + epoch(2) + seq(6) = 11, so
   `cid = bytes[11..19]` (CID length is a compile-time const 8; the demux parser and
   `mbedtls_ssl_conf_cid` must agree, so it is deliberately not configurable). `by_cid` hit →
   push `(datagram, src)`. Miss → **silent drop** with a rate-limited counter: a type-25
   record can never begin a handshake, must never reach the cookie path, and answering it
   would be an amplification primitive. A stale post-restart session lands here; the device
   recovers through its own timeout + re-handshake, which is today's restart semantics
   unchanged.
2. **Anything else**: `by_addr` lookup. On a hit, one new guard: if the record is a
   *plaintext handshake record* (`byte[0] == 22`, epoch bytes `[3..5] == 0`) **and** the
   session's `established` flag is set, do *not* feed the stale session — fall through
   to (3). This fixes the known source-address-reuse lockout (a rebooted or replacement
   device handed a still-mapped ip:port by its NAT used to be deaf until the old session
   idled out) as a side effect. The `established` guard is what makes it safe: during a
   lossy in-flight handshake, a retransmitted cookied ClientHello still routes to the
   session that owns it. Stated plainly so nobody rediscovers it as a bug: during that
   pre-`established` window a *spoofed* plaintext ClientHello from the mapped address is
   also fed to the in-flight session, where it is DTLS flight-machinery noise — it cannot
   evict the handshake, cannot complete one (the cookie never re-runs on a promoted
   context), and costs at most a retransmitted server flight. Otherwise push to the
   session channel as today.
3. **Miss** → the stateless pending-listen path.

### Pending listen, cookies, HelloVerifyRequest

The invariant to preserve is the current one, verbatim: *fully stateless pre-cookie — an
unverified source owns no map entry, no channel, no thread, and costs at most one reply
smaller than its ClientHello.* mbedTLS's native HVR machinery replaces both `DTLSv1_listen`
and the hand-rolled HMAC cookie code (which gets deleted — the library's cookie module is
the same HMAC-SHA256-over-claimed-address construction, with built-in timed key rotation; a
process restart still only invalidates in-flight exchanges).

One long-lived pending `mbedtls_ssl_context` + `ConnState` lives on the listener thread,
`mbedtls_ssl_session_reset` between attempts, rebuilt lazily after promotion or poisoning —
today's single shared `PendingListen`, one-for-one. Per unknown-source datagram:

- `session_reset`; re-set MTU 1400 (`mbedtls_ssl_set_mtu` — no `SSL_clear` ordering gotcha
  in mbedTLS; the workaround comment dies with the OpenSSL path);
- `mbedtls_ssl_set_client_transport_id(ssl, <src ip:port bytes>)` — the cookie's address
  binding, replacing the ex-data peer stamping;
- mint a fresh 8-byte CID from `getrandom`, check it against `by_cid` under the demux lock
  (regenerate on the effectively-impossible collision — at most one unpromoted CID exists at
  a time because the single listener thread mints and promotes serially, so a mint-time
  check is sufficient and no reservation set is needed), then
  `mbedtls_ssl_set_cid(ssl, ENABLED, cid, 8)` — **must precede the handshake and must be
  re-applied after every `session_reset`**;
- place the datagram in the `ConnState`'s one-shot buffer and step
  `mbedtls_ssl_handshake` once.

Outcome mapping — and the one place mbedTLS is genuinely trickier than `DTLSv1_listen`,
which returned a three-way verdict. mbedTLS returns `MBEDTLS_ERR_SSL_HELLO_VERIFY_REQUIRED`
when it has emitted an HVR (→ today's `Retry`: zero state kept), but both "garbage record
silently discarded in DTLS mode" and "cookie-verified ClientHello consumed, flight 2
written" surface as `WANT_READ`. The disambiguation is a thin first-party wrapper installed
via `mbedtls_ssl_conf_dtls_cookies` around `mbedtls_ssl_cookie_check` that records
verified/not-verified into a listener-thread-confined flag per attempt:

- HVR error → HVR already went out through our `f_send` straight to the claimed source →
  reset, stay stateless.
- `WANT_READ` with the cookie flag set → a cookie-verified ClientHello was parsed and the
  PSK flight 2 (ServerHello with our CID in the extension, ServerKeyExchange hint,
  ServerHelloDone — cheap, no certificates) was already written inline from the listener
  thread → **promote**.
- Anything else → reset, drop.

The cookie context and its flag are touched only on the listener thread; a promoted context
never re-checks a cookie (renegotiation stays at its disabled default), so nothing about
this is shared across threads.

### Promotion

On promote: charge the per-IP quota **now** (unchanged point in the flow — post-cookie, on a
proven address; same shared `ConnQuota`, same 4096 global / 256-per-/64 caps); insert
`by_addr[src]` **and, provisionally, `by_cid[cid]`** — the client's Finished is an epoch-1
record already carrying our CID, so the CID route must exist before the session thread ever
reads; move the entire pending context + boxed `ConnState` to a spawned `dtls-{peer}` thread
(the context is consumed, not reset — the listener lazily rebuilds, today's pattern; all
callback context pointers are heap boxes, pointer-stable across the move). A failed spawn
releases the permit and removes both map entries through the existing RAII shape. Each
session takes a `conn_id` from a process-global `AtomicU64` at construction (both
transports); it doubles as the identity for guarded map removal (remove an entry only if it
still carries my `conn_id`) and as the Block1 rekey below.

### Session thread, IO, retransmission

`ConnState` (the `DgramIo` successor, wired in as `p_bio`, `p_timer`, and per-connection
user data via the shim-wrapped `mbedtls_ssl_set_user_data_p`) holds the socket clone, the
current committed peer, the staged last-source slot, the receiver, the one-shot pending
buffer, the identity/token slot, two `Instant`s of timer state, and the handshake/idle
deadlines. Callbacks are all Rust `extern "C"`:

- `f_send` → `send_to(current_peer)`.
- `f_recv_timeout` → drain the pending buffer first, else
  `rx.recv_timeout(min(READ_TICK, mbedTLS timer remaining, wall-clock deadline remaining))`.
  On timer expiry it returns `MBEDTLS_ERR_SSL_TIMEOUT`, which makes **mbedTLS drive its own
  flight retransmission** through the mandatory `mbedtls_ssl_set_timer_cb` pair — replacing
  OpenSSL's re-enter-`accept`-on-tick idiom. If a popped datagram is larger than the buffer
  mbedTLS offers, **drop the whole datagram and keep waiting — never truncate**. (This
  incidentally closes a latent min-copy truncation in today's `DgramIo::read`, which copies
  a datagram prefix when the caller's buffer is short; reviewers confirmed it against the
  source. OpenSSL always offers a max-size buffer so it never bites today, but the new IO
  layer must not inherit the hazard.)
- The 30s `HANDSHAKE_DEADLINE` is enforced *inside* the read callback as well as at the
  handshake loop — preserving the current code's can't-dodge-it property against a peer that
  keeps valid fragments flowing fast enough that the loop never sees a quiet tick.

Handshake loop: `mbedtls_ssl_handshake` until 0. On completion, read
`mbedtls_ssl_get_peer_cid`:

- **CID negotiated** (device offered): set `established`, log
  `cid=<hex> negotiated identity=<id> peer=<addr>`. The device's own CID is zero-length, so
  mbedTLS emits **no** CID on server→device records — they stay content type 23. This is
  load-bearing (the device blackholes unexpected-CID records) and is asserted on the wire in
  the test plan, never assumed. `by_addr` is **retained** for now: the client's epoch-0
  flight retransmits still route by address if our final flight was lost. It is removed only
  after the first successful post-handshake read of a CID-routed record — from then on the
  session is CID-only and the 5-tuple is free for other devices.
- **Not negotiated** (the entire current fleet): remove the provisional `by_cid` entry and
  behave exactly as today — addr-keyed routing, 300s idle deadline, bit-for-bit.

Steady state is today's loop verbatim: `ssl_read` → `dtls_common::process_datagram`
(dedup, ACK/NON/RST, `handler.handle`) → `ssl_write`.

### Address migration (RFC 9146 §6 anti-spoofing)

CIDs are plaintext on the wire, so an observed CID must never move the reply path by itself.
The listener **never** updates routing state on datagram arrival. The session thread stages
each fed datagram's source; only after `mbedtls_ssl_read` returns data — i.e. the record
passed AEAD *and* mbedTLS's default anti-replay window, so neither a spoofed nor a replayed
captured record can trigger it — does it compare staged source against the committed peer.
On change: commit (writes follow immediately), log one migration line, and remove any stale
`by_addr` entry that still carries this session's `conn_id`. A subsequent authentic record
from the original address flips it back — last-authenticated-wins. The accepted residual
(an off-path attacker racing a captured *not-yet-delivered* record from a spoofed source can
divert replies until the device's next authentic record) is the RFC's own accepted residual:
self-healing within one telemetry interval, no confidentiality impact.

### PSK callback and identity carriage

`mbedtls_ssl_conf_psk_cb(conf, cb, p)` with the `Arc<PskResolver>` as `p`. The callback runs
mid-handshake on the session's own OS thread — the same blocking-lookup-on-a-plain-thread
model as today, and the reason the PSK exchange never touches the listener thread (it fires
at ClientKeyExchange, post-promotion). The body becomes a stack-neutral helper factored out
of `tls_common.rs::psk_callback` — `resolve_psk_identity(&PskResolver, &[u8]) ->
Option<PskEntry>` with identical reject semantics (non-UTF-8 identity, resolver miss,
resolver error: all become one indistinguishable handshake failure) — shared by this
callback and the OpenSSL TCP callback so the two paths cannot drift. On a hit:
`mbedtls_ssl_set_hs_psk`, then stash `(identity, token)` into `ConnState` through the
per-connection user data — retiring `SESSION_EX_INDEX` ex-data for the DTLS path (the TCP
path keeps its ex-data, behind the shared helper). Resolver cache TTLs, negative/stale
semantics, and the `COAP_SERVICE_SECRET` contract are byte-for-byte unchanged.

One parity check carried from review: OpenSSL's `Ok(0)` reject and mbedTLS's nonzero-return
reject may emit different alert types (`unknown_psk_identity` vs a generic
`handshake_failure`) — a minor identity-probing oracle difference. The wire tests assert
unknown-identity and wrong-PSK failures are observably identical; if they aren't, the fix is
to set a random PSK on reject instead of failing the callback — a five-line change.

### Shared config

One `mbedtls_ssl_config`, built at startup, immutable thereafter (documented-shareable):
datagram transport, server endpoint; min = max = TLS 1.2 (shim-wrapped inline setters — the
same pin as `tls_common.rs`); ciphersuites `{0xC0A8 TLS_PSK_WITH_AES_128_CCM_8, 0x00A8
TLS_PSK_WITH_AES_128_GCM_SHA256, 0x00AE TLS_PSK_WITH_AES_128_CBC_SHA256}`, CCM8 first,
mirroring `PSK_CIPHER_LIST`; `mbedtls_ssl_conf_cid(conf, 8, MBEDTLS_SSL_UNEXPECTED_CID_IGNORE)`;
`conf_rng` = the getrandom callback; the cookie pair as above; anti-replay and renegotiation
left at their defaults (on and off respectively). 8-byte CIDs are comfortably under the
device's 32-byte echo cap and cheap on constrained uplinks (+8 bytes per record).

One deliberate behavior delta at cutover: mbedTLS selects ciphersuites in **server**
preference order, where the OpenSSL listener's default follows the client's. The fleet pins
CCM8 first on both ends, so nothing changes for it — but a third-party client that prefers
GCM while also offering CCM8 negotiated GCM against the OpenSSL listener and will negotiate
CCM8 against this one. Both are in the pinned suite list; noted so a post-cutover suite
change in a foreign client's logs reads as this, not as a defect.

### Deadlines, eviction, limits

`HANDSHAKE_DEADLINE` 30s, `READ_TICK` 1s, channel depth 32, dedup 150s/256, all connection
caps: unchanged. Idle splits by capability: non-CID sessions keep `IDLE_DEADLINE` 300s;
CID-negotiated sessions get `LOFT_DTLS_CID_IDLE_SECS` (proposed default 21600 = 6h), because
multi-hour PSM gaps are the design case and a 5-minute reaping would re-impose exactly the
timeout-plus-rehandshake cost CID exists to remove. The cost of a long deadline is one
mostly-parked thread + one quota permit per sleeping device — within the documented
thread-per-connection capacity plan (hundreds-to-low-thousands vs the 4096 cap; ~40KiB of
mbedTLS buffers per session ≈ 160MiB at the theoretical ceiling, inside `MemoryMax=1536M`) —
and a rebooting device strands its old CID session for up to the deadline (bounded; one
permit). There is deliberately **no identity-keyed session stealing in v1**; if the canary or
fleet growth shows permit pressure from reboot loops, one-session-per-pigeon eviction (the
WebSocket endpoint's precedent) is the specced fast-follow.

### Block1 rekey (correctness under migration)

`handler.rs` keys Block1 reassembly by `(peer-string, leaf)`. Under migration the peer
string changes mid-upload, orphaning the reassembly; under CID two sequential sessions can
also collide on a reused address. The key becomes `(conn_id, leaf)` — stable across
migrations, unique across connections, on **both** transports (`DeviceSession` gains
`conn_id: u64`; `peer` stays for logging and the session thread keeps it accurate
post-migration). This ships active for both stacks regardless of `LOFT_DTLS_STACK` — it is
stack-independent and independently testable.

### Config and wiring

- `LOFT_DTLS_STACK` = `openssl` | `mbedtls` (default `openssl` at first ship) — selects
  which implementation binds `LOFT_UDP_LISTEN`.
- `LOFT_DTLS_MBED_CANARY_ADDR` (optional, e.g. `0.0.0.0:5685`) — additionally runs the
  mbedTLS listener on that address while the primary stays OpenSSL, **sharing the same
  `ConnQuota` and `PskResolver` instances**. This is the canary mechanism: same process,
  same unit, same hardening, same credential path, no second service.
- `LOFT_DTLS_CID_IDLE_SECS` (default 21600).

Listener signatures keep `(config, resolver, handler, rt)`; any listener thread exiting
still kills the process (systemd `Restart` unchanged).

### Packaging and co-dependent artifacts

Three artifacts must move together (they have drifted before when only one was edited):

- **`loft/Dockerfile`**: build stage += `libmbedtls-dev`; runtime stage += `libmbedtls21`
  and an `nm -D /usr/lib/*/libmbedtls.so.21 | grep -q mbedtls_ssl_conf_cid` gate (the CID
  analogue of the existing `openssl ciphers` PSK gate — the shim's `#error` probes cover the
  build headers, this covers the runtime lib). The `libssl3t64` package and its PSK gate
  stay: the TCP path still needs them.
- **`infra/coap-terminator/loft.service`**: comments only — the library inventory
  (mbedTLS/OpenSSL/rustls split), the now-inaccurate "statically-linked C shim around
  `DTLSv1_listen`" line (the new shim genuinely compiles a small non-crypto C TU), and
  closing the `MemoryDenyWriteExecute` "UNVERIFIED" caveat once the canary has run under the
  real unit. mbedTLS needs no functional unit change: no JIT, no `dlopen`, no netlink, RNG
  via `getrandom` — but this is verified live in the canary, not assumed.
- **`docs/infra/coap-terminator.md`**: runtime-needs list (+`libmbedtls21`), the deploy
  verification recipe (+ a CID tcpdump spot-check), and rewriting the "Known gaps" CID entry
  as resolved, including recording the fortanix-preference override.

VPS one-time prep: `apt-get install libmbedtls21` before installing the new binary; the
post-deploy `ldd` check extends to `libmbedtls.so.21`. The binary logs
`mbedtls_version_get_number()` at startup so the journal always names the runtime library
version actually loaded.

## Migration / rollout

Every production step is gated on explicit repo-owner approval, per the repo rule — nothing
below is autonomous. There is no staging terminator (staging's PSK allowlist is a deliberate
deny-all), so the canary runs on the production VPS, made safe by: an in-process canary
listener on a separate port (never touching 5684 until cutover), a same-binary env-var stack
switch, a currently tiny CoAP fleet, loft's restart-cheap stateless design, and the
documented stop→install→start binary rollback.

**Phase 0 — spike (hard gate, ~half a day).** In the trixie build image: `apt-get install
libmbedtls-dev`, `nm -D` for `mbedtls_ssl_conf_cid` (+ confirm `mbedtls_ssl_set_user_data_p`
is header-inline as expected), then a probe linking `-lmbedtls -lmbedcrypto` running one
PSK-CCM8 loopback handshake with CID negotiated. This converts the debian/rules reading into
an artifact-level fact. If it fails structurally, stop and re-price the sidecar fallback —
do not proceed on hope. **Passed** on every criterion (packages `3.6.5-0.1~deb13u1`
throughout; the probe additionally proved the type-25/offset-11 uplink shape with
type-23-only downlink on the wire, and ran again dynamically linked against the runtime
stage's `libmbedtls21` alone). The VPS side needs no separate spike touch of its own: the
fact that matters there is the runtime `.so` resolving against the shipped binary, which
Phase 2's `apt-get install libmbedtls21` + extended `ldd` check proves inside the one
already-gated host window — and the host then never carries `-dev` or any toolchain at all,
rather than install-then-remove.

**Phase 1 — land the code, default off.** `mbedtls-ffi-shim`, `dtls_common.rs` extraction,
`dtls_mbed.rs`, config knobs, the `conn_id` Block1 rekey (active on both stacks), packaging.
Atomic, individually-buildable commits. Full local matrix green (test plan below), including
the netns rebind wire-proof and the no-CID regression cells that stand in for the current
fleet.

**Phase 2 — inert production deploy.** `apt-get install libmbedtls21`; build/extract via the
documented Docker flow; `cp /usr/local/bin/loft /usr/local/bin/loft.prev`; stop → install →
start with `LOFT_DTLS_STACK` unset. Proves link (`ldd` resolves `libmbedtls.so.21`), the
standing runbook checks, and several observed poll cycles of the live production device.
Zero intended behavior change; any anomaly → reinstall `loft.prev`.

**Phase 3 — canary listener.** Systemd drop-in adds
`Environment=LOFT_DTLS_MBED_CANARY_ADDR=0.0.0.0:5685`; a time-boxed `INPUT` accept for
5685/udp, **source-restricted to the test client's egress address as a required
precondition, not a nicety**: the canary deliberately shares the primary listener's
`ConnQuota`, and promotion charges a permit at cookie verification — before any PSK check —
so an attacker completing cookie exchanges against an open 5685 could drain the shared
4096-permit table and starve the production listener on 5684. The source restriction is
what keeps the canary's added pre-auth surface at zero; DTLS-PSK then gates everything
past it. Because the canary is the same process on the same host, its PSK lookups egress
from the VPS's own address — already in `COAP_SERVICE_ALLOWED_IPS`, so no dovecote change.
Drive the full rebind harness against it from the dev box (real dovecote, dedicated test
pigeon), then the hardware pass (below) at a 60s cadence for ≥72h with forced rebinds. Watch
for SIGSYS under the real unit (`MemoryDenyWriteExecute` is the unit's own named first
suspect), cgroup memory, and clean journal. Tear down the firewall rule when done.

**Phase 4 — cutover.** Drop-in `Environment=LOFT_DTLS_STACK=mbedtls`, restart in a
low-traffic window. `LOFT_DTLS_MBED_CANARY_ADDR` is removed in the same edit. Immediate
verification: the standing recipe (`systemctl status`, `journalctl -u loft`, `ss -tuln |
grep 5684`, `coap-client` PSK GET over **both** transports), a journal line confirming the
stack and library version, and several uninterrupted poll cycles of the live production
device — which, offering no CID, is precisely the fleet-regression canary. Rollback lever A:
remove the drop-in + restart (same binary, seconds). Lever B: reinstall `loft.prev`. Both
are fleet-safe: loft is stateless and devices re-handshake after any restart.

**Phase 5 — device fleet update.** The `CONFIG_PIGEON_COAP_DTLS_CID` default flip in
`~/pigeon` (its Kconfig help text already commits to this once the server supports CID)
lands alongside Phase 1 rather than waiting here — see the recorded decisions: with the
fleet still on pre-integration firmware, the flip triggers no release and reaches no device
before its first integration build. What remains in this phase is the fleet itself: one
fleet device is updated first and repeats the hardware acceptance against 5684. An
un-negotiated CID offer is a no-op, so mixed fleets are fine indefinitely; the nRF91 caveat
(CID rides the modem firmware, needs mfw 1.3.5+, degrades to CID-less with a logged warning
— which keeps working) is moot for the current fleet, whose units all run a new-enough
modem firmware.

**Phase 6 — cleanup (after two clean weeks post-cutover and owner sign-off).** Delete the
OpenSSL `dtls.rs` path and the `dtls-ffi-shim` crate; remove `LOFT_DTLS_STACK` and the
canary knob; keep OpenSSL for `tls_tcp.rs`; sweep the three co-dependent artifacts together.

## Risk register

- **Debian lib feature set** was verified from package sources, not a running box — if the
  shipped `.so` lacked CID the plan stalls. *Mitigation*: Phase 0 is a hard gate before any
  porting effort; the `#error` + `nm` gates keep it true forever after.
- **Pending-listen semantics differ from `DTLSv1_listen`**: promotion moves a
  partially-handshaken context (flight 2 already written from the listener thread), and a
  flood of cookie-valid ClientHellos costs the listener slightly more CPU per packet.
  *Mitigation*: PSK flight 2 is tiny (no certs); quota still charges at cookie-verify;
  garbage→reset stays allocation-free (asserted by shim tests); listener saturation is a
  canary watch item.
- **`WANT_READ` ambiguity** (garbage-discard vs cookie-verified) breaks the stateless
  posture if mishandled. *Mitigation*: the cookie-check recording wrapper is a specified
  first-class mechanism, not an implementation afterthought, with its own unit tests.
- **Server accidentally emitting CID-bearing downlink** would be silently blackholed
  on-device — indistinguishable from packet loss. *Mitigation*: mbedTLS emits no CID for a
  zero-length peer CID by design, and the harness and the on-hardware pass both *assert*
  zero type-25 records server→device; mandatory test, never an assumption.
- **CID-based traffic redirection** (CIDs are plaintext): spoofed or replayed datagrams must
  not move the reply path. *Mitigation*: RFC 9146 §6 rule — commit only after an
  authenticated, replay-window-passing read; explicit spoof and replay cells in the harness;
  unknown-CID datagrams dropped with zero state and zero response. Accepted residual: the
  RFC's own capture-and-race window, self-healing on the next authentic record.
- **Long-idle CID sessions** hold a thread + permit each for up to the idle deadline, and a
  reboot-looping device strands sessions. *Mitigation*: bounded by the unchanged caps and
  `MemoryMax`; deadline tunable without redeploy; identity-keyed eviction specced as a
  fast-follow if the canary shows pressure. Kernel conntrack is a separate budget loft can't
  see (pre-existing, now with longer-lived entries) — a soak watch item.
- **Shared-config thread safety** (what stalled the fortanix 3.x branch). *Mitigation*:
  structural — config immutable post-startup, contexts `Send`/`!Sync` single-thread-owned,
  RNG stateless `getrandom`, cookie state listener-confined; `MBEDTLS_THREADING_PTHREAD` is
  a backstop, not a dependency.
- **Retransmission idiom change** (timer callbacks + `MBEDTLS_ERR_SSL_TIMEOUT` instead of
  re-entering accept): bugs here surface only under loss. *Mitigation*: the shim ports
  `timeout_retransmission`'s test shape; the netns harness includes a netem lossy-link cell.
- **systemd hardening interaction** (`MemoryDenyWriteExecute`, `@system-service`,
  `RestrictAddressFamilies`): expected clean (no JIT/dlopen/netlink) but verified live in
  the canary under the real production unit, closing the unit's own "UNVERIFIED" note.
- **Dual-stack window** doubles DTLS review surface and can let the three co-dependent
  artifacts drift. *Mitigation*: both libs are apt-patched (no owned rebuilds), the
  coordinated-edit list is written into the work items, and cleanup has a fixed trigger.
- **ClientHello preemption misfire**: a client legitimately re-handshaking from its old
  5-tuple leaves its previous session to idle out holding a permit. Accepted — bounded, and
  strictly better than today's lockout.
- **Alert-parity oracle** (`unknown_psk_identity` vs generic failure): wire-tested; if
  distinguishable, normalize via random-PSK-on-reject.
- **Device logs lie**: Zephyr's CID-status getsockopt has a known uninitialized-read bug on
  native builds, so device-side "CID active" lines can be false. All acceptance criteria are
  wire-level or server-journal-level; device logs are explicitly non-authoritative.
- **Future dist-upgrade** to an mbedTLS 4.x with changed CID APIs. *Mitigation*: none needed
  now (trixie pinned); the soname and the shim's compile gates fail loudly rather than
  miscompile.

## Test plan

**Tier 1 — shim unit/integration** (`cargo test -p mbedtls-ffi-shim`, run inside the trixie
Docker build stage so it links the real Debian lib): feature-gate compile probes; the
listen flow (fresh CH → HVR, cookie-less repeat → HVR, cookied CH → accepted) with an
allocation-count guard across 10k garbage/HVR cycles; a garbage corpus (truncated records,
wrong version, type-25 runts) → drop with the pending context reusable after reset; timer-cb
driven flight retransmission over a lossy in-memory BIO (the port of
`timeout_retransmission`); PSK reject paths (non-UTF-8, miss, error) wire-indistinguishable;
loopback CID negotiation with a zero-length client CID asserting CCM8 selection and
`get_peer_cid`.

**Tier 2 — local netns NAT-rebind wire-proof** (the acceptance gate for the feature itself;
scripted under `scripts/test/`, no hardware, no production traffic). Topology: `cli ↔ nat ↔
srv` namespaces over veth pairs; nftables SNAT in `nat`; loft in `srv` with
`LOFT_DTLS_STACK=mbedtls` against a stub PSK endpoint honoring the
`/internal/coap-psk/:id` + bearer contract (harness-minted test PSKs only — the harness
needs no dovecote and no real credentials). Rebind = swap the SNAT rule to a new source port
+ `conntrack -F`; capture = tshark on the server-side veth. Cells:

1. **CID rebind survival** (primary): mbedTLS's own `ssl_client2` (`cid=1 cid_val=` — the
   empty value matches the device's zero-length offer exactly) exchanges, rebinds, exchanges.
   Assert: uplink records are type 25 with an 8-byte CID; **all** downlink records are type
   23; zero ClientHellos after the rebind; exactly one handshake + one migration line in the
   journal; the second exchange succeeds.
2. **Full-stack authoritative client**: the pigeon `native_sim` build with the CID Kconfig
   enabled, attached via TAP into the client namespace, polling a shadow through the
   stub-backed loft; rebind mid-cadence; next poll succeeds with no re-handshake and no
   retransmit stall, wire shows type-25 uplink.
3. **Mixed fleet / no-CID regression**: an OpenSSL-libcoap client and `ssl_client2 cid=0`
   behave exactly as today, including rebind → re-handshake recovery and 300s idle teardown;
   a CID session survives a synthetic >300s gap (both eviction edges tested by swinging
   `LOFT_DTLS_CID_IDLE_SECS`).
4. **Anti-spoof/redirect**: replay a captured authenticated type-25 record (and a
   bit-flipped variant, and an already-delivered genuine record) from a third address —
   reply path must not move, no migration line, original client still served.
5. **Unknown/garbage CID** → silent drop, no state, no response, no crash; type-25 runts
   below 21 bytes → drop.
6. **Address-reuse fix**: migrate a CID session off address A, immediately handshake a new
   client from exactly A → succeeds (routed to the cookie path); and the negative: a bare
   spoofed ClientHello alone must not evict an in-flight (non-established) handshake.
7. **Block1 across migration**: multi-block upload with a rebind between blocks completes
   with no 4.08 (validates the `conn_id` rekey), plus handler unit tests on the new key
   including the ported table-full admission test.
8. **Loss soak**: netem 10% on the veth across 50 handshakes+exchanges — all eventually
   succeed, no listener wedge, quota returns to baseline.
9. **Parallel listeners**: OpenSSL on one port + mbedTLS canary on another, both serving,
   shared quota decrementing once per connection.

The same matrix runs against the Docker compose path (its regression gate).

**Tier 3 — on-hardware** (canary phase; runs on the bench ESP32-C6 per the recorded
hardware decision — the board keeps its live production CoAP role until the canary actually
begins, its handoff is coordinated through the team lead first, and until then no
`/dev/tty*` it or anything else is attached to is touched). Rebuild with
the CID Kconfig, point at the canary port, soak ≥72h at 60s cadence behind real NAT; force
rebinds via the lab router's conntrack and via natural UDP-timeout expiry (cadence above the
NAT timeout). Acceptance is wire/journal evidence only: one handshake per boot across all
rebinds, ≥1 migration line per forced rebind, all shadow GETs and telemetry POSTs landing
end-to-end through dovecote, a short owner-approved VPS-side tcpdump confirming type-25
uplink / type-23-only downlink, zero SIGSYS/restarts, memory in budget. Post-cutover and
fleet-default phases re-run the same acceptance against 5684.

## Effort

S ≤ 0.5 day, M = 1–2 days, L = 3–5 days:

| Item | Size |
|---|---|
| Phase 0 spike (symbol gate + PSK-CCM8/CID loopback probe in the trixie image + once on the VPS) | S |
| `mbedtls-ffi-shim` crate (C TU, externs, safe wrappers, cookie/timer/BIO/RNG/PSK plumbing, unit tests) | L |
| `dtls_common.rs` extraction + `dtls_mbed.rs` listener (dual-map demux, pending listen, promotion, session loop, migration, idle split) | L |
| Shared `resolve_psk_identity` helper + mbedTLS callback + user-data identity carriage; TCP path re-pointed | S |
| Config/wiring: `LOFT_DTLS_STACK`, canary addr, CID idle knob, parallel-listener plumbing | S |
| Block1 rekey to `(conn_id, leaf)` on both transports + handler tests | S |
| Packaging/docs: Dockerfile gates, VPS apt step, unit comments, `coap-terminator.md` sweep | S |
| netns harness + tshark asserts + spoof/loss/negative cells + PSK stub | M |
| pigeon `native_sim` full-stack local e2e (TAP into the harness, CID build) | M |
| Rollout execution (inert deploy, canary soak, cutover + watch) | M hands-on; calendar adds the soak windows |
| Cleanup release (delete OpenSSL DTLS path + `dtls-ffi-shim`, doc sweep) | S, deferred |

Total ≈ 2.5–3 engineer-weeks hands-on, dominated by the shim and the listener port; calendar
time adds roughly 1–2 weeks of canary/cutover soak.

## Resolved questions (owner decisions)

The design review's three open questions, ruled on by the owner:

1. **The on-hardware pass runs on the bench ESP32-C6** — the unit currently serving as the
   live CoAP test device against production loft. Repurposing it is approved for the canary
   phase only: it keeps its production role until Tier 3 actually starts, and the handoff
   goes through a team-lead check-in first rather than being taken unilaterally. An
   ESP32-C6 remains the cleaner first proof in any case (its CID support is pure software,
   where nRF91 CID rides the modem firmware), and the local `native_sim` cell covers
   everything except real-NAT/RF behavior in the meantime.
2. **`LOFT_DTLS_CID_IDLE_SECS` defaults to 21600 (6h)**, as proposed. The fleet's PSM
   profile does not trend past that deadline, and the knob moves without a redeploy if it
   ever does.
3. **The fleet default flip (`CONFIG_PIGEON_COAP_DTLS_CID=y` in `~/pigeon`) is approved
   immediately**, ahead of soak testing, rather than riding a release train. The reason to
   wait — a flip triggering a firmware release, or staged FOTA images silently carrying the
   new default with stale build-time credentials — does not apply to this fleet: every unit
   is still on pre-pidgeiot-integration firmware, so nothing picks the default up before
   its first integration build, and every unit already runs a modem firmware new enough for
   CID. Soak testing follows the flip instead of gating it.
