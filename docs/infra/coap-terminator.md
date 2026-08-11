# CoAP terminator (`loft`)

`loft` is the workspace's native Rust service that terminates the CoAP device transports —
CoAP-over-DTLS/UDP (`coaps://`, the primary transport for PSM'd cellular devices) and
CoAP-over-TLS/TCP (`coaps+tcp://`, RFC 8323) — and translates them onto dovecote's ordinary
HTTP device routes. The Workers runtime is HTTP-only and cannot terminate raw UDP or CoAP
framing, so this runs as a container on the VPS (same host that runs Kratos), one process,
both listeners on port 5684.

The wire-visible behavior (resource map, block-wise transfer, status mapping, client
examples) is documented in `docs/api.md` → "CoAP device surface". This file is the
deployment/operations side.

## Trust chain

```
device ──DTLS-PSK / TLS-PSK──▶ loft ──HTTPS──▶ dovecote ──▶ pigeon's Durable Object
         identity = pigeon id         Authorization:            verify_device_token
         key      = tls_psk_secret    Bearer <device token>
```

1. A `Coap`-connector pigeon is minted three credentials by its own Durable Object
   (`create` / `token/refresh`, always rotated together):
   - `tls_psk_identity` — the pigeon's id.
   - `tls_psk_secret` — a 32-char hex PSK. Short on purpose: RFC 4279 only obliges TLS
     stacks to support PSKs up to 64 bytes, mbedTLS's default `MBEDTLS_PSK_MAX_LEN` is 32,
     libcoap's client caps at 64. (The 92-char bearer token is unusable as a PSK on exactly
     the stacks CoAP targets — found the hard way with a real libcoap handshake failing on
     bad record MAC.)
   - `token` — the ordinary device bearer token.
2. At handshake time, loft's PSK callback resolves identity → (PSK, token) through
   dovecote's `GET /internal/coap-psk/:pigeon_id`, authenticated by the shared
   `COAP_SERVICE_SECRET`. Positive results are cached 60s, negatives 10s; a stale positive
   may be served for up to 5 min only if dovecote is unreachable.
3. After the handshake, loft acts as a plain device-side HTTP client: every proxied request
   carries `Authorization: Bearer <token>` and is verified cryptographically by the pigeon's
   own Durable Object — loft adds exactly one check of its own (Uri-Path pigeon id must
   equal the handshake identity) and weakens nothing.
4. Scope of `COAP_SERVICE_SECRET`: per-identity device credentials only (each still
   DO-verified per request). No dashboard, org, flock, or Postgres access. Treat it like any
   other production secret regardless.
5. Revocation: `token/refresh` overwrites the DO's verification key, so old tokens die
   instantly. The 60s PSK cache means an OLD PSK can still complete a *handshake* for up to
   60s after a refresh — but every request on that session presents the revoked token and
   401s at the DO, so no data access survives the refresh.

## Configuration (env vars)

| Var | Default | Meaning |
|---|---|---|
| `COAP_SERVICE_SECRET` | (required) | Shared secret with dovecote; same name on both sides |
| `LOFT_DOVECOTE_URL` | `https://api.pidgeiot.com` | Upstream base URL |
| `LOFT_UDP_LISTEN` | `0.0.0.0:5684` | DTLS listener |
| `LOFT_TCP_LISTEN` | `0.0.0.0:5684` | TLS/TCP listener |
| `LOFT_PSK_TTL_SECS` | `60` | Positive PSK cache TTL |
| `LOFT_LOG` | `info` | `tracing` filter |

No certificates anywhere: PSK ciphersuites (`PSK-AES128-CCM8` preferred, GCM/CBC-SHA256
fallbacks) authenticate both sides, so RFC 8323-over-PSK needs no server cert and constrained
clients need no CA store or clock. TLS is pinned to 1.2 on both listeners — the classic PSK
ciphersuites are a TLS 1.2 concept, and TLS 1.3's external-PSK mechanism is a different thing
constrained stacks don't speak.

Stateless across restarts: the only in-memory state is the PSK cache, per-connection DTLS
session state, and UDP duplicate-detection windows — all safely lost on restart (devices
rehandshake).

## VPS bring-up

In order:

1. **Secret** (once):
   ```sh
   openssl rand -base64 32
   cd dovecote && bunx wrangler secret put COAP_SERVICE_SECRET   # paste the value
   ```
2. **DNS**: `coap.pidgeiot.com` → A/AAAA record for the VPS, **DNS-only (grey cloud)**.
   Cloudflare's proxy carries neither raw UDP nor port 5684, so an orange-clouded record
   would silently break both transports. (This also means no CF DDoS shielding on 5684 —
   the DTLS cookie exchange and connection caps below are the mitigation.)
3. **Firewall**: open `5684/udp` and `5684/tcp` inbound on the VPS.
4. **Run it** (from a checkout on the VPS):
   ```sh
   cd infra/coap-terminator
   cp loft.env.example loft.env   # paste the same secret
   docker compose up -d --build
   ```
   The build context is the repo root; `.dockerignore` keeps `loft.env`, `secrets.env`,
   and the other credential files out of it. If a `--build` ever ran on this host before
   those exclusions existed, the secret is sitting in a cached build layer — run
   `docker builder prune --all` there and rotate `COAP_SERVICE_SECRET` (both owner-gated
   operations) before trusting that host's cache again.
5. **Verify** (libcoap's `coap-client`; needs a build with `MAX_KEY >= 32`, any stock one):
   ```sh
   # Create a Coap-connector pigeon in the dashboard, note id + tls_psk_secret, then:
   coap-client -m get -u <pigeon_id> -k '<tls_psk_secret>' \
     "coaps://coap.pidgeiot.com/device/pigeons/<pigeon_id>/shadow"
   ```
   A JSON shadow document back over DTLS is the whole chain working. `coaps+tcp://` same
   command, TCP transport.

Local dev loop: `docker-compose` stack + `wrangler dev --env dev` as usual, plus
`COAP_SERVICE_SECRET` in `dovecote/.dev.vars` (gitignored), then

```sh
COAP_SERVICE_SECRET=<same value> LOFT_DOVECOTE_URL=http://127.0.0.1:8787 \
  LOFT_UDP_LISTEN=127.0.0.1:5684 LOFT_TCP_LISTEN=127.0.0.1:5684 cargo run -p loft
```

`[env.dev]`'s `COAP_DEVICE_HOST = "127.0.0.1"` makes freshly minted dev pigeons point at it.

## Security posture (DTLS/UDP specifics)

- **Anti-amplification**: `SSL_OP_COOKIE_EXCHANGE` with a per-connection random cookie —
  OpenSSL answers the initial ClientHello with a small HelloVerifyRequest and does nothing
  further (no PSK lookup, no dovecote call) until the client echoes the cookie from its
  claimed source address. A spoofed source costs one small reply.
- **Unknown identities** are negative-cached (10s), so a garbage-identity flood cannot be
  amplified into a dovecote request flood.
- **Connection caps**: 4096 concurrent per listener, and a 256-connection fair share per
  source address (IPv6 counted per /64, so rotating interface identifiers doesn't dodge it);
  per-connection channel backpressure drops excess UDP datagrams; 30s wall-clock handshake
  deadline enforced inside the IO layer, so neither silence nor a paced byte-trickle can
  stretch it; 300s idle teardown.
- Threat model note: loft terminates TLS for devices, so it is trusted infrastructure in the
  same class as dovecote itself. It never holds dashboard credentials, and every device-side
  request it makes is still independently verified by the owning Durable Object.

## Known gaps and upgrade paths

- **No RFC 9146 Connection ID.** Surveyed with source-level evidence: rust-openssl (OpenSSL
  itself never implemented DTLS 1.2 CID — upstream issue #18724 remains open), webrtc-dtls
  (the Rust port never received pion's CID work), fortanix/rust-mbedtls (the C code is
  vendored with CID compiled in but zero Rust wrapper), wolfssl-rs (C supports it, bindings
  don't expose it, and the crate is GPL-licensed). Consequence for PSM'd cellular devices:
  when the NAT mapping dies during sleep, the next wake is a fresh handshake (~2 RTT) rather
  than a seamless resume. Paths, in preference order: (1) contribute the
  `mbedtls_ssl_set_cid` wrapper to fortanix/rust-mbedtls and swap the DTLS listener behind
  its existing trait seam; (2) wrap wolfSSL directly (license review first); (3) OpenSSL
  ships DTLS 1.3 (RFC 9147 has CID built in) and rust-openssl exposes it.
- **Server-side handshake retransmission timers**: the safe `openssl` crate doesn't expose
  `DTLSv1_get_timeout`/`DTLSv1_handle_timeout` (both are `SSL_ctrl` macros, so a small shim
  can reach them). Convergence currently relies on client-side retransmission, which every
  real DTLS client implements; the shim slots into `complete_handshake` in `loft/src/dtls.rs`
  when wanted.
- **Session resumption**: OpenSSL's server-side session cache is on by default (session
  IDs + TLS 1.2 tickets), but constrained PSK clients rarely attempt resumption, and with
  PSK suites a full rehandshake is already cheap (no certs, no signatures). Not load-bearing
  for correctness.

## Future: anycast / multi-region

The design is deliberately anycast-ready: no state outlives a connection except caches, so
any number of loft instances can run behind one IP. The natural path is Fly.io (anycast UDP
+ TCP on the same app, `fly.toml` service ports `5684/udp` + `5684/tcp`), pointing
`coap.pidgeiot.com` at the Fly anycast IP; each region's instance resolves PSKs against the
same dovecote. Two things to revisit at that point: DTLS handshakes must complete against a
single instance (Fly's UDP routing pins a 5-tuple to an instance, which suffices), and the
absent-CID story matters more (a NAT rebind can land on a different region's instance;
without CID that is just the same rehandshake cost as today).
