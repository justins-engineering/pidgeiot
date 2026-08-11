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
3. **Firewall**: do this before step 4 — `docker compose up` publishes the CoAP ports the
   moment the container starts, regardless of host firewall policy. See "Firewall" below.
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

### Firewall

Docker's own port publishing bypasses whatever host firewall policy you'd expect to gate it.
`infra/coap-terminator/docker-compose.yml` publishes `5684:5684/udp` and `5684:5684/tcp` for
`loft`; those get installed as DNAT rules in `iptables`'s `PREROUTING` chain, and the resulting
traffic transits `FORWARD`, never `INPUT`. A `-P INPUT DROP` policy — however tight — has no
opinion on it: the port is world-reachable the instant the container starts, independent of any
INPUT-chain rule. The chain that actually governs container-published ports is `DOCKER-USER`,
which Docker consults ahead of its own generated rules and, unlike a hand-added `FORWARD` rule,
survives a `dockerd` restart (`dockerd` flushes and regenerates its own chains on every restart
but leaves `DOCKER-USER` alone).

Match on the WAN interface the traffic actually arrives on (`ens3`), not `docker0` — `docker0`
is wrong twice over: wrong direction (`DOCKER-USER` sees the packet already routed toward the
container, i.e. entering on `ens3` and headed for the bridge, not the reverse) and wrong bridge
(Compose creates its own per-project `br-<hash>`, not the daemon's default `docker0`, so a
`docker0`-scoped rule wouldn't even match this container's traffic).

CoAP itself has to stay world-open on both `5684/udp` and `5684/tcp` — devices roam across
carrier NAT, and DTLS/TLS-PSK is the access control, so there's no source-IP allowlist to layer
on top the way there is for SSH below. Explicit `RETURN`s make that intent visible, followed by
a backstop drop for anything else that reaches the WAN interface:

Docker ships `DOCKER-USER` pre-populated with a catch-all `RETURN`, so appending the backstop
lands it *after* that rule where it can never be reached — a silent no-op that looks correct in
`iptables -L`. Rebuild the chain in order instead of appending to it:

```sh
iptables -F DOCKER-USER
iptables -A DOCKER-USER -p udp --dport 5684 -j RETURN
iptables -A DOCKER-USER -p tcp --dport 5684 -j RETURN
iptables -A DOCKER-USER -i ens3 -m conntrack --ctstate NEW -j DROP
iptables -A DOCKER-USER -j RETURN
```

The trailing `RETURN` restores the default Docker expects, now *after* the backstop. Keep
`-i ens3` on the drop: unscoped, it also matches container-initiated NEW connections traversing
`FORWARD` and silently kills loft's own outbound calls to dovecote.

Verify with `iptables -L -v -n`, never bare `-L` — the latter omits interface matches, so a rule
missing its `-i` looks identical to one that has it.

The trailing `DROP` matters beyond the two CoAP ports: it's the backstop against a future
compose edit publishing something that was never meant to be reachable. `loft` is the first
workload on this host to use Docker's bridge networking and port publishing at all, so this
chain has no prior traffic on it — Kratos's container runs with `network_mode: host`, which
bypasses Docker's NAT and forward path entirely and binds its own sockets straight onto the
host.

That distinction decides which chain protects what, so don't generalize one to the other.
Kratos's admin API has no authentication by design (only trusted internal callers are supposed
to reach it) and binds `127.0.0.1:4434` directly. Because it never traverses `DOCKER-USER`,
that chain would not save you if its bind address were ever widened to `0.0.0.0` — the host
`INPUT` policy below is the only thing standing there, which is one more reason the default
`DROP` at the end of it is not optional. Conversely, anything published by a bridge-networked
container is governed by `DOCKER-USER` and is invisible to `INPUT`. Either mistake fails the
same quiet way: the service starts clean, works normally, and the exposure only surfaces in a
network scan.

Host baseline (`INPUT` chain) is the usual shape, nothing CoAP-specific:

```sh
iptables -A INPUT -i lo -j ACCEPT
iptables -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
iptables -A INPUT -p icmp --icmp-type destination-unreachable -j ACCEPT
iptables -A INPUT -p icmp --icmp-type echo-request -m limit --limit 1/s -j ACCEPT
iptables -A INPUT -p tcp --dport 22 -m conntrack --ctstate NEW -m recent --name ssh --set
iptables -A INPUT -p tcp --dport 22 -m conntrack --ctstate NEW \
  -m recent --name ssh --update --seconds 60 --hitcount 6 -j DROP
iptables -A INPUT -p tcp --dport 22 -j ACCEPT
iptables -P INPUT DROP
```

Destination-unreachable stays open for Path MTU discovery — dropping it turns a clean "needs
fragmentation" signal into connections that just hang. Echo-request stays open but rate-limited
(ping keeps working; it can't be turned into a reflection flood). SSH gets an `xt_recent`
brute-force throttle ahead of its accept rule.

Postgres needs no rule here at all: prod and staging both point Hyperdrive at managed Crunchy
Bridge (`dovecote/wrangler.toml` — only `[env.dev]`'s Hyperdrive binding uses a
`localConnectionString`, i.e. an actual local Postgres), so the access control that matters is
Crunchy Bridge's own allowlist — Hyperdrive's egress ranges plus this VPS's own address — not an
inbound rule on this box. Kratos is published outbound-only through a Cloudflare Tunnel, so it
needs no inbound 80/443 rule either. This VPS has no direct-inbound HTTP surface at all: just 22
and 5684.

#### IPv6

Leave `5684` closed on IPv6 for now. `loft` binds `0.0.0.0` by default on both listeners
(`LOFT_UDP_LISTEN`/`LOFT_TCP_LISTEN`, `loft/src/config.rs`) — there's no v6 listener behind the
port, so opening a v6 firewall hole ahead of one existing would just advertise a black hole. The
DNS step above already provisions an AAAA record alongside the A record; that only means a
client *can* route to this host over v6, not that anything here is listening for CoAP on it —
leave the v6 hole out until `loft` actually binds one.

SSH does listen on `[::]:22`, though, so it needs the same treatment as v4 — but as its own
rules, not shared ones: `xt_recent` keeps its hit lists keyed by `--name`, and that table is
shared across address families, so reusing the v4 rule's name for v6 would let an attacker's v4
attempts count against (or clear) the v6 rate limit and vice versa.

```sh
ip6tables -A INPUT -i lo -j ACCEPT
ip6tables -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
ip6tables -A INPUT -p ipv6-icmp -j ACCEPT
ip6tables -A INPUT -p tcp --dport 22 -m conntrack --ctstate NEW -m recent --name ssh6 --set
ip6tables -A INPUT -p tcp --dport 22 -m conntrack --ctstate NEW \
  -m recent --name ssh6 --update --seconds 60 --hitcount 6 -j DROP
ip6tables -A INPUT -p tcp --dport 22 -j ACCEPT
ip6tables -P INPUT DROP
```

The `recent`-module throttle above (both families) counts connections, not
authentication failures, so a burst of legitimate SSH sessions can trip it
same as a credential-guessing script. A fail2ban-based replacement is
prepared in [`ssh-hardening.md`](./ssh-hardening.md) — SSH hardening is
host-wide, not CoAP-specific, so it lives in its own doc rather than here;
the rules above are still what's actually live on the host until that
cutover runs.

Never blanket-drop `ipv6-icmp` the way v4 ICMP sometimes gets treated — on v6 it isn't just
diagnostics. Neighbor Discovery (address resolution) and Router Advertisements (the default
route under SLAAC) both ride on it, so filtering it doesn't just break pings; it produces a
delayed loss of connectivity as neighbor and route state expires, which looks exactly like a
random, unexplained lockout. Allow UDP 546 as well only if DHCPv6 is actually in use on this
host.

#### Operational notes

None of the above survives a reboot by itself — `iptables`/`ip6tables` rules are runtime state.
Install `iptables-persistent` and run `netfilter-persistent save` once the rules are correct, or
they're gone on the next reboot or kernel update.

Apply a `-P INPUT DROP` (or any policy change) over SSH carefully — a typo here turns into a
lockout whose only fix is the provider's console, i.e. a support ticket, not another `ssh`
attempt. Use `iptables-apply` (auto-rolls-back if the new rules aren't confirmed within a
timeout) or stage the change as a script with a delayed self-revert, rather than typing the
policy line directly into an interactive session.

conntrack is a separate resource budget from anything `loft` tracks itself. `loft`'s own
connection caps (4096 concurrent per listener, 256 per source /64 — see "Security posture"
above) bound what `loft` will admit, but every UDP DTLS session also occupies a kernel conntrack
entry independently of that cap. A large device fleet can exhaust `nf_conntrack_max` before it
comes close to `loft`'s own limits, and the failure is invisible from `loft`'s side — the kernel
drops the packet before `loft` ever sees it, so there's nothing in `loft`'s logs to explain the
loss. Check `nf_conntrack_max` before scaling the fleet, not after sessions start dropping.

Worth cleaning up while auditing this host, unrelated to CoAP specifically:
`systemd-resolved`'s LLMNR listener is on by default (`0.0.0.0:5355` and `[::]:5355`, both TCP
and UDP) and serves no purpose on a public VPS. Either disable it directly (`LLMNR=no` in
`/etc/systemd/resolved.conf`) or rely on the default-DROP `INPUT`/`ip6tables` policies above,
which already cover port 5355 on both families — disabling the listener is still the tidier fix,
since it means a future permissive rule change can't accidentally re-expose it.

Local dev loop: `docker-compose` stack + `wrangler dev --env dev` as usual, plus
`COAP_SERVICE_SECRET` in `dovecote/.dev.vars` (gitignored), then

```sh
COAP_SERVICE_SECRET=<same value> LOFT_DOVECOTE_URL=http://127.0.0.1:8787 \
  LOFT_UDP_LISTEN=127.0.0.1:5684 LOFT_TCP_LISTEN=127.0.0.1:5684 cargo run -p loft
```

`[env.dev]`'s `COAP_DEVICE_HOST = "127.0.0.1"` makes freshly minted dev pigeons point at it.

## Security posture (DTLS/UDP specifics)

- **Anti-amplification**: `DTLSv1_listen` driven statelessly on the listener thread, with a
  cookie that is an HMAC over the claimed source address keyed by a process-lifetime random
  key — the initial ClientHello gets a small HelloVerifyRequest and nothing else (no
  connection state, no PSK lookup, no dovecote call) until a client echoes a valid cookie
  from that source address. A spoofed source costs one small reply.
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
