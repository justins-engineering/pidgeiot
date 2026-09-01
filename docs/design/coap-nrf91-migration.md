# Moving the departure boards from HTTPS to CoAP on nRF9160

Scope: the UMass departure boards (Circuit Dojo nRF9160 Feather, `~/embedded-departure-board`)
move their pigeon connector from `Https` to `Coap`, with DTLS offloaded to the modem. Bandwidth on
metered LTE-M is the motive; the shadow poll is what costs. This document is a design and a work
breakdown, not an implementation.

Two owner rulings are taken as given: the move happens, and the FOTA download path is chosen on
**reliability**, not bandwidth.

Nothing here was verified against hardware. Every claim that needs a device to settle it is marked
and collected in [Bench checks](#bench-checks).

## Summary of recommendations

| # | Question | Recommendation |
|---|---|---|
| 1 | Modem capability | Fleet is on mfw 1.3.7. CID, PSK-DTLS and CCM_8 are all available. Proceed. |
| 2 | PSK provisioning | Keep it build-time via the existing `pigeon_psk.c` modem path. Add auth-failure backoff. |
| 3 | pigeon library | Small, bounded work. Raise `PIGEON_COAP_CONFIG_MAX`; it is a hard blocker as it stands. |
| 4 | FOTA | **Hybrid.** CoAP for shadow/telemetry/logs, HTTPS Range for firmware. |
| 5 | Connector migration | Add `PUT /pigeons/:id/connector`. Do not re-provision. Fix the credential-wipe bug first. |
| 6 | Bandwidth | ~15x less per poll cycle. ~86 MB/month to ~6 MB/month at the board's 300 s cadence. |
| 7 | Sequencing | After the 0.13.7 soak, behind the already-queued feather LTE-M v6 round trip. |

## 1. Modem capability

**The fleet is on `mfw_nrf9160_1.3.7`.** No console log anywhere records a modem version — the
board does not compile `CONFIG_MODEM_INFO` and never queries `+CGMR`. The evidence is the flash
record instead: `~/.nrfutil/logs/nrfutil-device.log` shows `mfw_nrf9160_1.3.7.zip` programmed and
verified against probe 1050038518 (the nRF5340-DK acting as J-Link for the Feather) on
2026-08-11, and that is the last modem-firmware operation on the machine. Eight earlier flashes
back to 2025-10 land on 1.3.7 every time except two deliberate downgrades. 1.3.5 was never even
downloaded.

This is bench-hardware provenance, not fleet observation. The one unit in the field went out from
this same lineage, so 1.3.7 is a well-founded inference — but the platform cannot *learn* a
device's modem version today. See bench check 8.

The board builds against NCS v3.4.0 LTS (`app/west.yml`, `nrf/VERSION` = 3.4.0, Zephyr 4.4.0),
well past the `nrf_modem` 2.4.0 that first exposed the CID socket options.

**What 1.3.7 gives us, from `nrfxlib/nrf_modem/include/nrf_socket.h` in our own tree:**

- Offloaded DTLS 1.2 over `socket(AF_INET|AF_INET6, SOCK_DGRAM, IPPROTO_DTLS_1_2)`. Zephyr's
  mbedTLS is bypassed entirely; credentials live in the modem store.
- `NRF_SO_SEC_DTLS_CID` (15) and `_CID_STATUS` (16), documented as
  `mfw_nrf9160 v1.3.5 or later`. mfw 1.3.5's release notes name RFC 9146 explicitly. Must be set
  before `connect()`; the extension rides in the ClientHello.
- `TLS_PSK_WITH_AES_128_CCM_8` (0xC0A8). **`TLS_PSK_WITH_AES_128_GCM_SHA256` is absent** from the
  modem's closed enumeration; the other PSK suites it offers are all CBC. CCM_8 is therefore the
  only AEAD PSK suite on this part, and it is also what `NRF_SO_SEC_DTLS_CONN_SAVE` requires.
  loft prefers CCM_8. The two sides already agree.
- `%CMNG` credential types 3 (PSK, hex-encoded) and 4 (PSK identity, ASCII). Writes are refused
  while `+CFUN` is 1, 2 or 21 — both `CFUN=0` and `CFUN=4` satisfy the constraint.

**What it does not give us, and this matters:** several `nrf_socket.h` options are nRF91x1-only
despite sitting in the same header. `NRF_SO_SEC_HANDSHAKE_STATUS` (19) is one — so there is **no
on-device way to distinguish a full handshake from a resumed one** on a 9160. Any acceptance
criterion about session reuse has to be read off loft's journal or a modem trace, never from
device logs. `NRF_SO_KEEPOPEN` (34) and `NRF_SO_SEC_DTLS_FRAG_EXT` (20) are likewise absent.

**Sleep and rebind.** Nordic's own framing of what CID buys is exactly our case: it "removes the
DTLS bind to the IP address, and the device does not need to maintain the connection or IP
address," saving "kilobytes of data each time a device connects." A PSM or eDRX sleep never closes
the socket, so the modem's DTLS context is preserved by construction. `CFUN=0` always closes every
socket regardless. Survival across `CFUN=4` and across a reboot is undocumented for the 9160 —
1.3.5's "DTLS context serialization" line is often glossed as reboot persistence, and Nordic's own
`CONN_SAVE` wording ("saving the session frees up memory in the modem"; "if the socket is closed,
the saved DTLS data is cleaned") reads as RAM-resident. Treat reboot survival as unproven.

CID also does not restore *server-initiated* reachability after a NAT mapping dies. Irrelevant
here: there is no CoAP push channel and the device always initiates.

**The 1 kB datagram ceiling is the constraint that shapes everything downstream.** mfw 1.3.7's
limitations block states "Secure socket buffer size is 2kB" *and* "Maximum length of DTLS datagram
is 1kB". The commonly-quoted 2 KB figure is the TLS number; DTLS is half that, and
`NRF_SO_SEC_DTLS_FRAG_EXT` — the option that would relax it — is nRF91x1-only. Section 5 covers
what this collides with in loft.

## 2. PSK provisioning

**The mechanism already exists and is correct.** `pigeon_psk.c` writes both halves into the modem
store on `CONFIG_MODEM_KEY_MGMT` builds: `MODEM_KEY_MGMT_CRED_TYPE_IDENTITY` raw, and
`MODEM_KEY_MGMT_CRED_TYPE_PSK` hex-encoded, with a SHA-256 digest compare before writing because
the modem refuses to read a PSK back. `psk_hex` is zeroized on every exit path.

The hex encoding deserves a note, because it looks like a double-encode bug and is not one. loft's
PSK bytes are the raw UTF-8 bytes of the 32-character hex secret string
(`loft/src/tls_common.rs:107`). `%CMNG` type 3 takes a hex string that the modem decodes to raw
bytes. pigeon hex-encodes those 32 ASCII characters into 64 hex digits, the modem decodes them back
to the same 32 bytes, and both ends key on an identical secret. **Do not "fix" this.**

**Ordering is the real hazard.** The modem store only accepts writes while the modem is offline, so
`pigeon_core.c` registers the PSK eagerly from `pigeon_init()` rather than lazily from the
transport's first connect. Nothing enforces the ordering in code — it is doc comments in six
places, backed by the indirect consequence that a late `pigeon_init()` gets a write failure. That
consequence has a hole: the digest compare short-circuits before any write, so on a warm restart
with matching credentials a late `pigeon_init()` succeeds silently. The ordering violation is
invisible on exactly the boots where it would otherwise be caught. The departure board must call
`pigeon_init()` before `lte_manager` brings the link up, and that ordering wants a test, not a
comment.

**Rotation is the operational hazard, and it is worse on CoAP than on HTTPS.** There is no runtime
re-provisioning path: `pigeon_coap_psk_registered` is set once and never cleared, no public API
accepts new credentials, and `struct pigeon_coap_config` is copied once in `pigeon_init()`. A
platform-side `token/refresh` re-mints the PSK *and* the endpoint *and* the bearer token together,
so a rotation strands the device until it is reflashed — same as an HTTPS token rotation, except
that CoAP has **no auth-failure backoff**. MQTT has `CONFIG_PIGEON_MQTT_AUTH_BACKOFF_SEC` (900 s)
for precisely this shape; the CoAP transport retries a doomed handshake against every resolved
address at the app's poll cadence, forever.

**Recommendation.** Keep provisioning build-time. It matches how this fleet already works — the
board bakes `CONFIG_PIGEON_TOKEN` and writes its CA bundle to the modem store at boot
(`lte_manager.c:147`), so the machinery and the offline-write discipline are both already in
production here. Building a runtime rotation path is a larger design (it needs an authenticated
channel that survives the credential being rotated) and is not what this migration is for.

Do add the backoff. Classify a handshake failure that names the credentials or the identity and
hold off, mirroring the MQTT connector. That turns a provisioning mistake from a battery-draining
hammer into a recoverable one.

## 3. pigeon library gaps

The good news first: **every socket option the CoAP UDP transport uses maps cleanly through NCS's
offload shim.** Checked against the vendored `nrf/lib/nrf_modem_lib/nrf9x_sockets.c` in our own
tree — `TLS_SEC_TAG_LIST`, `TLS_HOSTNAME`, `TLS_DTLS_CID`, `TLS_DTLS_CID_STATUS` and
`TLS_CIPHERSUITE_USED` all have entries in `z_to_nrf_optname`, and the shim claims
`IPPROTO_DTLS_1_0..1_2` sockets. Nothing in the DTLS path is native-mbedTLS-only: pigeon never sets
a ciphersuite list, a session cache option or a handshake timeout. The mbedTLS-specific
configuration lives in the ESP32 sample's board file, not in the library.

Better still, **pigeon's existing CID setting is already the right one for this pairing.** It sets
`TLS_DTLS_CID_SUPPORTED`, i.e. a zero-length own CID: "when you send to me, use no CID; I will
carry yours on the uplink." That is precisely the shape loft's entire test matrix and rebind
harness exercise, and it is sufficient for the rebind case — loft routes an incoming record by
parsing *its own* CID out of the uplink at a fixed offset, then migrates the address route after an
authenticated read. `TLS_DTLS_CID_ENABLED` (a non-empty own CID) would ask loft to address downlink
records to us, which buys nothing and puts us on a path loft has never seen.

### Blockers

**`PIGEON_COAP_CONFIG_MAX` is 256 bytes.** The board runs `CONFIG_PIGEON_SHADOW_CONFIG_MAX=768`.
The CoAP path caps shadow config strings at a hardcoded 256 in `pigeon_coap_internal.h`,
independent of and smaller than the Kconfig symbol that advertises the limit. A `target_config`
carrying a `firmware` object (a 64-hex sha256 alone is 64 bytes) plus `telemetry_interval`,
`update_stop_interval`, `log` and `firmware_repush` lands close to 256 and will exceed it on some
pushes. This silently shrinks the board's shadow ceiling by 3x and would fail a FOTA push. Raise it
to track `CONFIG_PIGEON_SHADOW_CONFIG_MAX`, and check the result against the frame budget below.

**`CONFIG_NET_UDP=n`** in both `app/prj.conf` and `app/prj_release.conf`. Neither those files nor
the board file carry any `COAP` or DTLS symbol. This is not a one-line connector swap.

**The UDP transport is not the build default.** `PIGEON_COAP_TRANSPORT_TCP` is the choice default,
so a build that sets only `CONFIG_PIGEON_CONNECTOR_COAP=y` compiles the RFC 8323 TCP transport and
then fails its own endpoint scheme check against the `coaps://` the platform mints. The failure is
loud but non-fatal: `LOG_ERR` plus `-EINVAL` on every exchange for the life of the firmware, since
the input is a compile-time constant. Set `CONFIG_PIGEON_COAP_TRANSPORT_UDP=y` explicitly. This
check would be better as a `BUILD_ASSERT`.

**`TLS_PEER_VERIFY` is never set anywhere in pigeon.** Nordic's note on that option says "there
must be at least one root CA in the modem credential storage, regardless if the value is NONE or
OPTIONAL," and the modem default is OPTIONAL — so we land inside the note's scope. Whether it
applies to a PSK-only sec tag with no certificate is not addressed by the documentation. The board
already holds a CA bundle at sec tag 2, but the PSK would live at `CONFIG_PIGEON_COAP_SEC_TAG`
(default 1), and whether "credential storage" means per-tag or global is undocumented. This is
bench check 1, the highest-risk unknown in the whole migration. If it bites, the fix is one line:
set `TLS_PEER_VERIFY_NONE` on PSK builds.

### Frame budget

`PIGEON_COAP_MSG_MAX` is `MAX(640, PIGEON_TELEMETRY_BODY_MAX + 384)`, which at the board's
`CONFIG_PIGEON_TELEMETRY_MAX_KEYS=8` is **1707 bytes**. That exceeds the modem's 1 kB DTLS
datagram ceiling. It is a buffer size rather than a wire size, so it is not itself a defect — the
board reports four telemetry keys, well inside 1 kB — but nothing in the build asserts the
relationship. Two request buffers of that size also sit on the caller's stack. Worth a
`BUILD_ASSERT` tying the frame ceiling to a configurable link MTU on offloaded builds.

### What CoAP does not get

`CONFIG_PIGEON_WS`, `CONFIG_PIGEON_TELEMETRY_BATCH` and `CONFIG_PIGEON_FOTA` all
`depends on PIGEON_CONNECTOR_HTTPS` (FOTA also allows MQTT). So a CoAP build has:

- **No push channel.** The device polls. Fine — section 6 shows a CoAP poll is cheap enough that
  push is not worth building.
- **No batched telemetry.** The batch shape over CoAP was deliberately deferred because loft frames
  requests differently. The board's recommendation was batching off at its cadence anyway, and at a
  300 s poll batching buys little.
- **No FOTA.** Section 4.

`CONFIG_PIGEON_LOG_UPLOAD` does work on CoAP, implemented as a hand-driven Block1 sequence.

**There is no runtime HTTPS fallback and should not be one.** The connector is a mutually-exclusive
build-time choice. A device that cannot reach loft cannot decide to speak HTTPS instead. The honest
recovery channel is a firmware push — which is exactly the argument section 4 turns on.

## 4. FOTA: keep the HTTPS Range download

**Recommendation: hybrid.** CoAP carries shadow, telemetry and logs. Firmware stays on the
field-proven HTTPS Range download.

This is the reliability answer, not the bandwidth answer. On bandwidth alone CoAP Block2 wins
easily (section 6). The ruling says reliability decides, and on reliability the two options are not
close.

### What CoAP block-wise would have to earn

loft's Block2 firmware path is genuinely well built. It is **stateless** — every block derives from
the request's own Block2 option and maps directly onto an HTTP `Range` against dovecote, with no
server-side transfer table. So resume after a re-handshake, a rebind or a reboot is free, and
re-requesting block N after any gap just works. It carries an ETag (first 8 bytes of the image
sha256) on *every* block, not only block 0, deliberately. It honors a client-proposed smaller SZX.

Against that:

- **pigeon has no Block2 client at all.** Its only block-wise code is a hand-driven Block1 sequence
  for log upload. Response-side reassembly, ETag tracking, `Size2` handling and the integration
  with `pigeon_fota_resume.c` are all net-new, hand-rolled against a hand-rolled CoAP client.
- **The modem's 1 kB datagram ceiling collides with loft's default block size.** A firmware GET
  carrying *no* Block2 option gets block 0 at szx 6 — a 1024-byte payload, which with CoAP options
  and DTLS record overhead exceeds 1 kB on the wire. The device must send an explicit Block2 with
  szx ≤ 5 on its very first request or first contact fails. That is a subtle trap in the one
  exchange that has no prior state to recover from.
- **Zero field hours.** loft's Block2 firmware path has never been driven by a real device; the C6
  FOTA verification ran over HTTPS.
- Per-block upstream cost is unchanged (each block is one ranged GET, one DO round trip, one R2
  read, against `DEVICE_FIRMWARE_LIMITER`), so at szx 5 the request count matches today's 512-byte
  chunks exactly. No saving there.

### What the HTTPS path already has

A 485 KB image completed over live LTE-M in 45m33s. Behind that number sit
`CONFIG_PIGEON_FOTA_RESUME` (persisting `{version, flushed_offset}`, never a RAM-tail offset,
rehashing the flashed slot on resume), `CONFIG_PIGEON_FOTA_CHUNK_RETRIES`, a separate 429 budget
honoring `Retry-After`, the per-`target_version` attempt budget with its re-push recovery story,
full-image sha256 verify, and the progressive-erase bug found and fixed on real hardware. That is
years of accumulated hardening on the exact path this fleet uses.

Wiring it into a CoAP build needs **no new mechanism** — the seam exists. `CMakeLists.txt` already
compiles `pigeon_https.c` for an MQTT build with its connector hooks `#ifdef`'d out, fed by
`CONFIG_PIGEON_FOTA_HTTPS_ENDPOINT` and its own sec tag. Extending `depends on` to
`|| PIGEON_CONNECTOR_COAP` follows a path already trodden.

The board also keeps TLS/TCP and native mbedTLS compiled in regardless, for the Swiftly departures
API (`custom_http_client.c` opens `SOCK_STREAM | SOCK_NATIVE_TLS`, deliberately native because the
modem's offloaded TLS cannot take Swiftly's large records). Keeping an HTTPS FOTA path costs
nothing this build was not already paying.

### The argument that decides it

**A CoAP build's only remote recovery channel is FOTA.** If firmware download also moved to CoAP,
any CoAP-side regression that stops the device talking — a PSK rotation, a handshake failure mode,
a loft change, a block-wise bug — would be unrecoverable without physically visiting the sign.
Keeping the download on an independent, field-proven transport means a bad CoAP rollout is a push
away from being fixed. That is the reliability property worth paying bandwidth for, and firmware
pushes are a few per year against 8,640 polls per month.

### What would flip it

A real nRF9160 driving loft's Block2 firmware path to completion repeatedly over live LTE-M — call
it twenty consecutive 485 KB downloads including induced mid-transfer rebinds and reboots — at a
lower failure rate than the HTTPS path's measured rate, *and* confirmation that the modem's actual
max DTLS payload comfortably holds szx 5. Absent both, the hybrid stands.

### Cheaper win on the same path

`pigeon_https.c` opens a socket, issues one request, and closes it — every call, FOTA chunks
included. A 485 KB image at 512-byte chunks is 970 chunks and therefore **970 full TCP+TLS
handshakes**, roughly 5 MB of transport for a 485 KB image. Reusing one socket across chunks would
cut that to about 1.3 MB and take most of the 45 minutes off, in one file, without touching the
transport the reliability case rests on. This is a better FOTA bandwidth investment than moving to
CoAP, and it is independent of this migration.

## 5. Platform and loft gaps

### Ready

- **CID.** loft's mbedTLS listener negotiates CID per-peer, never requires it, and its whole test
  matrix uses the zero-length client CID the nRF9160 offers. A non-CID client is still served, just
  address-routed.
- **Ciphersuites.** loft pins `PSK-AES128-CCM8:PSK-AES128-GCM-SHA256:PSK-AES128-CBC-SHA256` on both
  stacks, with `set_security_level(0)` so CCM_8 is actually selected rather than merely listed. The
  modem offers CCM_8 and CBC-SHA256 from that list; mbedTLS selects in server preference order, so
  the negotiated suite will be CCM_8. That is also the only AEAD suite on the part.
- **IPv6.** Dual-stack listener live since 2026-08-26, AAAA published, `COAP_SERVICE_ALLOWED_IPS`
  carries the VPS v6 egress. LTE-M carriers are IPv6-first, so this was a prerequisite and it is
  done.
- **Route coverage.** All five device routes map 1:1. The Uri-Path pigeon id is checked against the
  handshake identity, 4.03 on mismatch, without an upstream call.

### Gaps

**The 1 kB datagram ceiling versus loft's 1024-byte block size.** loft sends a body whole up to
1024 bytes and spontaneously Block2s above it, at szx 6. A 1024-byte CoAP payload plus options plus
a DTLS record header plus the CCM_8 tag exceeds 1 kB on the wire. Any shadow response in roughly
the 950-1024 byte band is emitted whole by loft and cannot be received by the modem. Today's
shadows are 200-400 bytes so this is latent, not live — but it is a cliff with no warning on either
side, and raising `PIGEON_COAP_CONFIG_MAX` (section 3) walks toward it. Either the device always
sends an explicit Block2 with szx ≤ 5, or loft's threshold becomes configurable. loft has no RFC
6066 max_fragment_length support and a hardcoded 1400-byte `DTLS_MTU`, so the device's only lever
is the SZX it asks for.

**The 300 s non-CID idle equals the board's poll interval exactly.** loft drops a non-CID DTLS
session after 300 s (not configurable); a CID session lives 6 h (`LOFT_DTLS_CID_IDLE_SECS`). The
board polls every 300 s. If CID fails to negotiate for any reason, every cycle sits precisely on
the deadline — a coin flip between reusing the session and paying a fresh handshake, which is
exactly the kind of intermittent nobody diagnoses quickly. CID is therefore load-bearing here, not
an optimization, and its negotiation needs a positive check at rollout (loft-side, since the
9160 has no `HANDSHAKE_STATUS`).

**30 s handshake deadline, not tunable.** Both stacks hardcode it, and production never calls
`set_handshake_timeout`, so mbedTLS's default RTO schedule applies (1, 3, 7, 15, 31 s) — about four
flight retransmissions inside the deadline. A mandatory HelloVerifyRequest cookie exchange costs
two extra round trips before the PSK flights even start. On LTE-M in poor RF after a PSM wake,
multi-second RTTs could plausibly exhaust 30 s. Worth making configurable before a fleet depends on
it.

**The cookie is bound to `ip:port`.** If a carrier NAT rebinds the source port between the
HelloVerifyRequest and the cookied ClientHello, the check fails and the client loops on HVR. Low
probability, unbounded consequence.

**No session resumption on the mbedTLS listener.** nRF91 apps commonly lean on the modem's
`TLS_SESSION_CACHE` as an alternative to CID; against this listener that always falls back to a
full handshake. Not a problem while CID works, but it removes the obvious fallback.

**Verify prod's `LOFT_DTLS_STACK`.** The in-repo unit still ships `openssl`, which has no RFC 9146
at all; production carries a `dtls-stack.conf` drop-in selecting mbedtls. That drop-in is the only
thing standing between this migration and no CID.

### Connector migration: route, not re-provision

**Recommendation: add `PUT /pigeons/:pigeon_id/connector`.** Do not re-provision.

Re-provisioning mints a new pigeon id, which is the Durable Object id, which is the PSK identity,
which is the URL path. Everything keyed on it goes: the shadow and its version counters, telemetry
history, the latest-value blob, ACL rows, the log ring, firmware catalog association, and the
dashboard's graph continuity. For a handful of signs that is survivable but wasteful, and it throws
away exactly the history the boards exist to demonstrate.

A route is cheap because `refresh_token` is already nine-tenths of it: it preserves the connector
*variant*, discards the old inner config, and re-mints endpoint + PSK + token together from current
env vars. Change the variant before that `match` and correct credentials fall out for free. The
work is factoring that `match` into a `mint_connector(env, do_id, kind, token)` helper shared by
`create`, `refresh_token` and the new handler, gating on `is_owner`, closing any open device WS
(the keypair rotates), and syncing through the existing `update_pigeon_pg_db`, which already writes
`connector` wholesale. Nothing device-side changes: the connector is a provisioning hint, and any
pigeon that has minted a PSK pair can complete a handshake with either terminator.

**A prerequisite, and it is independently urgent.** `PUT /pigeons/:pigeon_id` currently writes the
connector column verbatim from the request body, with no re-minting and no validation, gated only
at Member level — and fancier's `UpdatePigeonModal` sends a default-constructed connector on
*every* pigeon save. So renaming or retagging a CoAP or MQTT pigeon today writes
`{"endpoint":"","token":"","tls_psk_identity":null,"tls_psk_secret":null}`, which makes the
terminator's PSK lookup 404 and cuts the device off until an owner runs `token/refresh`. HTTPS
pigeons survive by luck, because device auth verifies against the separate `device_public_key`
column that `update` does not touch. Two smaller findings sit alongside it: `read_back_pigeon` does
not call `strip_secrets`, so `PUT /pigeons/:id` returns the stored token and PSK secret to a
Member, contradicting `docs/api.md`; and a Member can therefore choose a pigeon's PSK.

This is a live defect on a shipping connector, not something this migration introduces. It should
be fixed on its own merits and before any connector-change route lands, since that route's whole
value is minting credentials the other route would then silently destroy.

Neither the DO's SQLite schema nor the Postgres mirror has an immutability trigger on `connector` —
the triggers guard `id` and `created_at` only. So the column is writable by design; the fix is in
the handler, not the schema.

One operational note: loft caches PSK positives for 60 s, so after a connector change the old
credentials can still complete a *handshake* for up to a minute. Every request on such a session
presents a revoked bearer token and 401s at the DO, so the failure is loud, but it means "the
change took effect" is not observable for 60 s.

## 6. Numbers

Order of magnitude, not precision. The ratio is robust; the absolutes carry maybe ±40%, mostly in
the TLS handshake estimate. All figures are IP-layer bytes.

**The handshake is the entire story.** `pigeon_https.c` opens a socket, issues one request, and
closes it — every call. A poll cycle is a shadow GET and a telemetry POST, so it is two full
TCP+TLS handshakes, and the modem's session cache is not enabled.

| | HTTPS / TLS 1.2 over TCP | CoAP / DTLS+CID over UDP |
|---|---|---|
| TCP handshake | ~180 B, 3 pkts | — |
| TLS handshake (ECDSA chain, no resumption) | ~3,500 B, ~10 pkts | — (amortized) |
| Shadow GET request | ~380 B | ~166 B |
| Shadow GET response (132 B body) | ~950 B | ~196 B |
| Teardown | ~250 B, 4 pkts | — |
| **One shadow exchange** | **~5.3 KB, ~26 pkts** | **~0.36 KB, 2 pkts** |
| **Poll cycle (GET + telemetry POST)** | **~10 KB, ~50 pkts** | **~0.7 KB, 4 pkts** |

The CoAP figures assume an established CID session. A full PSK DTLS handshake with the mandatory
HelloVerifyRequest is roughly 950 B over 6 packets — so even re-handshaking on *every* cycle is
~1.7 KB, still 6x cheaper than one HTTPS cycle. With loft's 6 h CID idle window the handshake
amortizes to about four per day.

The response headers are a real cost on the HTTPS side and are easy to forget: Cloudflare adds
`CF-Ray`, `Report-To`, `NEL`, `alt-svc` and friends to a 132-byte body, and dovecote adds its CORS
headers on top. CoAP's equivalent is a 4-byte header and a Content-Format option.

**Monthly device-side data, 30 days:**

| Cadence | HTTPS | CoAP + CID | Delta |
|---|---|---|---|
| 300 s (the board's actual pigeon poll) | ~86 MB | ~6 MB | ~15x |
| 30 s (the display-refresh cadence, for scale) | ~860 MB | ~60 MB | ~15x |

The board polls pigeon at 300 s (`CONFIG_PIGEON_CLIENT_POLL_INTERVAL_SECONDS`, overridable by the
shadow's `telemetry_interval` with no clamp). Its 30 s cadence is the Swiftly fetch and display
refresh, on a separate native-TLS socket that this migration does not touch — listed above only to
show what the pigeon poll would cost if it ever moved there. The 15x holds either way because it is
a property of removing the handshake, not of the cadence.

For completeness, the FOTA figures from section 4: today ~5 MB of transport for a 485 KB image
(970 chunks, 970 handshakes); with socket reuse ~1.3 MB; over CoAP Block2 at szx 5, roughly 740 KB.
Bandwidth would pick CoAP. Reliability picks HTTPS, and a few campaigns a year against 8,640 polls
a month says the poll is where the money is.

## 7. Sequencing and work breakdown

Sizes are relative: small ≈ under a day, medium ≈ a few days, large ≈ a week or more with hardware
time.

**Gate:** after the 0.13.7 soak, and behind the already-queued post-soak feather LTE-M v6 round
trip plus CID rebind over v6 (task #26's remaining item). That bench session answers whether this
fleet's hardware can hold a CID session over the real carrier path at all, which is the premise
everything below rests on. Do not start the device work before it.

### Phase 0 — prerequisites, independent of the migration

| # | Repo | Task | Size |
|---|---|---|---|
| 0.1 | pidgeiot | Stop `PUT /pigeons/:id` writing the connector; drop the connector picker from `UpdatePigeonModal`; add `strip_secrets` to `read_back_pigeon`. Live defect. | small |
| 0.2 | loft | Confirm production's `LOFT_DTLS_STACK` drop-in selects mbedtls. Without it there is no CID. | small |
| 0.3 | pidgeiot | Feather LTE-M round trip over IPv6 + CID rebind (task #26 remainder). Bench. | medium |

### Phase 1 — platform

| # | Repo | Task | Size | Depends |
|---|---|---|---|---|
| 1.1 | pidgeiot | `mint_connector()` helper factored out of `refresh_token`'s match; shared by `create`, `refresh_token`, new handler. | small | 0.1 |
| 1.2 | pidgeiot | `PUT /pigeons/:pigeon_id/connector`, `is_owner`, closes device WS, syncs to PG. Plus `docs/api.md` heading, `**Auth:**` line and glance-table row (test-enforced). | medium | 1.1 |
| 1.3 | pidgeiot | fancier: connector-change action in `ConnectorInfo` beside the refresh button, routed through `TokenReveal` + `helpers/device_credentials.rs` so the new PSK is readable once. | small | 1.2 |
| 1.4 | loft | Make the spontaneous-Block2 threshold and `HANDSHAKE_DEADLINE` configurable; both are currently compile-time constants that a cellular client can be hurt by. | small | — |

### Phase 2 — device library

| # | Repo | Task | Size | Depends |
|---|---|---|---|---|
| 2.1 | pigeon | Raise `PIGEON_COAP_CONFIG_MAX` to track `CONFIG_PIGEON_SHADOW_CONFIG_MAX`; `BUILD_ASSERT` the frame ceiling against a link MTU on offloaded builds. **Hard blocker.** | small | — |
| 2.2 | pigeon | Set `TLS_PEER_VERIFY` explicitly on PSK builds if bench check 1 says a PSK-only sec tag needs a CA present. | small | BC1 |
| 2.3 | pigeon | Auth-failure classification + backoff on the CoAP transport, mirroring `CONFIG_PIGEON_MQTT_AUTH_BACKOFF_SEC`. | small | — |
| 2.4 | pigeon | Extend `CONFIG_PIGEON_FOTA`'s `depends on` to include `PIGEON_CONNECTOR_COAP`; reuse the MQTT seam (`CONFIG_PIGEON_FOTA_HTTPS_ENDPOINT`, separate sec tag, `BUILD_ASSERT` a non-empty token). | medium | — |
| 2.5 | pigeon | Turn the endpoint scheme check into a `BUILD_ASSERT` instead of a per-exchange runtime `LOG_ERR`. | small | — |
| 2.6 | pigeon | Optional, independent: reuse one socket across FOTA chunks in `pigeon_https.c`. Bigger FOTA win than anything else here. | medium | — |

### Phase 3 — departure board

| # | Repo | Task | Size | Depends |
|---|---|---|---|---|
| 3.1 | EDB | Bump the pinned pigeon SHA (currently 35 commits behind) and re-verify the existing HTTPS build. Do this on its own, before any connector change. | medium | 2.x |
| 3.2 | EDB | Lift `CONFIG_NET_UDP=n`; add `CONFIG_PIGEON_CONNECTOR_COAP=y`, `CONFIG_PIGEON_COAP_TRANSPORT_UDP=y`, `CONFIG_PIGEON_COAP_DTLS_CID=y`, a distinct `CONFIG_PIGEON_COAP_SEC_TAG`, and the FOTA HTTPS endpoint/token pair. | medium | 3.1 |
| 3.3 | EDB | Guarantee `pigeon_init()` runs before `lte_manager` brings LTE up, with a test rather than a comment. The modem store only accepts writes offline and the digest short-circuit hides the violation on warm boots. | small | 3.2 |
| 3.4 | EDB | Re-check the `pigeon_transport_lock` borrow across `custom_http_client.c` — the modem allows several concurrent TLS sessions but one handshake in flight, and a DTLS handshake now joins that queue. | medium | 3.2 |
| 3.5 | EDB | Add `CONFIG_MODEM_INFO` and report `+CGMR` as a telemetry key, so the platform can gate a CID rollout on observed modem firmware instead of flash-log inference. | small | — |

### Phase 4 — bench validation

Sequential, one board, after the soak frees the feather.

1. Provision a staging pigeon, change its connector to `Coap` through the new route, and read the
   minted PSK once from the dashboard.
2. Cold boot with the modem offline: confirm both `%CMNG` slots are written and that a warm reboot
   short-circuits on the digest compare.
3. First handshake against `coap.pidgeiot.com`. Read the negotiated suite from
   `TLS_CIPHERSUITE_USED` and confirm CCM_8. Confirm CID negotiated **from loft's journal**, not
   from the device — `HANDSHAKE_STATUS` does not exist on this part.
4. Shadow GET, telemetry POST, shadow report, log upload. Compare bytes on the wire against
   section 6.
5. PSM sleep past the carrier's NAT timeout, wake, confirm the session survives with no
   re-handshake. Then past 6 h to confirm the CID idle boundary behaves.
6. Induced rebind (the existing CID harness topology, but over the real carrier path).
7. A shadow push at the raised config ceiling, to walk right up to the 1 kB datagram limit and
   confirm where it actually breaks.
8. A full FOTA campaign over the retained HTTPS path from a CoAP build, including a mid-download
   abort and resume.
9. A deliberate PSK rotation, to confirm the new backoff engages instead of hammering.

## Bench checks

Ordered by risk. Items 1-3 gate the design; the rest gate the rollout.

1. **Does a PSK-only sec tag still need a root CA in the modem credential store?** Nordic's
   `PEER_VERIFY` note says one is required "regardless" of the verify setting, and does not address
   the no-certificate case. The modem default is OPTIONAL, so we are inside the note's scope. If it
   applies, either provision a dummy CA at the PSK sec tag or set `TLS_PEER_VERIFY_NONE`.
2. **Actual maximum DTLS payload with CID enabled.** Documented as a 1 kB datagram; CID adds bytes
   to every uplink record. This sets the usable SZX and the shadow config ceiling.
3. **`%CMNG` accepted lengths and quoting convention for types 3 and 4.** No maximum PSK secret or
   identity length is documented anywhere. Our identity is a 64-character hex string. Published
   examples are unquoted while certificate examples are quoted — exactly the difference that
   silently writes a wrong-length key.
4. **Does the DTLS context survive `CFUN=4` and a reboot on a 9160?** `NRF_SO_KEEPOPEN`, the
   documented preservation mechanism, is nRF91x1-only. Treat 1.3.5's "context serialization" as
   unproven for reboot survival.
5. **Session cache scope, limits and whether it helps a PSK handshake at all.** Undocumented, and
   the on-device instrument for measuring it does not exist on this part — observe at loft.
6. **PSK-DTLS handshake cost** for our configuration, by modem trace. Published figures measure
   certificate handshakes and will overstate a PSK one substantially.
7. **`setsockopt(TLS_DTLS_CID)` after `connect()`.** Undocumented. Do not use its return value as a
   feature probe.
8. **Field-unit modem firmware.** Nothing observed. Phase 3.5 turns this from inference into fleet
   data and is a prerequisite for gating a CID rollout on `mfw >= 1.3.5` across more than one board.
