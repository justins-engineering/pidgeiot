# Adding a NIDD connector through Verizon ThingSpace

Scope: a fourth `Connector` variant, `Nidd`, whose device side is the carrier's SCEF reached through
Verizon ThingSpace. Uplink arrives at dovecote as a ThingSpace NiddService callback; downlink leaves
through the ThingSpace API. The document covers capsules, dovecote, fancier, `docs/api.md`, the
owner's `thingspace-sdk` crate, and what the `~/pigeon` transport has to mirror. It is a design and
a work breakdown, not an implementation. Repo state: pidgeiot `main` at `fcc093c` (this worktree,
branch `nidd-connector`), `thingspace-sdk-rust` at `9d920a4`.

Rulings taken as given: the owner's 2026-09-24 ask ("add a NIDD connector starting with Verizon
ThingSpace"), and the 2026-09-18 position that the platform stays vendor-agnostic, "IP transports
over LTE-M first; NIDD as an optional carrier-specific path, never a dependency"
(memory `project_product_strategy.md:117`).

Tier 1, the staging synthetic suite and tier 2, the bench nRF9160 Feather against staging, ran on
2026-09-24. Section 13.2 records the bench's results, and the sections they changed (7, 8.4, 8.5,
12, 14) say what the bench measured in place of what the pages promised; every claim neither run
settled is a check in [section 13](#13-tests-and-the-staging-verification-plan) or sits in
[section 18](#18-unverified-and-sources). External pages were read on 2026-09-24; internal anchors
are `file:line` at `fcc093c`.

How this was reached: three competing designs (smallest footprint, failure-driven, device-first) and
two reviews, one for conventions and running cost, one for Verizon semantics and security, then
three skeptic passes (security, Verizon semantics and operations, conventions) whose must-fix
findings are folded in. This document keeps the smallest design's scope, the device-first design's
routing, claim key and radio contract, and the failure-driven design's acknowledgement and
de-duplication rules. Where the reviews disagreed, [section 16](#16-decisions-for-the-owner) names
the alternative.

## 1. Summary of recommendations

| # | Question | Recommendation |
|---|---|---|
| 1 | What binds a device to a pigeon | The modem's IMEI. A Nidd pigeon's Durable Object id is `id_from_name("nidd:imei:<imei>")`, so a callback reaches its DO with no index, no Postgres read and no cache in the way. |
| 2 | Who may claim the device | A 16-byte claim key minted at create, built into the firmware, sent once per boot in a `HELLO` frame, which also pins the line's ICCID. Until claimed, the DO stores nothing from the device and sends it nothing but a rate-limited notice. The same key signs every platform frame with a truncated HMAC, so only dovecote can steer the device. |
| 3 | How a callback is trusted | Source address among Verizon's eight published addresses (fail closed), then the body's `password` in constant time, then `accountName` equal to this environment's account. 403, never 401. Against a direct forger only the password is secret (4.2); forged uplink is decision D12. |
| 4 | Routes | One new route, `POST /internal/thingspace/nidd`. No listener-management or send route: registering the listener is an owner runbook, and every send starts inside a Durable Object. |
| 5 | Envelope | One type byte, and no NUL byte anywhere, since ThingSpace refuses a downlink holding two in a row. Uplink bodies are exactly today's HTTPS device bodies; `HELLO` carries the claim key as hex; the downlink shadow is a text header of its two versions (at most 22 bytes), raw `target_config` and a 16-character hex HMAC tag; replies are 21 to 30 bytes. |
| 6 | Size budget | 1358 bytes a frame in both directions at the platform; the device caps an uplink frame at the 1273 bytes the bench modem accepts, and batches. A Nidd pigeon's `target_config` always fits at 1319 bytes (up to 1337 while its versions are short), refused with 413 at the dashboard write. |
| 7 | When to acknowledge | After the durable handoff (the DO's write plus the queue enqueue), never before. Billing, the Postgres sync and every downlink run after the response. |
| 8 | Duplicates | De-duplicated in the pigeon's DO on `requestId` plus a frame digest, in the same row as the claim. A ThingSpace resend is never stored or billed twice. Whether identical frames from two wakes stay distinct depends on Verizon's request ids, a Phase 2 gate (B5). |
| 9 | Downlink | A shadow write that raises `target_version` pushes one frame, at most one unsolicited push per 15 minutes; `HELLO` and shadow reports get replies, and so does any uplink from a device still owed a shadow, since only a frame sent while the device holds the connection its uplink opened reliably arrives (D13). Every downlink has a 30 s delivery window. No downlink at all in the steady state. |
| 10 | Free-tier fuse | Checked at the gateway, as the HTTP telemetry route does, so the DO opens no Postgres connection on the uplink path; raced against a one-second timer, failing open. A paused device gets one `PAUSED` notice an hour. |
| 11 | ThingSpace tokens | One `ThingSpaceSession` Durable Object per environment: tokens in its storage, logins single-flight behind a mutex, and a latch that stops after two failed logins per credential set and login epoch (Verizon locks the account after five). |
| 12 | Postgres | No schema change and no migration. |
| 13 | SDK | Depend on the owner's crate for the three outbound calls after a short patch; parse the inbound callback in dovecote, where it is tested as the security boundary it is. Accept its `AGPL-3.0-only` licence for that crate alone. |
| 14 | Radio contract | At most 4 radio accesses an hour (Verizon's guideline), an obligation on the application's cadence, telemetry batched inside one frame, the radio released within 5 s. |
| 15 | Departure board | Stays on LTE-M IP. NIDD suits low-duty sensors and, possibly, an e-paper variant on a core Verizon supports, which the nRF9151 is not. |
| 16 | Gate | Nothing merges to `main` until the bench nRF9160 Feather attaches to Verizon NB-IoT and the SIM's NIDD plan is confirmed. Never the nRF9151: Verizon does not support the board. First, and independent of NIDD: the SDK's public example worker is removed and the account credentials rotated (task 0.5). |
| 17 | Effort | 65 to 105 hours for the platform through a proven staging loop; 36 to 62 more for the device library and production. |
| 18 | Running cost | About $0.0004 per device-day on Cloudflare at 5-minute readings sent every 15 minutes, $0.0001 at hourly readings. NIDD carrier pricing is unpublished. |
| 19 | Who may create a Nidd pigeon | Only a flock in an organization listed in `NIDD_ALLOWED_ORG_IDS`, a fail-closed allowlist, while D2 keeps NIDD to JES's own devices. |

## 2. What NIDD is on Verizon, and what each fact fixes here

Sources, each read on 2026-09-24:

- [NIDD] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-non-ip-data-delivery.html
- [SEND] https://thingspace.verizon.com/documentation/apis/connectivity-management/api-reference/send-nidd-to-devices.html
- [CB] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-callback-services.html
- [CBBP] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-callback-services/best-practices.html
- [REG] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Register_Callback_Listener/
- [LOGIN] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Start_Connectivity_Management_Session/
- [CRED] https://thingspace.verizon.com/documentation/apis/connectivity-management/getting-started/getting-credentials.html
- [NUG] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/network-usage-guidelines.html

| Fact | Source | What it fixes in this design |
|---|---|---|
| "NIDD is only supported for NB-IoT devices currently"; the send API "currently supports NB-IoT devices only" | [NIDD], [SEND] | A NIDD device build forces NB-IoT; every LTE-M build, the departure board included, is out (sections 14, 16) |
| The device attaches a Non-IP PDN on APN `VZWSCEF` and indicates CP CIoT optimization | [NIDD] | The minted endpoint is `nidd://VZWSCEF`; the device reads the APN from it |
| The network "supports more than one simultaneous Packet Data Network (PDN) connection", IP and Non-IP together, for devices that support it | [NIDD] | A board may keep an IP PDN beside `VZWSCEF` for HTTPS firmware download, which is why a Nidd pigeon still gets a bearer token |
| NIDD is enabled by choosing the NIDD price plan on Activate, Restore or Change Service Plan, and the application must "wait for this additional callback before sending or received NIDD messages" | [NIDD] | Provisioning is done in the ThingSpace portal before a pigeon is created; the `niddConfigResponse` callback is logged, not acted on |
| "The maximum size of the data can be 10864 bit or 1358 bytes", base64 on the API | [SEND] | One 1358-byte budget per frame, counted decoded |
| "NIDD is capable of transporting up to 1500 bytes in a single transmission" | [NIDD] | The only published uplink figure. The bench modem refuses far less (B4: 1273 bytes accepted, 1283 refused), so the device caps an uplink frame at 1273 and batches; the platform accepts up to 1358 |
| `maximumDeliveryTime` "allowed range is 2 secs -- 2592000 secs (30 days)" | [SEND] | Downlinks use 30 s (D7): nothing buffered was seen delivered later |
| Unreachable device: "the data is buffered by the Verizon Network", a callback says it is buffered, and another says it "could not be delivered" once the window passes; statuses `Delivered`, `Queued`, `DeliveryFailed` | [NIDD], [SEND] | No reachability API, and on the bench nothing buffered reached a later wake or connection (B8, D13): the device's next uplink carries an owed push as its reply |
| Callbacks "must acknowledge receipt ... by sending back a 2xx status code"; unacknowledged ones are "resent by ThingSpace three more times at 5 minute intervals, for a total of 4 attempts", then archived 30 days and resendable through support by Request ID | [CB] | Not what B6 observed: every refused callback (403 or 503) drew exactly one more attempt 1.2 to 1.7 s later, then nothing for three hours. So 2xx only after the durable handoff, a 503 is a lost uplink unless that one retry lands, and de-duplication on `requestId` plus digest absorbs the retry |
| An acknowledgement deadline, the "2 seconds" in the brief | not found on [CB], [CBBP], [REG] or [SEND] (re-read live for [CB] on 2026-09-24) | Treated as a design target, not a contract: the synchronous path is one Postgres read, one DO hop of synchronous SQL and one enqueue, and every callback logs its latency (section 18, U1) |
| Username and password travel "as plain text in the callback messages"; "ThingSpace will not interact with any sort of authentication system" | [CB] | The password is one of three gates, never the only one |
| Callback username and password "Must be 40 characters or fewer" and must not be the UWS credentials | [REG] | A random 40-character alphanumeric password per environment |
| Callbacks come from 137.117.33.109, 168.62.173.153, 3.87.163.45, 3.91.119.203, 54.197.62.209, 35.165.205.14, 54.200.43.232, 34.216.81.234 | [CB] (same list on [REG]) | `THINGSPACE_CALLBACK_ALLOWED_IPS`, exact addresses, fail closed. They are Amazon and Microsoft cloud addresses on a list [CBBP] calls changing, so the gate is a filter, not a secret (4.2) |
| "you may only register one callback endpoint per service per account"; deregister the test URL before registering production | [CBBP] | Staging holds NiddService during bring-up, production at cutover; the other environment's allowlist is empty and, after cutover, its ThingSpace secrets are deleted (8.2) |
| HTTPS listeners need "a valid certificate registered with a legitimate certificate authority. A self-signed certificate will not work"; allowed ports include 443 | [CBBP], [REG] | The Workers custom domains (`api.pidgeiot.com`, `api-staging.pidgeiot.com`) on 443 |
| OAuth token "valid for one hour from when it was first issued, and any further token requests during that time will return the same token" | [CRED] | Cached until five minutes before expiry |
| Session token "will expire after 20 minutes of inactivity" | [LOGIN] | Reused while last used under 15 minutes ago |
| "The ThingSpace Platform will lock a contact record after 5 consecutive failed log in attempts" | [LOGIN] | One session object per environment, at most two failed logins per credential set and epoch |
| "Automated RF access attempts, for both mobile originations and terminations, should be limited to 4 per hour"; release the radio "within 5 seconds of the last byte" | [NUG] | The device contract of section 14, and batched telemetry |

## 3. capsules

Plain `String` fields: capsules stays free of Worker and Dioxus dependencies, and the SDK's
`DeviceID` would pull in a crate whose default feature is `worker`
(`thingspace-sdk-rust/Cargo.toml:57`). No `*Row` variant is needed: the config carries no
database-native timestamp and rides the existing `PigeonRow.connector` JSON text
(`capsules/src/lib.rs:241`).

Beside `CoapConfig` and `MqttConfig` (`capsules/src/lib.rs:517`, `:534`):

```rust
/// NIDD connector: the device's traffic rides Verizon ThingSpace's Non-IP Data Delivery, so the
/// carrier terminates the radio side. Uplink reaches dovecote as a ThingSpace callback and
/// downlink leaves through ThingSpace's API; no PidgeIoT terminator is involved.
///
/// `imei` binds the carrier's callbacks to this pigeon and fixes its id. `claim_key` is what the
/// device presents once per boot to prove it was built for this pigeon. `token` never rides
/// NIDD: it authorizes the HTTPS device routes, which a board reaches only over a second, IP PDN.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
#[serde(default)]
pub struct NiddConfig {
  /// `nidd://VZWSCEF`: the APN the Non-IP PDN attaches to, written as a URI so the device
  /// library can check the scheme against the transport it was built with.
  pub endpoint: String,
  /// Bearer token for the HTTPS device routes. Empty on every read route.
  pub token: String,
  /// The modem's 15-digit IMEI, supplied at create and fixed for the pigeon's life.
  pub imei: String,
  /// 32 lowercase hex characters (16 bytes). Returned by create and token refresh only, and
  /// never a TLS-PSK. It also keys the HMAC tag on every platform frame the device receives.
  pub claim_key: Option<String>,
}
```

`Connector` (`capsules/src/lib.rs:542-546`) gains `Nidd(NiddConfig)`. The wire form is externally
tagged like the others; the minimum create body is `{"Nidd":{"imei":"<imei>"}}`, the struct-level
`#[serde(default)]` filling the rest.

Method arms (`capsules/src/lib.rs:553`, `:565`, `:579`), with rustdoc updated in the same change:

- `token()`: `Connector::Nidd(c) => &c.token`. Doc sentence added: "For `Nidd` it authorizes only
  the HTTPS device routes; the carrier and the claim key authenticate the NIDD path."
- `psk()`: `Connector::Https(_) | Connector::Nidd(_) => return None`. Doc names `Nidd` beside
  `Https`: "and for `Nidd`, whose claim key is not a TLS-PSK and must never reach a terminator".
  This alone keeps the internal PSK route answering 404 for a Nidd pigeon
  (`dovecote/src/objects/pigeons.rs:1058`, `get_device_psk_internal`).
- `endpoint()`: `Connector::Nidd(c) => &c.endpoint`.
- `impl Default for Connector` (`:619`) stays `Https`.
- `PigeonCreateRequest` (`:332`) doc: "the connector names the variant; its contents are ignored,
  except a `Nidd` connector's `imei`, which binds the pigeon to its modem".
  `PigeonUpdateRequest` (`:358`) is unchanged and still carries no connector.

A wire-contract block after the MQTT one (`capsules/src/lib.rs:588-617`):

```rust
// --- NIDD wire contract ---
//
// The halves of the NIDD contract `~/pigeon` mirrors, the way `pigeonhole` mirrors the MQTT
// topics above. The frame layout itself lives in docs/api.md, which both sides follow.

/// APN of Verizon's NIDD service, and the authority of every minted `nidd://` endpoint.
pub const NIDD_APN: &str = "VZWSCEF";

/// Largest NIDD frame dovecote sends or accepts: Verizon's downlink cap of 10864 bits, counted
/// before base64. The modem's own uplink ceiling is lower (1273 bytes accepted, 1283 refused, on
/// an nRF9160 with mfw 1.3.7), so the device library holds its frames to 1273; docs/api.md states
/// that cap.
pub const NIDD_MAX_FRAME_BYTES: usize = 1358;

/// Largest serialized `target_config` a Nidd pigeon always accepts: one frame less the downlink
/// `SHADOW` frame's type byte, its version header at its longest (22 bytes, two ten-digit
/// versions) and its 16-character tag, all laid out under docs/api.md's NIDD frames. A write while
/// the versions are shorter may carry a little more.
pub const NIDD_MAX_TARGET_CONFIG_BYTES: usize = NIDD_MAX_FRAME_BYTES - 1 - 22 - 16;

/// Whether `imei` is 15 ASCII digits whose last digit is the Luhn check over the first 14.
/// Catches the single-digit slips an IMEI copied off a module label invites.
pub fn imei_is_valid(imei: &str) -> bool {
  let bytes = imei.as_bytes();
  if bytes.len() != 15 || !bytes.iter().all(u8::is_ascii_digit) {
    return false;
  }
  let sum: u32 = bytes
    .iter()
    .rev()
    .enumerate()
    .map(|(i, b)| {
      let d = u32::from(b - b'0');
      match i % 2 {
        0 => d,
        _ if d * 2 > 9 => d * 2 - 9,
        _ => d * 2,
      }
    })
    .sum();
  sum % 10 == 0
}
```

`imei_is_valid` lives in capsules because both sides need it: fancier checks the create form before
submitting and dovecote refuses a bad IMEI at create. The same Luhn loop was compiled and run in
the smallest design's scratch file (`nidd/kiss/luhn.rs` in the job directory): `490154203237518`
passes; a changed last digit, 14 digits, 16 digits and a letter fail.

Tests, extending `connector_tests` (`capsules/src/lib.rs:744`): `a_nidd_connector_round_trips`
(including `{"Nidd":{"imei":"490154203237518"}}` with the other fields defaulted);
`a_nidd_pigeon_row_parses` (closes the silent `unwrap_or_default` at `:241` for the new build);
`nidd_has_no_psk`; a `Nidd` case in `every_variant_reports_its_endpoint`; `imei_check_digit`.
`a_connector_in_the_body_is_ignored` still holds.

## 4. Identity and authentication

### 4.1 The chain

SIM (authenticates to the carrier) -> SCEF -> ThingSpace -> HTTPS callback carrying the listener
password, from one of eight published addresses -> dovecote gateway, which derives the pigeon's
Durable Object from the IMEI -> the DO, which stores nothing until the device has claimed it.
Three hops, three credentials: the SIM, the callback gates, and the claim key. The bearer token is
on none of them.

### 4.2 How a callback is trusted

Three gates, cheapest first, each failing closed, in the order of the existing service-internal
routes (`internal_psk_lookup`, `dovecote/src/lib.rs:484-554`) adapted to a credential that sits in
the body:

1. **Source address.** `CF-Connecting-IP` (set by Cloudflare's edge, not forgeable by a client)
   must appear in `THINGSPACE_CALLBACK_ALLOWED_IPS`. Exact matching is enough: Verizon publishes
   eight single addresses. The body of `is_allowed_coap_service_ip`
   (`dovecote/src/helpers/coap_service.rs:25`) becomes a private `is_allowed_by(env, req, var)`
   that both it and a new `is_allowed_thingspace_ip(env, req)` call, reusing `allowlist_matches`
   (`:39`) and its tests. It is its own var, never `COAP_SERVICE_ALLOWED_IPS`, which also opens
   the PSK route and exempts the terminator from the failed-auth limiter.
2. **Password.** The body's `password`, compared with the `THINGSPACE_CALLBACK_PASSWORD` secret
   by `constant_time_eq` (`dovecote/src/helpers/crypto.rs:9`). An unset or whitespace-only secret
   counts as unconfigured, the definition at `dovecote/src/lib.rs:503-511`, and here answers 503
   (5.1, step 4). For a day after a rotation a second secret,
   `THINGSPACE_CALLBACK_PASSWORD_PREVIOUS`, holds the password the rotation replaced and is
   accepted too (`match_callback_password`, both compared in constant time every time, logged as
   `password=previous`): B6 saw ThingSpace keep sending a replaced password for up to 13 min 55 s
   after the new one was registered, and with no resend after a refusal each such callback was
   lost (8.4). The username is registered as the constant `pidgeiot` and not checked: it carries
   no entropy.
3. **Account.** The `accountName` inside `niddResponse` must equal the `THINGSPACE_ACCOUNT_NAME`
   secret. This is the gate that stops the realistic spoof: any other ThingSpace customer can
   register our URL as their listener, so their callbacks arrive from the same eight addresses,
   and with a leaked password they would pass the first two gates. A mismatch is dropped with a
   200 and a log line.

What the gates are worth against a direct forger, stated plainly. The account gate stops only
bodies ThingSpace composes for another account; someone who posts from one of the eight addresses
writes `accountName` themselves, and it is not secret: a billing number every holder of the
account's API credentials sees. The addresses are cloud addresses, not Verizon's own:
3.87.163.45 sits in Amazon's `AMAZON-IAD` block and 137.117.33.109 in Microsoft's `MICROSOFT`
block (https://rdap.arin.net/registry/ip/3.87.163.45 and
https://rdap.arin.net/registry/ip/137.117.33.109, read 2026-09-24), and Verizon points at "the
latest listing of IP addresses" [CBBP], so the list changes and a released address returns to a
public pool. Against such a forger the listener password is the only real uplink secret, and every
holder of the API credentials can read it back [LIST]. After `HELLO`, the DO checks uplink frames
only for the claim and the line pin (4.4), never the key. Decision D12 records that residual.

Refusals are 403, never 401: fancier signs a tab out on any 401 from this API (CLAUDE.md, the
expired-session note), the reason `internal_consent_record` never answers 401
(`dovecote/src/lib.rs:547`). The route sits under `/internal/`, outside `/device/pigeons/*`, and is
not wrapped in `DeviceAuthGuard` (`dovecote/src/helpers/device_limits.rs:117-195`): eight shared
ThingSpace addresses behind one per-address failure budget would let one bad callback lock every
NIDD device out.

### 4.3 How a callback reaches its pigeon

A Nidd pigeon's Durable Object id is `PIGEONS.id_from_name("nidd:imei:<15 digits>")` (worker 0.8.6,
`src/durable.rs:82`) instead of `unique_id()` (`dovecote/src/lib.rs:1509`).

- The callback computes the same name from the IMEI it carries and reaches the DO directly. No
  reverse index, no Postgres read on the uplink path, no Hyperdrive cache, and no dependency on
  the best-effort Postgres mirror, so the "DB sync is best-effort" rule (CLAUDE.md conventions)
  holds with no exception.
- Uniqueness is free and atomic: a second create for the same IMEI lands in the same DO, which
  already holds a `pigeons` row and answers 409.
- Cost, accepted: the pigeon is bound to its modem for life. A replaced modem is a new pigeon, and
  no future connector-change route applies to a Nidd pigeon. A SIM moved to another board follows
  the board, which is the right identity for a device platform.
- Cost, handled: delete then recreate reuses the id. The create route clears any Postgres rows and
  the R2 log dictionary left under that id before mirroring the new pigeon (section 9).
- Why the IMEI: it names the modem, is printed on the module, is readable by firmware
  (`MODEM_INFO_IMEI`, `/home/justin/pigeon-nidd/nidd-test/src/main.c:142`), is what Verizon's MO
  example names at the top level, and is an accepted `kind` for downlink [SEND]. ICCID changes
  with a SIM swap; an MDN can be reassigned.
- Why not a Postgres index: every uplink would pay a lookup that Hyperdrive cannot usefully cache
  (one IMEI repeats every 15 minutes, never inside the 60-second cache), a new pigeon would be
  deaf until its best-effort mirror row existed, and making that insert strict would break the
  best-effort convention. The alternative that keeps a table for tenancy reasons is decision D1.

### 4.4 The claim key

An operator-typed IMEI is an assertion, not proof. Anyone who can create a pigeon can type any
valid IMEI, and on one shared JES ThingSpace account that would hand a stranger's device data and
downlink to whoever typed it first. So the DO stores nothing from a device, and sends it no
shadow, until the device has claimed the pigeon:

- Create mints a 16-byte `claim_key` with `mint_device_psk`
  (`dovecote/src/objects/helpers.rs:60`), reused unchanged, and returns it once in the 201 body,
  like a PSK.
- The device sends `HELLO` (type byte plus the key as 32 hex characters) at every boot, and
  again when told it is unclaimed.
- The DO compares in constant time; a match sets `claimed_at`. Every other frame from an unclaimed
  pigeon is dropped, and the device is told so at most once an hour (section 6.4).
- A good `HELLO` also pins the line: the DO stores the ICCID (or, failing that, the IMSI) from the
  callback's inner `deviceIds`, which Verizon's MO example carries ([SEND], saved copy
  `nidd/understand/vz/send-nidd-to-devices.txt:620-647`). A later frame whose callback names a
  different line is dropped as if unclaimed, without clearing the claim; a good `HELLO` from the
  new line moves the pin, which is how a SIM swap recovers. B5 records which identifiers real
  callbacks carry; if they carry neither, the pin is dropped and D12 says so.
- The key signs every platform frame. `SHADOW` and `STATUS` end in the first 8 bytes of HMAC-SHA256
  over the rest of the frame, keyed by the 16 key bytes and written as 16 hex characters, and the
  device drops any frame whose tag fails. Without it, anyone able to call ThingSpace's send API for
  the account (either dovecote environment, the owner's shell, any other holder of the API
  credentials) could send a `SHADOW` with `target_version` 0x7fffffff, which the device would keep
  over every real push, or a `PAUSED` that silences it for years. dovecote computes the tag with its
  existing WebCrypto `hmac_sha256` (`dovecote/src/helpers/stripe_webhook.rs:284`), so no crate is
  added. One exception: a `STATUS UNCLAIMED` answering a `HELLO` whose key did not match is signed
  with the key that `HELLO` presented, so a device built with a stale key can still verify it, and a
  forger's `HELLO` draws a notice the real device rejects. Frames carry no nonce: a replayed
  `SHADOW` loses to a newer version, and a replayed `STATUS` repeats an effect the platform already
  chose, a `PAUSED` for at most the device's 86400-second cap.
- Downlink is gated on the claim too: no shadow push goes to an unclaimed pigeon.
- The claim does not expire with the token. A NIDD-only board has no IP path to take a new token,
  so tying the claim to the token's one-year expiry would turn every expiry into a site visit.
- `token/refresh` mints a new token and a new claim key and clears `claimed_at`, so "refresh
  revokes the device until it is rebuilt", the dashboard's story for every other connector, holds
  for Nidd too.
- The key crosses the carrier in the clear once per boot. That is acceptable because it opens
  nothing by itself: a forged `HELLO` also needs a Verizon source address and the listener
  password (the account name is not secret, 4.2), and the claim then pins the line. It does put
  the downlink HMAC key in front of the carrier and ThingSpace, which already carry every frame.
  After `HELLO`, uplink frames are authenticated by the callback gates and the line pin, never by
  the key, so the listener password is what stands between a forger and a claimed pigeon's data;
  decision D12 records that. The bearer token would not be acceptable there: it opens the HTTPS
  device routes from anywhere.

### 4.5 What the bearer token does

It is minted at create and rotated by refresh as for every variant (`create`,
`dovecote/src/objects/pigeons.rs:790-797`), so `token()` stays total. It never rides a NIDD frame
(69 raw bytes each time, and readable by the carrier in every callback body). It authorizes the
HTTPS device routes, which a board reaches only over a second, IP PDN; firmware download is the
case that matters.

### 4.6 Tenancy

One ThingSpace account, JES's, holds every NIDD SIM, and dovecote carries one credential set per
environment. dovecote has no per-organization secret store, the bench SIM is on that account, and
Verizon's Terms reserve resale and making "the Services available to any third party"
(https://thingspace.verizon.com/legal/terms-of-service.html, read by the readers on 2026-09-24).
Whether customer lines may run on JES's account is decision D2. Until it is answered, NIDD is used
for JES's own devices, and the create route enforces that: a Nidd create is refused 403 unless the
flock belongs to an organization listed in `NIDD_ALLOWED_ORG_IDS`, a fail-closed var where empty
denies, the convention `DEMO_PIGEON_IDS` follows (`dovecote/src/helpers/demo.rs:3-9`). Without it
any account could probe JES's IMEIs through the create route's 409 and squat them, and a support
deletion would race the squatter's next create. The allowlist keeps other accounts out of create;
the claim key keeps even an allowlisted account from reaching a device it did not build. D1 is
revisited before D2 opens NIDD to customers.

### 4.7 What each kind of attacker can do

| Attacker holds | Can do | Cannot do |
|---|---|---|
| The callback URL only | Nothing: 403 at the address gate | |
| A Verizon source address (another ThingSpace customer registering our URL) | Reach the password gate | Pass it without the password; pass the account gate at all |
| Also a leaked callback password, through that registration | Reach the account gate | Pass it: ThingSpace writes `accountName`, ours only on our account's callbacks |
| A direct path from one of the eight addresses (a host there, or an address Verizon has released) | Write its own `accountName`, so only the password gate is left | Pass it without the password |
| That path and the listener password | Inject `TELEMETRY` and `SHADOW_REPORT` into a claimed pigeon whose IMEI and ICCID it knows (D12) | Steer the device: every platform frame carries the claim key's tag |
| JES's ThingSpace API credentials | Read the listener password back (`GET /callbacks` returns it [LIST]), re-register `NiddService` elsewhere and so divert every uplink, send frames to any line on the account | Post to our route from outside Verizon's addresses; steer a device, which drops frames without the claim key's tag. The remedy is section 8.4's rotation |
| A dashboard account outside `NIDD_ALLOWED_ORG_IDS` | Nothing: Nidd create answers 403 before any IMEI lookup, so no 409 confirms an IMEI | |
| A manager in an allowlisted organization | Create a Nidd pigeon for any valid IMEI, receiving a claim key | Receive a byte from or send a byte to a device it did not build; the rightful create then answers 409, and the organization holding the pigeon, JES's own, deletes it |
| A device's claim key | Claim that device's pigeon from a line Verizon reports under that IMEI, pinning that line; with API credentials too, sign frames that device accepts | Claim it without a Verizon source address and the listener password |

[LIST] is https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/List_Callback_Listeners/
(read by the readers on 2026-09-24).

## 5. Routes

One new route. No listener-management route and no send route (5.3).

### 5.1 New: the NiddService callback, as `docs/api.md` will carry it

Under `## Service-internal API`, after `### Consent hooks` (`docs/api.md:3225`). The outer fence
below has four backticks only so this document can nest the example; `docs/api.md` gets the inner
text.

````markdown
### ThingSpace NIDD callbacks

#### `POST /internal/thingspace/nidd`

**Auth:** ThingSpace callback credentials required

Verizon ThingSpace's `NiddService` callback: every uplink from a `Nidd` pigeon
(`niddMONotificationResponse`), every downlink delivery report (`niddMTDeliveryResponse`) and every
line-configuration result (`niddConfigResponse`) arrives here. Not a device or dashboard route; the
only legitimate caller is ThingSpace. Three gates, each failing closed: the source address
(`CF-Connecting-IP`) must appear in the environment's `THINGSPACE_CALLBACK_ALLOWED_IPS` (Verizon's
published callback addresses; empty means deny-all); the body's `password` must equal the
`THINGSPACE_CALLBACK_PASSWORD` Worker secret, compared in constant time (for a day after a rotation,
the password it replaced, `THINGSPACE_CALLBACK_PASSWORD_PREVIOUS`, is accepted too, since ThingSpace
keeps sending it for minutes); and `accountName` must be this environment's ThingSpace account.
ThingSpace sends the password in clear text inside the body, which is why the other two gates are
not optional. Any `Content-Type` is accepted, and the body is capped at 8 KiB before it is parsed.

Body is ThingSpace's callback JSON, unchanged. An uplink:

```json
{"username": "pidgeiot", "password": "<callback_password>", "requestId": "<uuid>",
 "deviceIds": [{"id": "<imei>", "kind": "IMEI"}],
 "niddResponse": {"niddMONotificationResponse": {"accountName": "<account_name>",
   "message": "<base64 frame>", "deviceIds": [{"id": "<imei>", "kind": "IMEI"}]}},
 "callbackCount": 1, "maxCallbackThreshold": 4}
```

The pigeon is the one whose `connector.Nidd.imei` equals the IMEI among
`niddMONotificationResponse.deviceIds`, falling back to the top-level `deviceIds`, with `kind`
compared case-insensitively. `message` decodes to one frame; see [NIDD frames](#nidd-frames).

```sh
curl -s -X POST https://api.pidgeiot.com/internal/thingspace/nidd \
  -H 'Content-Type: application/json' \
  -d '{"username":"pidgeiot","password":"<callback_password>","requestId":"<uuid>","deviceIds":[{"id":"<imei>","kind":"IMEI"}],"niddResponse":{"niddMONotificationResponse":{"accountName":"<account_name>","message":"<base64 frame>","deviceIds":[{"id":"<imei>","kind":"IMEI"}]}},"callbackCount":1,"maxCallbackThreshold":4}'
```

From an address outside the allowlist this answers `403`. The example is the shape ThingSpace
sends, for replaying one against a local `wrangler dev`, whose allowlist is loopback.

- `200`, empty body: processed, or deliberately dropped because a retry could not change the
  outcome: an IMEI no pigeon is bound to, a pigeon whose device has not claimed it, an account
  over its free-tier allowance, a repeat of a callback already stored, a frame that is malformed,
  over a cap or of an unknown type, a delivery report (a `DeliveryFailed` or `Queued` one first
  marks the push it may have carried pending), a configuration result, another account's
  callback, or an authenticated body of a shape dovecote does not know.
- `400`: the body is not JSON, or carries no `password`. ThingSpace keeps it in its 30-day
  archive, resendable through support once a parser is fixed.
- `403`: source address outside the allowlist, or a wrong password. **Never `401`**, for the same
  reason as [`POST /internal/consent`](#post-internalconsent).
- `413`: body over 8 KiB.
- `503`: NIDD is not configured in this environment (the callback password or the account name
  is unset), or a store the uplink needs failed (the pigeon's Durable Object or the telemetry
  queue). **The uplink is lost** unless ThingSpace's one retry, about a second later, succeeds:
  ThingSpace documents three resends at five-minute intervals, but makes that single immediate
  retry and no other. dovecote logs each such answer as lost with its `requestId`, the only
  handle for a support resend from ThingSpace's archive. A retry of a callback that was in fact
  stored is recognised by its `requestId` and frame digest and never stored or billed twice, and
  one that arrives while the first attempt is still storing it waits for that attempt's outcome.
````

Glance row, in document order after the `POST /internal/consent` row (`docs/api.md:103`), with the
Auth cell repeating the heading's `**Auth:**` text verbatim:

```text
| [`POST /internal/thingspace/nidd`](#post-internalthingspacenidd) | ThingSpace callback credentials required | Verizon ThingSpace delivers a NIDD uplink, delivery report or line result |
```

**Handler.** A named `async fn nidd_callback(req, ctx)` registered beside `/internal/consent`
(`dovecote/src/lib.rs:1228`). `let cors = build_cors(&ctx.env, &req);` first (`lib.rs:66`); every
return is `.with_cors(&cors)`; every refusal an explicit `let ... else`, never `?`.

1. `is_allowed_thingspace_ip(&ctx.env, &req)` or 403, logging the refused address as
   `internal_psk_lookup` does (`lib.rs:491-497`).
2. `req.text()`, then over `NIDD_CALLBACK_MAX_BYTES` (8192) is 413, the contact route's pattern
   (`lib.rs:3553-3563`). The largest legitimate body, a 1358-byte frame's base64 inside the
   564 bytes B5 measured around it, is under 2400 bytes (section 17).
3. `serde_json::from_str::<CallbackAuth>` (three fields, `password: Option<String>`,
   `request_id: Option<String>` renamed from `requestId` and `callback_count: Option<i64>` renamed
   from `callbackCount`): not JSON or no password is 400.
4. `THINGSPACE_CALLBACK_PASSWORD` and `THINGSPACE_ACCOUNT_NAME` present and non-blank, or 503: the
   service is what is missing, not the caller's right to it (the Stripe webhook precedent,
   `lib.rs:5368-5373`). It does not defer the callback: ThingSpace retries once at once and never
   later (B6), so a deploy gap loses what arrives in it, and the route logs each loss. Then
   `constant_time_eq` against the secret: 403 on mismatch. This check follows the parse so that the
   no-password, not-configured and wrong-password lines each carry the body's request id and attempt
   (`nidd_cb outcome= request= attempt=`), unverified but enough to match a refusal to ThingSpace's
   next attempt (B6 could not).
5. `serde_json::from_str::<NiddCallback>` (dovecote's own struct, `helpers/nidd.rs`, section 6.6):
   a body that authenticates but does not parse is logged with `CallbackAuth`'s `request_id` (or
   `none`) and the parse error's `e.classify()` and `e.column()` only, and answered 200.
6. Dispatch on the `niddResponse` variant:
   - **Uplink.** `accountName` differs: 200 and a log line. `callback_imei(&callback)` finds no
     IMEI, or the `message` is not standard base64, or it decodes to zero bytes: 200 and a log line.
     Otherwise derive the object with `namespace.id_from_name(&nidd_object_name(&imei))`. If byte 0
     is a billable type (`TELEMETRY` or `SHADOW_REPORT`), run `check_ingest_fuse(&ctx.env,
     &pigeon_id)` here, exactly where the HTTP telemetry route runs it (`lib.rs:981-988`), raced
     against a one-second `Delay` in the pattern at `dovecote/src/helpers/turnstile.rs:118-132`, and
     carry the answer as `X-Nidd-Ingest: paused` or `open`. The race is needed because the fuse
     opens a Hyperdrive socket and runs an uncached query with no deadline of its own
     (`dovecote/src/helpers/usage.rs:603-639`, `dovecote/src/helpers/hyperdrive.rs:18-31`), before
     the acknowledgement, on every billable callback, resends included. A timeout fails open and is
     logged, the fuse's own error rule (`usage.rs:597-599`). Then `nidd_uplink_via_do(&obj_id,
     &frame, &request_id, ingest, line)` (new, in `helpers/pigeons.rs` beside `psk_lookup_via_do`,
     `:130`: a bare internal POST of the raw frame bytes to `/pigeon/nidd/uplink` in the binary-safe
     style of `proxy_binary_to_pigeon_do`, `:188`, carrying `X-Nidd-Request-Id`, `X-Nidd-Ingest`
     and, when `callback_line` finds one, `X-Nidd-Line`, never a caller header). DO 2xx is 200; DO
     404 (no pigeon behind that IMEI) is 200 and a log line naming the derived pigeon id; DO 5xx or
     a dispatch error is 503, and a line naming the uplink as lost.
   - **Delivery report.** Log `requestId`, `status`, `reason` and the derived pigeon id. 200. A
     `DeliveryFailed` or `Queued` report first goes to the DO at `/pigeon/nidd/missed`, which
     marks the owed push pending (6.4), and the line reads `outcome=pending` when one was owed; a
     failed hop is logged and left to the push's delivery window.
   - **Configuration result.** Log `status` (`ConfigCreated`, or a failure with its `reason`) and
     the derived pigeon id. 200.
7. Logging rule for the whole route: request ids, derived pigeon ids, statuses, sizes and latencies.
   Never the body, the password, the frame, the account name, the IMEI, the ICCID or the IMSI. Never
   a serde error's `Display` either: it quotes the offending value, so an IMEI sent as a JSON number
   logs as ``invalid type: integer `490154203237518`, expected a string at line 1 column 21``
   (serde_json 1, reproduced in `nidd/fix-serde-probe/` in the job directory). Every parse failure,
   of `CallbackAuth`, `NiddCallback` or a frame's body, logs `e.classify()` and `e.column()` only,
   through one helper (6.6). Every callback logs one line,
   `nidd_cb kind= outcome= pigeon= request= attempt= ms=`, so `ms` can be watched against the
   unpublished acknowledgement deadline; one refused before it is trusted logs
   `nidd_cb outcome= request= attempt=` (step 4).

### 5.2 Changed routes

**`POST /flock/pigeons`** (`docs/api.md:1100`, route at `dovecote/src/lib.rs:1444`). Text added to
the body paragraph at `docs/api.md:1110-1115`, then a curl example, then a third blockquote after
the MQTT one (`docs/api.md:1138-1145`):

````markdown
`connector` may also be `{"Nidd": {"imei": "<imei>"}}`. For `Nidd` the `imei` is read: it must
be 15 digits ending in its Luhn check digit, and it fixes the pigeon's id for life, so a
replaced modem is a new pigeon. Every other connector field is still ignored and minted
server-side. New answers: `400` for an IMEI that fails the check; `403` "Forbidden: NIDD is not
enabled in this environment" when this deployment has no ThingSpace account configured; `403`
"Forbidden: NIDD is not enabled for this organization" when the flock's organization is not
allowlisted, checked before the IMEI is looked at; `409` "Conflict: a pigeon with this IMEI
already exists". The `201` body's `connector.Nidd` carries
`endpoint` (`nidd://VZWSCEF`), `token`, `imei` and `claim_key`; like the token, the claim key
is shown only here and by `token/refresh`.

```sh
curl -s -X POST https://api.pidgeiot.com/flock/pigeons \
  -H 'Cookie: ory_kratos_session=<session_token>' \
  -H 'Content-Type: application/json' \
  -d '{"flock_id":"<flock_id>","name":"Field Sensor 1","connector":{"Nidd":{"imei":"<imei>"}}}'
```

> **NIDD is terminated by the carrier, not by the edge Worker or a PidgeIoT service.** A `Nidd`
> device attaches a Non-IP PDN on Verizon's `VZWSCEF` APN over NB-IoT; its uplink reaches dovecote
> as a ThingSpace callback and its downlink leaves through ThingSpace's API. The minted endpoint
> is `nidd://VZWSCEF`. No PSK is minted. The bearer token is minted as for every variant but never
> rides NIDD; it serves the HTTPS device routes for a board that also holds an IP PDN. The device
> proves it was built for this pigeon with the claim key. See
> [NIDD device surface](#nidd-device-surface-via-verizon-thingspace).
````

Gateway change, for a `Nidd` connector only, after the flock authorization (`lib.rs:1472-1493`):
the `THINGSPACE_ACCOUNT_NAME` check (403); the organization allowlist, `flock.org_id` (known from
`lib.rs:1482-1485`) listed in `NIDD_ALLOWED_ORG_IDS` or 403, a personal flock never passing, so an
account outside it never reaches a 409; `capsules::imei_is_valid` (400); then
`let Ok(obj_id) = namespace.id_from_name(&nidd_object_name(&imei)) else { ... 500 }` in place of
`unique_id()`. The existing `unique_id().map_err(...)?` at `lib.rs:1509-1512` is a `?` inside a
route closure; the same change rewrites it as a `let ... else`. After the DO's 201, the mirror
insert (`lib.rs:1561`) runs the clean slate of section 9 inside its own transaction.

**`PUT /pigeons/:pigeon_id/shadow`** (`docs/api.md:1489`). One response added:

> - `413` "Payload Too Large: this NIDD pigeon's target_config must serialize to at most <n>
>   bytes", for a `Nidd` pigeon, checked before anything is written. One downlink frame carries
>   the whole `target_config`, and a config the device could never receive must not become its
>   target. `<n>` is one frame less the `SHADOW` frame's type byte, version header and tag, so it
>   is never below 1319 and up to 1337 while the versions are short.

**`POST /pigeons/:pigeon_id/token/refresh`** (`docs/api.md:1358`). Added to `:1364-1367`:

> For a `Nidd` pigeon the refresh also mints a new claim key and marks the pigeon unclaimed; the
> IMEI and endpoint are kept. The device is refused until it is rebuilt with the new key.

**Read routes** (`GET /pigeons/:pigeon_id`, `/detail`, the list): `connector.Nidd` keeps
`endpoint` and `imei`, with `token` empty and `claim_key` null (`strip_secrets`, section 6.5).

**`DELETE /pigeons/:pigeon_id`**: no surface change. The DO also wipes its NIDD state (section 6.1),
the IMEI stops resolving, and it can be registered again.

### 5.3 No owner-facing management route

- **Listener registration** is once per environment, displaces whatever holds `NiddService` on the
  account ("one callback endpoint per service per account" [CBBP]), and exposes the listener
  password in clear on read ([LIST]). An HTTP route would put that power behind a web credential
  for no recurring use. It is the runbook of section 8.5.
- **Sending** starts only inside a pigeon's Durable Object, from a shadow write or an uplink. A
  send route would need its own authorization story and would let a dashboard user spend carrier
  money outside the shadow's rules.
- **Listener drift**, someone re-registering `NiddService` elsewhere, is silent uplink loss. v1
  checks it by hand at every deploy (section 8.5, step 2); an hourly automated check is decision
  D11.

### 5.4 The rest of `docs/api.md`

Changed text:

- `:115-117`: "two service-internal routes" becomes three, naming ThingSpace's callback, which
  authenticates by source address, callback password and account rather than a shared service
  secret.
- `:189-192`: the token-field list gains `connector.Nidd.token`, and the claim key beside the PSK
  as the other write-once secret.
- `:1187-1191`, "The connector is a provisioning hint, not a transport boundary": true for the three
  IP variants; a Nidd pigeon's uplink is authenticated by the SIM, the callback gates and the
  claim, and reaches only the pigeon its IMEI names.
- `:3210-3212`, the PSK lookup's 404 list: `Https` and `Nidd` mint no PSK.
- Type reference (`:3294-3295`): `Connector` gains `Nidd(NiddConfig)`; `NIDD_APN`,
  `NIDD_MAX_FRAME_BYTES`, `NIDD_MAX_TARGET_CONFIG_BYTES`, `imei_is_valid`.
- Rate & size limits (`:252`): rows for the NIDD frame (1358 bytes each way: the downlink held
  to it by the 413 below, the uplink by the device build), a Nidd `target_config` (1319 bytes at
  worst, 413), the
  callback body (8 KiB, 413), the downlink delivery window (30 s), status notices (one an hour
  per pigeon), unsolicited pushes (one per 15 minutes per pigeon) and the de-duplication window
  (the last 64 uplinks per pigeon).

New text: `---`, then `## NIDD device surface (via Verizon ThingSpace)` before
`## Service-internal API` (`:3177`), modelled on the MQTT surface (`:3101-3173`); slug
`nidd-device-surface-via-verizon-thingspace`. Its H3s are prefixed so no slug collides with an
existing heading (the MQTT section already owns `### Rotation and deletion`, `:3159`):

- `### NIDD network and provisioning`: NB-IoT only, `VZWSCEF`, the NIDD price plan and the
  `ConfigCreated` wait [NIDD]; the IMEI binding.
- `### NIDD binding and claim`: sections 4.3 and 4.4.
- `### NIDD frames`: the tables and exact bytes of section 7.
- `### NIDD downlink and replies`: section 6.4.
- `### NIDD sizes and cadence`: the frame budget and the device contract of section 14.1.
- `### NIDD billing and the free tier`: one billable message per telemetry reading and per stored
  shadow report, never for a resend or a `HELLO`; downlinks unmetered; a paused account's uplink
  dropped with a `PAUSED` notice.
- `### NIDD SIM changes and deletion`: a new modem is a new pigeon; refresh unclaims; delete frees
  the IMEI.

No route H4 sits in that H2: the surface has no HTTP route of its own.

What `fancier/src/helpers/api_doc.rs` enforces, and how this text meets it: the route H4 is
exactly a backticked `METHOD /path` and sits under an H3; a non-empty `**Auth:**` line follows;
the glance row repeats the Auth text verbatim and falls in document order after the consent row;
its purpose cell is over ten characters; every new in-document link (`#nidd-frames`,
`#post-internalconsent`, `#post-internalthingspacenidd`,
`#nidd-device-surface-via-verizon-thingspace`) resolves to a GitHub-slugged heading; fences are
three backticks only; example bodies carry placeholders (`<callback_password>`, `<imei>`,
`<account_name>`), never values (`docs/api.md:17-18`). `cargo test -p fancier --target
x86_64-unknown-linux-gnu` proves it.

Beyond `docs/api.md`: the CLAUDE.md connectors paragraph (a fourth variant; it still says "`Https`
or `Coap`", stale since MQTT), CLAUDE.md's free-tier-fuse paragraph (the NIDD callback becomes the
second surface checked at the gateway, 6.3 step 6) and `README.md:77` ("Cellular NIDD has no
implementation anywhere yet"), both once the platform half is live.

## 6. Durable Object changes

### 6.1 `Pigeons`: storage

One single-row table, created in `DurableObject::new` beside the others
(`dovecote/src/objects/pigeons.rs:172-347`) and shaped like `pigeon_telemetry_latest` for the same
billing reason: `id INTEGER PRIMARY KEY` is the rowid, so no backing index, one row read and one
row written per uplink whatever happens, plus the second read step 10 of 6.3 needs after a
telemetry enqueue.

```sql
CREATE TABLE IF NOT EXISTS pigeon_nidd (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  claimed_at INTEGER,
  line_id TEXT,
  awaiting_version INTEGER NOT NULL DEFAULT 0,
  pushed_version INTEGER NOT NULL DEFAULT 0,
  pushed_at INTEGER NOT NULL DEFAULT 0,
  notice_at INTEGER NOT NULL DEFAULT 0,
  seen TEXT NOT NULL DEFAULT '[]'
);
```

| Column | Holds |
|---|---|
| `claimed_at` | Unix seconds of the last good `HELLO`; NULL until claimed and after a refresh |
| `line_id` | The ICCID (or IMSI) that good `HELLO` arrived from; NULL when its callback named neither, and after a refresh |
| `awaiting_version` | The newest `target_version` the device has not yet confirmed; 0 when converged |
| `pushed_version`, `pushed_at` | The `target_version` of the last `SHADOW` frame sent and when; both 0 after a failed send |
| `notice_at` | When the last `PAUSED` or `UNCLAIMED` notice went out, for the once-an-hour limit |
| `seen` | JSON array of the last 64 de-duplication keys, oldest first |

An absent row means all defaults (unclaimed). `delete` (`:1095-1140`) gains
`DELETE FROM pigeon_nidd;` beside the other explicit wipes, since the table has no foreign key to
`pigeons`. No alarm is used, so none needs clearing. Never add an index to this table, for the
reason CLAUDE.md gives for the telemetry blob: rows read are rows scanned, and an index can only add
rows written.

### 6.2 `Pigeons`: dispatch

Two trusted-internal paths in `fetch` (`:349-385`): `"/pigeon/nidd/uplink" => nidd_uplink(self,
req).await` and `"/pigeon/nidd/missed" => nidd_missed(self)`, a delivery report's hop (6.4). The
uplink's body is the raw frame bytes; its headers are `X-Nidd-Request-Id`, `X-Nidd-Ingest`
(`paused` or `open`), and `X-Nidd-Line` (the callback's ICCID or IMSI) when there is one. Their
trust argument is the one `grant_acl_internal` (`:362`) and `write_telemetry_device` (`:370`) rest
on: the DO has no public address, and the only caller is the callback route after all three of its
gates.

### 6.3 `Pigeons`: the uplink path, `nidd_uplink`

Every read and write below is synchronous SQL. The only `await` before the response is the queue
enqueue in step 7, which is why the de-duplication key is recorded only after it succeeds, and why
step 10 re-reads the row and applies this uplink's changes to the fresh copy: a dashboard write or
another callback may have changed it during the await, and two synchronous statements cannot be
interleaved. The object serves other requests during that await, ThingSpace's retry of this very
callback among them: a callback still unanswered after about 4 s is retried while the first
attempt runs (13.3). So step 7 holds the key in memory while it awaits, and a retry that finds it
there waits in step 2 for that attempt's outcome. A refused callback is retried once, 1.2 to
1.7 s after the refusal was answered (B6).

1. Read `pigeon_nidd` (defaults if absent). The `pigeons` row is read, with `one_row` (`:164`,
   never `SqlCursor::one()`), only when it is needed: for a `HELLO` (the claim key), while
   `claimed_at` is NULL (to tell an unclaimed pigeon from no pigeon; no row is 404, and the
   gateway answers ThingSpace 200), and when a downlink is planned (the IMEI, and the claim key
   that signs the frame). A claimed row
   implies a live pigeon, since `delete` wipes both, so the steady-state path skips it.
2. De-duplication key: `X-Nidd-Request-Id` plus the first 16 hex characters of the frame's SHA-256
   (`sha2` is already a dovecote dependency). A key already in `seen` answers 200 `duplicate` and
   writes nothing. A key an attempt holds in step 7 makes this one wait for it: `duplicate` once
   that attempt stored or refused the frame, and on from step 1 once it failed. A resend repeats
   both halves, so it is recognised. The key keeps two uplinks apart only when their request ids or
   their bytes differ: [SEND] defines `requestId` for downlink callbacks alone ("All of the callback
   messages have the same requestId") and says nothing on uplink uniqueness, and several frames
   repeat byte for byte across wakes (every boot's `HELLO`, a converged shadow report, a flat
   telemetry map whose values have not changed). If an uplink request id ever repeats inside the
   64-key window, the later frame is dropped as a duplicate: a boot's `HELLO` gets no `SHADOW`, or a
   reading is lost. B5 is the gate: two identical `HELLO`s sent from two wakes must both reach the
   DO as new. If they do not, the frame layout gains a device sequence byte before task 3.1 fixes
   it.
3. Empty frame or unknown type byte: record the key, 200 `rejected`, one log line with the type
   byte and length only.
4. `HELLO` (`0x04`): the 32 hex characters decoded and compared in constant time with the stored
   `NiddConfig`'s `claim_key`. Match: `claimed_at = now`, `line_id` = the `X-Nidd-Line` value, plan
   a `SHADOW` reply. Mismatch: plan `STATUS UNCLAIMED` with argument 1, signed with the presented
   key (4.4), if a notice is due. Wrong length: no notice. Record the key, 200.
5. Any other type while `claimed_at` is NULL, or naming a line other than a stored `line_id`: plan
   `STATUS UNCLAIMED` with argument 0 if due, record the key, 200 `unclaimed`. The notice is how a
   device that booted before its pigeon existed learns to send `HELLO` again. Argument 0 asks for
   a `HELLO`, never for silence: callbacks arrive out of order (7.4), so a notice planned for a
   frame processed before the same wake's `HELLO` can reach a device whose claim has just
   succeeded. Only a failed `HELLO` draws argument 1, the one that stops billable sends.
6. Billable type (`TELEMETRY`, `SHADOW_REPORT`) with `X-Nidd-Ingest: paused`: plan a `PAUSED`
   notice (argument 3600) if due, record the key, 200 `paused`. The gateway ran the fuse, as the
   HTTP telemetry route does, so this path opens no Postgres connection. That departs from the rule
   `device_ingest_paused`'s rustdoc states (`dovecote/src/objects/pigeons.rs:1481-1495`: the check
   lives in the DO for surfaces that authenticate and write in one DO hop, HTTP telemetry being
   the exception) and CLAUDE.md's free-tier-fuse paragraph repeats. Task 1.6 updates the rustdoc
   and task 1.10 the paragraph: the NIDD callback is the second surface checked at the gateway,
   because a Hyperdrive connection opened from the DO may bill it for DO duration (section 17,
   U14).
7. `TELEMETRY` (`0x01`): parse the rest of the frame as `capsules::TelemetryReportBody` (flat or
   batched, `capsules/src/lib.rs:738`). Once it parses, claim the key in memory (`NiddInFlight`,
   6.6), `ingest_telemetry(pigeons, body)` (6.7), and settle the claim the moment it returns, which
   answers every retry waiting in step 2. The claim never reaches `seen`, so it ends with the
   attempt: an object reset inside the await leaves nothing to turn the retry away. No reading is
   aged for its attempt: a retry that stores follows a refusal by about a second (B6), inside the
   resolution of a reading's own `age_secs`, or follows a slow attempt that failed by a few seconds
   (row 12). `Stored`: record the key, 200. `Rejected` (malformed, over a cap): record the key, 200
   `rejected`, logged by the parse error's `e.classify()` and `e.column()`, never its `Display`,
   which would quote the frame's values (5.1, step 7); step 8's parse logs the same way. `Failed`
   (the merge or the enqueue): record nothing, 503, the honest answer, which the gateway logs as a
   lost uplink; if ThingSpace's one retry lands it redoes the work. A merge that succeeded before a
   failed enqueue is re-applied with the same values, but the retry's rate alerts then diff its
   readings against the failed attempt's newest values rather than what preceded the uplink, so they
   can miss a real step or fire on one that never happened.
8. `SHADOW_REPORT` (`0x02`): parse `PigeonShadowReportRequest` (`capsules/src/lib.rs:493`). If
   `(current_version, current_config)` equals what is stored, it is the device repeating itself:
   record the key, no bill, reply as below. Otherwise `write_shadow_report` (`:1534`, synchronous
   already), record the key, and mark the tail to bill one message and sync Postgres. Then
   `awaiting_version` becomes `target_version` if the device is behind, else 0. Reply: a `SHADOW`
   when `target_version > current_version`, else `STATUS STORED` carrying the version. The
   report is the one confirmed call in the device library, so it always gets exactly one reply.
9. After a `TELEMETRY`, stored or refused, and after a billable frame dropped while paused when
   no notice is due, plan a `SHADOW` if `shadow_reply_due(&row, now)` (6.4): the reply goes out
   while the connection the uplink opened is still up. `HELLO` and reports plan theirs in steps 4
   and 8.
10. Re-read `pigeon_nidd`, apply this uplink's changes to that copy (the key appended to `seen` and
    trimmed, and any claim, notice or push fields), write it once (upsert), respond 200, then hand
    the tail to `wasm_bindgen_futures::spawn_local` (the pattern at
    `dovecote/src/helpers/hyperdrive.rs:41`).
    The Durable Object stays active while that I/O is pending, and `waitUntil` "has no effect in
    Durable Objects" (https://developers.cloudflare.com/durable-objects/api/state/, read
    2026-09-24), so `spawn_local` is the spelling that says what happens. The tail owns clones of
    `env`, the `SqlStorage` handle (`Clone`, worker 0.8.6 `src/sql.rs:179`) and the pigeon id, and
    runs, in order: for a newly stored report, `count_billable_messages(env, id, 1)` and
    `update_shadow_pg_db` exactly as `handle_ws_shadow_report` does today (`:1797-1828`); then the
    planned downlink, if any (6.4).

Row cost of a steady-state telemetry uplink: `pigeon_nidd` two reads (before and after the
enqueue) and one write, `pigeon_telemetry_latest` one read and one write. Nothing else: the claim
lives in memory and costs no row.

### 6.4 `Pigeons`: downlink

**The trigger.** `update_shadow` (`:2574-2627`) is the only hop that knows the connector; the
gateway's `PUT /pigeons/:pigeon_id/shadow` only proxies (`dovecote/src/lib.rs:2043`). It gains a
`read_connector(pigeons) -> Result<Connector>` (`SELECT connector FROM pigeons LIMIT 1;` through
`one_row`) and, for `Nidd`:

1. Before the write: 413 when the serialized `target_config` (the `config_str` it already builds)
   is longer than one `SHADOW` frame carries at the version the write creates, counted by
   `shadow_config_cap` with the device converged on it (the longest header the push can carry
   until the next write); never below `capsules::NIDD_MAX_TARGET_CONFIG_BYTES`.
2. After the write and the unchanged `broadcast_shadow_update` (`:2636`; a dual-PDN board may
   also hold a socket): read `pigeon_nidd`; if the new `target_version` exceeds
   `awaiting_version`, set it; if `shadow_push_due`, plan a `SHADOW`; write the row; respond; send
   in a `spawn_local` tail. The dashboard PUT never waits on ThingSpace and never fails because of
   it, the rule the WebSocket push already follows.

**The push rule**, two pure functions in `helpers/nidd.rs`, one per trigger:

```rust
/// Whether a dashboard write pushes the shadow unsolicited: the device is claimed and behind, the
/// newest target has not been sent, and no `SHADOW` went out in the last hold window.
pub fn shadow_push_due(row: &NiddRow, now: i64) -> bool {
  row.claimed_at.is_some()
    && row.awaiting_version != 0
    && row.pushed_version < row.awaiting_version
    && now - row.pushed_at >= NIDD_PUSH_HOLD_SECS
}

/// Whether an uplink from the claimed device draws the `SHADOW` it is owed as its reply, sent
/// while that uplink's connection is up: the device is behind, and the newest target is unsent,
/// held or pending, or went out longer ago than its delivery window without being confirmed.
pub fn shadow_reply_due(row: &NiddRow, now: i64) -> bool {
  row.claimed_at.is_some()
    && row.awaiting_version != 0
    && (row.pushed_version < row.awaiting_version || now - row.pushed_at > NIDD_MT_DELIVERY_SECS)
}
```

`NIDD_PUSH_HOLD_SECS` is 900 and `NIDD_MT_DELIVERY_SECS` is 30, dovecote's own; the second is
also every downlink's `maximumDeliveryTime` (D7).

Two rules because only a device that holds a connection reliably receives a push. In the B7
retest, with PSM and eDRX off, two pushes to the RRC-idle device were paged 12.6 and 13.8 s after
the send, after ThingSpace had already reported `DeliveryFailed` or `Queued` at about 10 s, and
neither arrived later; every frame sent while the device held the connection its own uplink
opened arrived whole (D13). So the dashboard's push stays a bounded best effort, and the reply to
the device's next uplink is what delivers: any uplink from the claimed line draws the owed
`SHADOW` unless the newest one went out inside its 30 s window and may still land. A `SHADOW`
also refreshes `awaiting_version` from the shadow it carries, so a device that converged by
another path stops being owed one.

| Event | Downlink | Row change |
|---|---|---|
| Dashboard write raising `target_version`, push due | `SHADOW` | `awaiting_version`, `pushed_version`, `pushed_at` |
| Dashboard write inside the hold window, or pigeon unclaimed | none; the next uplink's reply carries the newest | `awaiting_version` |
| `HELLO` that matches | `SHADOW` always (the device asked) | `claimed_at`, `pushed_*`; `awaiting_version` = target if behind, else 0 |
| Shadow report, device behind | `SHADOW` (the reply) | `awaiting_version` = target, `pushed_*` |
| Shadow report, device converged | `STATUS STORED <version>` | `awaiting_version = 0` |
| Telemetry, or a billable frame while paused with no notice due, reply due (the newest target held, unsent, marked missed, or out over 30 s unconfirmed) | `SHADOW` (the reply) | `awaiting_version`, `pushed_*` |
| `DeliveryFailed` or `Queued` report while the device is behind | none; the next uplink carries the push | `pushed_version = 0` (pending) |
| `HELLO` that does not match | `STATUS UNCLAIMED 1`, signed with the presented key, at most one an hour | `notice_at` |
| Other frame from an unclaimed pigeon or an unpinned line, or a billable frame while paused | `STATUS UNCLAIMED 0` or `STATUS PAUSED 3600`, at most one an hour | `notice_at` |
| Converged device sending telemetry | none | none |

**Sending.** The tail builds the frame (`shadow_frame` or `status_frame`, section 6.6), appends its
tag with `sign_frame` (7.1), and calls `thingspace::send(&env, &pigeon_id, &imei, &frame)` (section
6.8). `pushed_*` is written before the send, in the synchronous upsert, so a second trigger during
the send sees the push as outstanding. If a `SHADOW` send answers 503 (not configured, latched,
unreachable), the tail runs
`UPDATE pigeon_nidd SET pushed_version = 0, pushed_at = 0 WHERE pushed_version = ?1 AND pushed_at = ?2`
with the values it wrote, so a newer push planned meanwhile is left alone; that makes the push due
again on the next uplink. An uplink that plans a `SHADOW` but cannot read the connector to address
it zeroes `pushed_*` in its own row write instead, with the same effect. A 502 (ThingSpace
refused the message itself; a 408 or 429 is a 503, 8.3) is logged and left as sent: the same
bytes would fail the same way, so the next uplink past the 30 s window tries again, a ThingSpace
call and no radio access, until the config changes. A failed `STATUS` is logged and dropped.

**Missed deliveries.** A `DeliveryFailed` report, or a `Queued` one (the network could not reach
the device, and nothing it buffered was seen delivered later), is handed to the pigeon's DO at
`/pigeon/nidd/missed`. `nidd_missed` marks the owed push pending with `NiddRow::mark_pending`:
`pushed_version` becomes 0 and `pushed_at` stays, so the next uplink carries it even inside the
30 s window while the missed push still holds back an unsolicited one. It answers `pending`, or
`converged` when nothing is owed, and the callback's line reads `outcome=pending` or
`outcome=logged`. A report does not name the frame it concerns, so a missed notice marks the push
too, at the cost of one early re-send. `Delivered` reports and configuration results are only
logged: `Delivered` means the network took the frame, not that the device has it (the B7 retest saw
it up to 2.8 s before the device did), and the device's own reported version is the convergence
signal. A lost report costs nothing the 30 s lapse does not recover.

**What this bounds.** At most one unsolicited push per 15 minutes per pigeon, so an operator saving
repeatedly does not flood a device; no downlink at all while converged; a push that missed, never
left or lapsed rides the device's next uplink; and a device that cannot apply a config draws at
most one `SHADOW` per uplink, inside the connection that uplink opened, so no radio access is
added.

### 6.5 `Pigeons`: connector arms

- `create` (`:774-916`), `Nidd` arm: `capsules::imei_is_valid` or 400 (defence in depth behind the
  gateway); confirm this DO's own id equals `PIGEONS.id_from_name(nidd_object_name(imei))`, a
  mismatch being a gateway bug (500); a `pigeons` row already present is 409 "Conflict: a pigeon
  with this IMEI already exists" (today a duplicate id surfaces as the insert's 500,
  `:867-870`); mint the claim key with `mint_device_psk`; build `NiddConfig { endpoint:
  build_nidd_endpoint(), token: device_token, imei, claim_key: Some(key) }`. The token is minted as
  for every variant (`:790-797`). `build_nidd_endpoint()` sits beside `build_mqtt_endpoint`
  (`:127`): `String::with_capacity(7 + NIDD_APN.len())`, `push_str("nidd://")`,
  `push_str(NIDD_APN)`; no host var.
- `refresh_token` (`:918-1041`), `Nidd` arm: keep `imei` and `endpoint` from the stored row, never
  rebuild them; new token, new claim key; then
  `UPDATE pigeon_nidd SET claimed_at = NULL, line_id = NULL`. Every existing arm rebuilds from
  scratch (`:949-988`); this one must not, or a refresh would unbind the device.
- `strip_secrets` (`:628-651`), `Nidd` arm: blank `token`, `claim_key: None`, keep `endpoint` and
  `imei`. The IMEI is device metadata the dashboard shows, covered by the privacy policy's
  "connector settings" (`docs/legal/privacy.md:21`). Its connector half becomes a plain function,
  `connector_without_secrets(&Connector) -> Connector` in `dovecote/src/helpers/pigeons.rs`, which
  `strip_secrets` and the two Postgres writers call, the writers for `Nidd` only (section 9).
- `get_device_psk_internal` (`:1058`): unchanged; `psk()` is `None`, so 404.
- **Rollback guard, shipped first and alone.** `refresh_token` reads the connector through
  `From<PigeonRow>`, whose `unwrap_or_default()` (`capsules/src/lib.rs:241`) turns an unknown
  variant into an empty `Https` connector, which the refresh would then write back, destroying the
  IMEI. Task 0.4 makes `refresh_token` parse the stored `connector` text itself and answer 500
  "stored connector unreadable" when it fails. Deployed before the Nidd release, it makes a later
  rollback land on code that refuses rather than rewrites.

### 6.6 `dovecote/src/helpers/nidd.rs`

One new file for the codec and the pure functions, all unit-tested on the host target:

- `CallbackAuth { password: Option<String>, request_id: Option<String> }`, the second with
  `#[serde(rename = "requestId")]`, so a body that authenticates but fails `NiddCallback` can still
  be logged by its request id; and `NiddCallback`: dovecote's own serde model of
  ThingSpace's body (`request_id`, top-level `device_ids: Vec<CarrierId>` defaulting empty,
  optional `status` and `callback_count`, and `nidd_response`, an enum of the three documented
  variants `niddMONotificationResponse`, `niddMTDeliveryResponse`, `niddConfigResponse`, each with
  only the fields dovecote reads). `CarrierId { id: Option<String>, kind: String }`, because
  Verizon's inner lists carry entries without an id.
- `imei_key(raw) -> Option<String>`: digits only; 15 digits returned if the Luhn check passes;
  14 digits gain their Luhn digit; 16 digits (an IMEISV) keep the first 14 and gain the Luhn digit;
  anything else is `None`. Which form Verizon reports is bench check B5.
- `callback_imei(&NiddCallback) -> Option<String>`: the first entry whose `kind` equals `imei`
  case-insensitively, from `niddMONotificationResponse.deviceIds` first and the top-level list
  second, through `imei_key`. Verizon's own examples spell the kind `IMEI`, `imei` and `Imei`.
- `callback_line(&NiddCallback) -> Option<String>`: the `ICCID` entry of
  `niddMONotificationResponse.deviceIds`, else its `IMSI` entry, `kind` compared
  case-insensitively; the line pin of 4.4. Never logged.
- `nidd_object_name(imei) -> String`: `String::with_capacity(25)`, `push_str("nidd:imei:")`,
  `push_str(imei)`.
- `parse_error_line(context, &serde_json::Error) -> String`: the context, the error's
  `classify()` category and its `column()`, built with `with_capacity`, never the error's
  `Display`, which quotes the offending value (5.1, step 7). Every NIDD parse failure logs through
  it.
- Frame constants (`TELEMETRY` 0x01, `SHADOW_REPORT` 0x02, `HELLO` 0x04, `SHADOW` 0x81, `STATUS`
  0x82, status codes 0, 1, 2, `NIDD_TAG_CHARS` 16), `is_billable(frame_type)`,
  `shadow_frame(&PigeonShadow) -> Vec<u8>` and `status_frame(code, arg) -> Vec<u8>` (unsigned,
  their text header written by one `push_header`), `shadow_config_cap(target, current)` (6.4) and
  `hello_key(body)`, the claim key a `HELLO`'s 32 hex characters present.
  `sign_frame(key: &[u8; 16], frame: Vec<u8>) -> Result<Vec<u8>, String>` appends the first 8 bytes
  of `hmac_sha256(key, &frame)` (`dovecote/src/helpers/stripe_webhook.rs:284`) as hex; it is async
  and runs on WebCrypto, so the host-target tests sign with their own HMAC (checked against RFC
  4231) through the same `push_tag`, and B7's probe checks real tags against the key in its
  `prj.local.conf`.
- `dedupe_key(request_id, frame) -> String`: `String::with_capacity(request_id.len() + 17)`, the
  id, `:`, and 16 hex characters pushed from a lookup table rather than `format!`.
- `NiddRow` (the table's row), `NiddRow::remember(key)` trimming `seen` to 64,
  `NiddRow::mark_pending`, `notice_due(&row, now)` (an hour since `notice_at`), `shadow_push_due`,
  `shadow_reply_due` and `delivery_missed` (6.4).
- `NiddInFlight`, a field of `Pigeons`: the telemetry uplinks being stored, by key, each with the
  `oneshot` senders of the retries waiting on it (`claim`, `wait`, `settle`; 6.3 steps 2 and 7).

The frame layout is a contract with C code in `~/pigeon`, so its constants stay in dovecote rather
than capsules, the reason the WebSocket frame cap sits in dovecote (`objects/ws.rs:4-11`), and
`docs/api.md` is the authority both sides follow.

### 6.7 Refactors of two existing handlers

- `handle_ws_telemetry` (`:1843-1940`) returns `()` and only logs a failed enqueue (`:1903-1904`),
  so reusing it would acknowledge a callback whose history, alerts and billing were lost. Its body
  becomes `ingest_telemetry(pigeons, body: TelemetryReportBody) -> TelemetryOutcome` with
  `Stored`, `Rejected(String)` and `Failed`. The WebSocket handler keeps its behaviour by matching
  on the outcome and logging; the NIDD path answers 503 on `Failed`.
- `handle_ws_shadow_report` (`:1797-1828`) awaits billing and the Postgres sync inline. Its tail
  half becomes `sync_shadow_report(env, pigeon_id, shadow)`, which the WebSocket path still awaits
  inline and the NIDD path runs after the response.

### 6.8 `ThingSpaceSession`, a new singleton class (`dovecote/src/objects/thingspace.rs`)

One instance per environment, `THINGSPACE.id_from_name("session")`, and the only code that holds or
uses ThingSpace tokens. One file holds both halves: the `#[durable_object]` struct
`ThingSpaceSession { state, env, login: futures::lock::Mutex<()> }` (`futures` is already a dovecote
dependency) and the client function `pub async fn send(env: &Env, pigeon_id: &str, imei: &str,
frame: &[u8]) -> SendOutcome` that pigeon DOs call. One internal path, `POST /send`, body
`{"pigeon_id", "imei", "frame_b64", "max_delivery_secs"}` (the pigeon id only for its log lines),
answering `200 {"request_id"}`, `502 {"reason": "<Verizon errorCode>"}` when ThingSpace refused the
message, or `503 {"reason": "not_configured" | "latched" | "unreachable"}`; `not_configured` also
whenever this environment's `THINGSPACE_CALLBACK_ALLOWED_IPS` is empty (8.2). It stores two keys and
nothing per pigeon, so pigeon `delete` has nothing to clear in it. Section 8 has its token logic.

It exists for one measured reason: Verizon locks the account's contact record after five
consecutive failed logins [LOGIN], and per-isolate caches cannot bound failures across isolates.
Throughput is not a concern: downlinks are paced by shadow writes, reports and boots.

## 7. The wire envelope

### 7.1 Layout

A NIDD frame is the bytes the device hands `send()` on its raw socket, and the bytes ThingSpace's
base64 `message` field decodes to on either API leg. Byte 0 is the type; the rest is the body. No
length field (the carrier delivers whole messages), no version byte (a new shape is a new type),
no sequence number (replies name the shadow version they confirm, and resends are recognised by
`requestId`, unless B5 shows uplink request ids repeat, 6.3 step 2).

**No frame holds a NUL byte.** ThingSpace's send API refuses any message holding two adjacent NUL
bytes (400 `NiddService.INPUT_INVALID.Message.AdjacentNullCharacters`, found by B7 and
documented nowhere), and a binary header puts `00 00` in every version below 65536. So a
platform frame's numbers travel as text: after the type byte, a header of two unsigned decimal
integers with one space between them and a newline after (`8 7\n`), at most 22 bytes. A version
is 0 to 2147483647; a negative one, which only a misbehaving device can report, is sent as 0.
The JSON a frame carries is serde_json's own serialization, which escapes every control
character, and the tag is hex. Device frames follow the same rule: `HELLO` carries the claim key
as hex, and JSON holds no NUL.

Every platform frame ends in a 16-character tag: the first 8 bytes of HMAC-SHA256 over every byte
before it, keyed by the pigeon's 16-byte claim key (4.4), written as lowercase hex. The device
drops a platform frame whose tag does not verify against the key it was built with. Device frames
carry no tag (D12).

| Byte 0 | Name | Direction | Body |
|---|---|---|---|
| `0x01` | `TELEMETRY` | device to platform | UTF-8 JSON, exactly a `POST /device/pigeons/:pigeon_id/telemetry` body: the flat map or `{"reports":[...]}` |
| `0x02` | `SHADOW_REPORT` | device to platform | UTF-8 JSON, exactly a `POST /device/pigeons/:pigeon_id/shadow` body |
| `0x03` | reserved | device to platform | Log upload, not in v1 |
| `0x04` | `HELLO` | device to platform | The claim key as 32 lowercase hex characters |
| `0x81` | `SHADOW` | platform to device | Header `<target_version> <current_version>\n`, then `target_config` as raw UTF-8 JSON up to the tag, then the tag |
| `0x82` | `STATUS` | platform to device | Header `<code> <arg>\n`, then the tag |
| `0x83` | reserved | platform to device | Application data (section 14.4), not in v1 |

`STATUS` codes, written in the header in decimal: `0` `STORED` (arg: the `current_version` just
stored), `1` `PAUSED` (arg: seconds to hold billable sends; the device caps it at 86400), `2`
`UNCLAIMED` (arg: 1 when it answers a `HELLO` whose key did not match, and the device stops
billable sends until its next boot; 0 otherwise, and the device sends `HELLO` again, at most
hourly). An argument is 0 to 4294967295. Reserved type bytes: `0x00`, `0x7f`, `0x80`, `0xff`. New
device frames take `0x05` upward, new platform frames `0x84` upward. An unknown type is logged and
dropped by dovecote and ignored by the device, the forward-compatible rule the WebSocket client
already follows.

### 7.2 Exact bytes

Computed by `nidd/synth/frames.py` in the job directory, which also parses every JSON body back;
the platform frames are pinned by golden tests in `helpers/nidd.rs`, which sign on the host with an
HMAC checked against RFC 4231. Byte 0 is shown first; the ASCII column is the text the body
carries.

**Frame 1: `TELEMETRY`, a batch of three readings taken five minutes apart, sent at one wake.**
294 bytes, 392 characters of base64.

```text
0000  01 7b 22 72 65 70 6f 72 74 73 22 3a 5b 7b 22 61  .{"reports":[{"a
0010  67 65 5f 73 65 63 73 22 3a 36 30 30 2c 22 6d 65  ge_secs":600,"me
0020  74 72 69 63 73 22 3a 7b 22 75 70 74 69 6d 65 5f  trics":{"uptime_
0030  73 22 3a 22 38 35 38 30 30 22 2c 22 72 73 72 70  s":"85800","rsrp
0040  22 3a 22 2d 39 37 22 2c 22 62 61 74 74 5f 6d 76  ":"-97","batt_mv
0050  22 3a 22 33 37 31 32 22 2c 22 74 65 6d 70 5f 63  ":"3712","temp_c
0060  22 3a 22 32 31 2e 35 22 7d 7d 2c 7b 22 61 67 65  ":"21.5"}},{"age
0070  5f 73 65 63 73 22 3a 33 30 30 2c 22 6d 65 74 72  _secs":300,"metr
0080  69 63 73 22 3a 7b 22 75 70 74 69 6d 65 5f 73 22  ics":{"uptime_s"
0090  3a 22 38 36 31 30 30 22 2c 22 72 73 72 70 22 3a  :"86100","rsrp":
00a0  22 2d 39 38 22 2c 22 62 61 74 74 5f 6d 76 22 3a  "-98","batt_mv":
00b0  22 33 37 31 31 22 2c 22 74 65 6d 70 5f 63 22 3a  "3711","temp_c":
00c0  22 32 31 2e 34 22 7d 7d 2c 7b 22 61 67 65 5f 73  "21.4"}},{"age_s
00d0  65 63 73 22 3a 30 2c 22 6d 65 74 72 69 63 73 22  ecs":0,"metrics"
00e0  3a 7b 22 75 70 74 69 6d 65 5f 73 22 3a 22 38 36  :{"uptime_s":"86
00f0  34 30 30 22 2c 22 72 73 72 70 22 3a 22 2d 39 37  400","rsrp":"-97
0100  22 2c 22 62 61 74 74 5f 6d 76 22 3a 22 33 37 31  ","batt_mv":"371
0110  31 22 2c 22 74 65 6d 70 5f 63 22 3a 22 32 31 2e  1","temp_c":"21.
0120  34 22 7d 7d 5d 7d                                4"}}]}
```

**Frame 2: `SHADOW_REPORT`, version 7 applied.** 78 bytes, base64
`AnsiY3VycmVudF9jb25maWciOnsidGVsZW1ldHJ5X2ludGVydmFsIjo5MDAsImxvZyI6ZmFsc2V9LCJjdXJyZW50X3ZlcnNpb24iOjd9`.

```text
0000  02 7b 22 63 75 72 72 65 6e 74 5f 63 6f 6e 66 69  .{"current_confi
0010  67 22 3a 7b 22 74 65 6c 65 6d 65 74 72 79 5f 69  g":{"telemetry_i
0020  6e 74 65 72 76 61 6c 22 3a 39 30 30 2c 22 6c 6f  nterval":900,"lo
0030  67 22 3a 66 61 6c 73 65 7d 2c 22 63 75 72 72 65  g":false},"curre
0040  6e 74 5f 76 65 72 73 69 6f 6e 22 3a 37 7d        nt_version":7}
```

**Frame 3: `SHADOW`, the push after a dashboard write.** `target_version` 8, `current_version` 7
(the device is one behind). 58 bytes, the last 16 the tag for the fixture key of 16 zero bytes;
base64 `gTggNwp7InRlbGVtZXRyeV9pbnRlcnZhbCI6OTAwLCJsb2ciOnRydWV9NzNjZThkZTdkNDM4ZjU0MQ==`.

```text
0000  81 38 20 37 0a 7b 22 74 65 6c 65 6d 65 74 72 79  .8 7.{"telemetry
0010  5f 69 6e 74 65 72 76 61 6c 22 3a 39 30 30 2c 22  _interval":900,"
0020  6c 6f 67 22 3a 74 72 75 65 7d 37 33 63 65 38 64  log":true}73ce8d
0030  65 37 64 34 33 38 66 35 34 31                    e7d438f541
```

The small frames, whole, tagged with the same fixture key:

| Frame | Bytes | Layout | Base64 |
|---|---|---|---|
| `STATUS STORED 7` | 21 | `82 30 20 37 0a` (`0 7`, newline), then `db37ac5430ae1394` | `gjAgNwpkYjM3YWM1NDMwYWUxMzk0` |
| `STATUS PAUSED 3600` | 24 | `82 31 20 33 36 30 30 0a` (`1 3600`, newline), then `1c273dd5d282365a` | `gjEgMzYwMAoxYzI3M2RkNWQyODIzNjVh` |
| `STATUS UNCLAIMED 0` | 21 | `82 32 20 30 0a` (`2 0`, newline), then `edba557d1f9f3445` | `gjIgMAplZGJhNTU3ZDFmOWYzNDQ1` |
| `STATUS UNCLAIMED 1` | 21 | `82 32 20 31 0a` (`2 1`, newline), then `6e3162e1b9f30649` | `gjIgMQo2ZTMxNjJlMWI5ZjMwNjQ5` |
| `HELLO` | 33 | `04`, then the key's 32 hex characters | 44 characters |

No claim key value appears in this document; the test fixture is 16 zero bytes.

### 7.3 Why this envelope

- **Uplink bodies are the HTTPS bodies.** The library already builds them, dovecote already
  parses them (`TelemetryReportBody`, `PigeonShadowReportRequest`), and their caps already apply.
  One type byte replaces the path an HTTP request gets for free.
- **The downlink shadow is target-only, with a text header.** The device authored `current_config`
  and has no use for `updated_at`; dropping both gives a `target_config` budget of at least 1319
  bytes, which the dashboard can count before saving. The header is text, not little-endian
  integers, because ThingSpace refuses two adjacent NUL bytes (7.1). The alternative, reusing the
  WebSocket `shadow_update` frame, carries both configs as escaped JSON strings, so a PUT could be
  refused because of the size of the *device's* current config, and the budget would vary with
  escaping. The same firmware-target shadow is 179 bytes here at single-digit versions, tag
  included, against 319 as the HTTPS route returns it.
- **Platform frames carry a tag; device frames do not.** The device obeys what it receives, so a
  frame it cannot authenticate could wedge it (4.4). The platform's uplink trust rests on the
  callback gates and the line pin instead; D12 names an uplink MAC as the alternative.
- **Replies need frames the WebSocket vocabulary lacks.** `HELLO` and `STATUS` exist because the
  claim and the report confirmation have no WebSocket equivalent; adding them to `WsInboundFrame`
  would widen the WebSocket protocol for a transport that does not use it.
- **Compactness.** The type byte replaces the WebSocket frame's `{"type":"telemetry",...}` wrapper
  (179 bytes against 149 for the same ten keys). A CBOR body would save about a quarter more on
  telemetry, but the workspace has no CBOR crate and, under the 4-an-hour radio contract, wakes
  cost far more than bytes.

### 7.4 Ordering, repeats and staleness

- Frames can arrive out of order: each is its own callback. Readings carry their own `age_secs`,
  resolved against receipt and clamped to 24 hours. ThingSpace's one retry comes about a second
  after a refusal (B6), so a reading stored on it is stamped at its arrival and is not aged.
- Once a frame's tag verifies, the device keeps the shadow with the highest `target_version` and
  ignores any `SHADOW` that is not newer, except to read its `current_version` as a confirmation. A
  repeated or late `SHADOW` therefore settles on the newest. Repeats are expected: any uplink from
  a device that is behind draws the owed `SHADOW` once the last one is 30 s old or was reported
  missed, and a push ThingSpace reported `Queued` may still land after its reply.
- A report is confirmed by `STATUS STORED` or by a `SHADOW` whose `current_version` is at least the
  version reported. Re-sending a report is harmless: the write is the same, and an identical report
  is not billed.
- A `SHADOW` whose `current_version` is below what the device applied tells the device its report
  was lost; it reports again, unless a report of that version still awaits its reply, since
  telemetry sent before the report can draw the owed `SHADOW` before the report is stored, or the
  same `SHADOW` brings a newer target, since the application reports that one instead and the
  older report could otherwise be stored after it.

### 7.5 Budgets against one frame

Uplink frames are measured against the 1273 bytes the bench modem accepts (B4: mfw 1.3.7 on the
nRF9160 accepted 1273 and refused 1283 in `send()` with `EINVAL`, before any radio access;
1274 to 1282 untested), which the device library caps a frame at; platform frames against the
1358 bytes ThingSpace sends. dovecote itself accepts any frame up to 1358.

| Frame | Size | Fits |
|---|---|---|
| `TELEMETRY`, flat, the library's worst case at 7 keys | about 1158 | yes |
| `TELEMETRY`, flat, the library's worst case at 8 keys (`pigeon/src/pigeon_internal.h:39-66`; `PIGEON_TELEMETRY_BODY_MAX` is 1323 with its NUL) | 1 + 1322 = 1323 | no: over 1273, refused at build time (14.2), since the core hands the transport one pre-built body and never splits it to fit |
| `TELEMETRY`, flat, 9 keys at the worst case | 1 + 1487 = 1488 | no |
| `TELEMETRY`, batch, ten realistic keys, 1 / 3 / 4 / 6 / 7 readings | 188 / 540 / 716 / 1070 / 1247 | yes |
| `TELEMETRY`, batch, ten realistic keys, 8 readings | 1424 | no |
| `SHADOW_REPORT`, the library's largest report body (`pigeon/src/pigeon_https.c:723-732`) | 1 + 384 | yes |
| `HELLO` | 1 + 32 = 33 | yes |
| `SHADOW`, `target_config` at the worst-case cap, ten-digit versions | 1 + 22 + 1319 + 16 = 1358 | yes, by construction |
| `SHADOW`, `target_config` at its cap, single-digit versions | 1 + 4 + 1337 + 16 = 1358 | yes, by construction |
| `SHADOW` carrying a firmware target (version, size, sha256), single-digit versions | 179 | yes |
| `STATUS` | 21 to 30 | yes |
| Callback body around a 1273-byte uplink, measured (B5: 564 bytes plus the frame's base64) | 2264 | under the 8 KiB route cap |

## 8. ThingSpace credentials, cache and session

### 8.1 Secrets

Names only. Values are set with `wrangler secret put NAME --env <env>` (dev: the gitignored
`dovecote/.dev.vars`), never appear in `wrangler.toml`, a log, a commit or chat, and are named in
the comment block above `[vars]` the way `COAP_SERVICE_SECRET` is (`dovecote/wrangler.toml:85-91`).

| Secret | What it is | Unset means |
|---|---|---|
| `THINGSPACE_PUBLIC_KEY`, `THINGSPACE_PRIVATE_KEY` | OAuth client key pair for `POST /api/ts/v1/oauth2/token` [CRED]; the SDK's `get_access_token(public_key, private_key)` | downlinks answer 503 `not_configured` |
| `THINGSPACE_UWS_USERNAME`, `THINGSPACE_UWS_PASSWORD` | UWS login for `POST /api/m2m/v1/session/login` [LOGIN] | same |
| `THINGSPACE_ACCOUNT_NAME` | The billing account (`<10 digits>-<5 digits>` [SEND]); also this environment's NIDD switch | callback 503, Nidd create 403, no downlinks |
| `THINGSPACE_CALLBACK_PASSWORD` | 40 random hex characters (160 bits), its own value per environment, never the UWS password [REG] | callback 503 |
| `THINGSPACE_CALLBACK_PASSWORD_PREVIOUS` | The listener password the last rotation replaced, set by the runbook's step 3 and deleted by the owner 24 hours later (8.4) | only the current password is accepted |

The account name is not a credential, but it identifies a Verizon billing account and the
repository is public, so it stays out of `wrangler.toml`.

### 8.2 Vars and bindings per environment

| Name | Kind | Production `[vars]` | `[env.staging.vars]` | `[env.dev.vars]` |
|---|---|---|---|---|
| `THINGSPACE_CALLBACK_ALLOWED_IPS` | var | `""` until cutover, then the eight addresses | the eight addresses during bring-up, `""` after cutover | `"127.0.0.1,::1"` |
| `NIDD_ALLOWED_ORG_IDS` | var | `""` until cutover, then JES's own organization ids | JES's test organization's id | the local test organization's id |
| `THINGSPACE_LOGIN_EPOCH` | var | `"1"` | `"1"` | `"1"` |
| `THINGSPACE` | Durable Object binding | `ThingSpaceSession` | same | same |

The eight addresses [CB]: `137.117.33.109,168.62.173.153,3.87.163.45,3.91.119.203,54.197.62.209,35.165.205.14,54.200.43.232,34.216.81.234`.
Only the environment that holds the `NiddService` registration admits callbacks at all, because
Verizon allows one callback endpoint per service per account [CBBP]. It is also the only one that
sends. [SEND] says of this API "You must register the NiddService as a callback listener", and a
second environment able to send could push its own shadow, with a `target_version` the other never
set, to a physical device whose reports go elsewhere; with API credentials it could also read the
other's listener password back [LIST]. So one environment is NIDD-configured at a time: `send`
answers 503 `not_configured` whenever `THINGSPACE_CALLBACK_ALLOWED_IPS` is empty, at cutover
staging loses its account name and API secrets, not only its allowlist (8.5), and dev never holds
ThingSpace API secrets.

`NIDD_ALLOWED_ORG_IDS` is the create gate of 4.6: comma-separated organization ids, empty denying
every Nidd create. `THINGSPACE_LOGIN_EPOCH` is folded into the login latch's fingerprint (8.3);
bumping it is how a false latch is cleared without rotating a Verizon credential.

`dovecote/wrangler.toml` edits:

- The three vars in all three blocks (`[vars]` at `:102`, `[env.staging.vars]` at `:338`,
  `[env.dev.vars]` at `:446`), under a comment block modelled on `COAP_SERVICE_ALLOWED_IPS`'s
  (`:92-101`), saying why the allowlist is its own var, what the org list and the epoch do, and
  naming the six secrets.
- The binding `{ name = "THINGSPACE", class_name = "ThingSpaceSession" }` beside `PIGEONS` in all
  three `durable_objects` blocks (`:262`, `:376`, `:482`).
- One migration after `v1` (`:289-291`):

```toml
[[migrations]]
tag = "v2"
new_sqlite_classes = ["ThingSpaceSession"]
```

No KV namespace, no new queue, no new cron trigger.

### 8.3 The token cache and single flight

What Verizon imposes: every M2M call carries `Authorization: Bearer <OAuth token>` and
`VZ-M2M-Token: <session token>` [REG][SEND]; the OAuth token lives one hour and a re-request inside
it returns the same token [CRED]; the session expires after 20 idle minutes [LOGIN]; five
consecutive failed logins lock the contact record until Verizon support unlocks it [LOGIN].

`ThingSpaceSession` keeps two keys in its own storage (the Durable Object key-value API, which a
SQLite-backed class also serves), so an eviction does not force a login. Tokens never leave the
object and are never logged.

- `tokens`: `{access, access_expires_at, session, session_used_at}`.
- `login_failures`: `{salt, fingerprint, count}`, present only after a login that returned no
  session token. The object is latched while `count` is 2 or more and the fingerprint matches.

`ensure_tokens`, in order:

1. Read the four API secrets and the account name. Any missing or blank: `not_configured`, no
   request.
2. Read `tokens`. The access token is fresh while `now < access_expires_at - 300`; the session
   while `now - session_used_at < 900`. Both fresh: done.
3. Take `self.login.lock().await`, then read `tokens` again: a request that waited on the mutex
   usually finds another has logged in. This is the single flight; a
   `futures::lock::Mutex` does it without `block_concurrency_while`, which resets the object when
   its future returns an error (worker 0.8.6 `src/durable.rs:300-310`; "If the callback throws an
   exception, the object will be terminated and reset",
   https://developers.cloudflare.com/durable-objects/api/state/, read 2026-09-24).
4. Read `login_failures`. Its fingerprint is `SHA-256(salt, login_epoch, 0x00, public_key, 0x00,
   private_key, 0x00, uws_username, 0x00, uws_password)`, `login_epoch` being the plain
   `THINGSPACE_LOGIN_EPOCH` var. A fingerprint that does not match is discarded: the credentials or
   the epoch changed. A match with `count >= 2`: `latched`, no request.
5. Stale access token: `get_access_token`; `access_expires_at = now + expires_in`. Stale session:
   `get_session_token`. Each call is raced against a 10-second `Delay`, the pattern at
   `dovecote/src/helpers/turnstile.rs:118-132`; the SDK takes no abort signal, so a timed-out
   request is abandoned rather than aborted.
6. Classify each attempt (pure functions, `classify_login` and `classify_send`, so the unit tests
   reach them):
   - Success deletes `login_failures` and writes `tokens`.
   - `/session/login` answering 401 with a gateway `fault` body (`900901` Invalid Credentials,
     `900902` Missing Credentials) means the bearer was stale or wrong and says nothing about the
     UWS password: [ERR] gives that shape for a bad bearer on any M2M call, and [CRED]'s "any
     further token requests during that time will return the same token" means a token first
     issued to another holder of the key pair arrives with less than its hour left. Drop the access
     token, fetch a new one and retry the login once inside the same mutex hold.
   - A refusal that names the credentials latches at once: an M2M `errorCode` body from
     `/session/login`, or a 400 or 401 from the OAuth endpoint itself.
   - Every other attempt that returns no `sessionToken` counts: any other status, a gateway fault
     on the retry, a 429, a 5xx, a timeout. A timed-out login is abandoned rather than aborted, so
     it may still have reached Verizon and counted toward "5 consecutive failed log in attempts"
     [LOGIN]. Only a failure before the request left the Worker is exempt. The second consecutive
     one latches; below that the answer is `unreachable`.
   - Each counted failure increments `count` in `login_failures`, written with 16 fresh random
     bytes of salt (`getrandom`, already a dependency) when its fingerprint is new. Latching sets
     `count` to 2, logs `thingspace_login outcome=latched status=<n>`, sends one `send_ops_email`
     (`dovecote/src/helpers/ops_probe.rs:32`, which logs instead of sending where
     `OPS_ALERT_EMAIL` is unset), and answers `latched`.

So an environment spends at most two of Verizon's five strikes per credential set and epoch, and
only one environment is configured at a time (8.2). A changed secret value or a bumped
`THINGSPACE_LOGIN_EPOCH` changes the fingerprint and re-arms; putting a secret again with the same
value does not, since the digest is of the values, so the epoch is how a false latch is cleared
without rotating a Verizon credential. The stored value is a salted digest of the secrets and the
epoch, never a secret.

`send`, in order: `ensure_tokens`; `send_nidd` with `NiddMessage { account_name, device_ids:
[DeviceID { id: imei, kind: "IMEI" }], maximum_delivery_time: 30, message: frame_b64 }`; on
200, parse `{"requestId"}`, set `session_used_at = now`, answer 200. A 401 fault drops the access
token; an M2M error code containing the `.SessionToken.` segment drops the session, since [ERR]
lists three (`REQUEST_FAILED.SessionToken.Expired`, `REQUEST_FAILED.SessionToken.Format` and
`INPUT_INVALID.SessionToken.Invalid`), the last being the likely answer once another login has
replaced the session; either way re-run `ensure_tokens` once and send once more, never looping. A
429 or 408 answers 503 `unreachable`, like a 5xx or a network error, so the push is due again at
once rather than after its delivery window; the Terms reserve "limits on the number or rate of
calls" [TOS]. Any other 4xx answers 502 with Verizon's `errorCode`. Before any of this, `send`
answers 503 `not_configured` when `THINGSPACE_CALLBACK_ALLOWED_IPS` is empty (8.2). Logged per
call: path, HTTP status, Verizon error code, latency, pigeon id. Never a token, a body, a header
value, a frame or an IMEI.

[ERR] is https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Synchronous_Error_Messages/
(read by the readers on 2026-09-24).

### 8.4 Rotation

- **UWS password or API key pair** (Verizon recommends a new UWS password every three months
  [CRED]): put the new secret in each environment. The latch fingerprint changes, the cached tokens
  keep working until they lapse, and the next login uses the new values.
- **Callback password**: the script of 8.5, run again. B6 measured what a rotation costs without
  a grace window: after re-registering, ThingSpace went on sending a password other than the
  registered one, interleaved with the new one, for 13 min 55 s the first time and 9 min 27 s
  the second, and since a refused callback is retried only once, a second later, every one of
  those uplinks was lost (B10 lost five of twelve that way). Which password they carried was not
  logged; the replaced one is the likely value. So step 3 first copies the password registered
  now (read from the listener list, never printed) into `THINGSPACE_CALLBACK_PASSWORD_PREVIOUS`,
  then mints the new one, and the route accepts both, each compared in constant time, logging
  `nidd_cb password=previous` whenever the old one arrives, which settles the question on the
  next rotation. The owner deletes the previous secret 24 hours later, which ends the window; a
  leaked old password is live that long and no longer. Step 3 copies the password only from a
  registration naming one of our own two callback URLs, so a first registration never admits a
  password set by whatever held `NiddService` before. An uplink sent between deregistering and
  registering (steps 4 and 5) is lost outright (B6), so rotate at a quiet time.
- **Suspected compromise of the account credentials**: rotate the UWS password and key pair in the
  ThingSpace portal first (anyone holding them can read the listener password back [LIST]), then
  the callback password.
- **Suspected compromise of the listener password**, which includes any compromise of the API
  credentials: rotate the callback password. Until then anyone able to post from one of the eight
  addresses can inject uplink into a claimed pigeon whose IMEI and ICCID they know (4.2, D12).
  Downlink stays safe: the device verifies every platform frame against its claim key.

### 8.5 Registering the listener (owner runbook, once per environment)

Registering displaces whatever holds `NiddService` on the account today (the 2023 middleware
registered one, and the SDK's example worker routes one to `/vzw/nidd`,
`thingspace-sdk-rust/examples/cf-worker/wasm-serv/src/lib.rs:30`), so it is the owner's action,
decision D4, and it waits on task 0.5. The commands live in `docs/infra/thingspace-nidd.md` as one
script, which is the authority; this section keeps its steps and the reasons for them. It is run
with `bash`, never pasted into an interactive shell, where `set -e` or an `exit` would close the
shell. It reads every value from exported environment variables, passes secrets to `curl` and `jq`
on stdin or through `env`, never on a command line, and prints none. curl's config syntax treats
`"` and `\` inside a quoted value as special, so a secret containing either needs escaping first.

1. Log in: OAuth, then the session. A failure here counts toward Verizon's five-strike lockout and
   the Worker's latch does not see it, so the script stops at the first failure.
2. List the account's listeners. The response carries each listener's password in clear [LIST],
   so only names and URLs are printed, userinfo redacted; the password of a `NiddService`
   registration naming one of our own two callback URLs is kept, unprinted, for step 3.
3. Put that password into `THINGSPACE_CALLBACK_PASSWORD_PREVIOUS` (8.4), then mint a fresh
   40-hex-character password straight into `THINGSPACE_CALLBACK_PASSWORD`. Reached only after
   steps 1 and 2 succeeded: from here every callback needs one of the two.
4. Log in afresh, then, if step 2 found `NiddService`, deregister it, as Verizon advises. A
   session was refused 401 96 seconds after it was issued (B6), between a successful
   deregistration and the registration, and left the account with no listener; so every step
   that changes the account runs on a fresh login.
5. Register this environment's URL for `NiddService`, username `pidgeiot`, with the new password.
   A 401 or 403 is retried once on a fresh login; anything else, or a second refusal, stops the
   script.
6. List again, on a fresh login, to confirm the URL. Whatever stopped the script, its last line
   says what holds `NiddService`: this URL, unchanged, or `NO LISTENER`, the state in which every
   uplink is lost until a run succeeds.

Four things the script relies on. `set -euo pipefail` makes a failed `curl -f` anywhere in a
pipeline stop the script, and the `nonempty` checks catch a 200 without a token, since `jq -r`
prints `null` and exits 0; without both, a failed login would still reach step 3 and replace the
Worker's callback password while step 5 fails, and every callback would then answer 403. [LIST],
[REG] and [DEREG] each say the request "must set the content-type to JSON", so steps 2 and 4 send
it too. [DEREG] lists `NiddService` among its valid service names (saved copy
`nidd/understand/vz/Deregister_Callback_Listener.txt:44`, and the live page, both read
2026-09-24). And an empty list from step 2 does not prove that nothing holds `NiddService`: the
list "only includes callback listeners that were registered through the Connectivity Management
API" [LIST], and "You cannot register a callback service through the REST API if the same callback
service has been registered through the SOAP API" [REG]. The 2023 middleware may have registered
through SOAP. If step 5 is refused that way, the owner has the SOAP registration removed through
the ThingSpace portal or Verizon support, and D11's drift check cannot see such a registration
either.

Step 2 is also the credential inventory. The account's API credentials are held by the dovecote
environment being registered (both deployed environments during bring-up, production alone after
cutover) and by the owner's machine: `secrets.env`, and the SDK repository's gitignored
`secrets.toml` while its live tests need them. Nothing else holds them: task 0.5 removes the SDK's
example worker deployment and confirms the 2023 middleware's host holds none. Anything found
holding them later is removed and the credentials rotated (8.4).

7. In the Cloudflare dashboard, a Configuration Rule turning Browser Integrity Check off for
   `/internal/thingspace/*` on both API hostnames, before the first callback. The zone's BIC has
   already refused non-browser clients on these hosts with "error code: 1010" (memory
   `reference_cloudflare_bic_python_ua.md`). ThingSpace's user agent, `Verizon's callback
   service`, met no challenge in 62 requests (B5), so the rule is insurance, not a fix.
8. Prove it with a real uplink (tier 2, B5): `wrangler tail --env staging` shows one `nidd_cb`
   line with `outcome=stored`.

The password is kept nowhere but the Worker secrets and the registration; a lost one is replaced
by running the script again, and 24 hours after any run that set
`THINGSPACE_CALLBACK_PASSWORD_PREVIOUS` the owner deletes it.

**Cutover to production**, each step on the owner's word: put the six production secrets
(`wrangler secret put` with no `--env`, production being the default environment); commit the
eight addresses into production's `[vars]` allowlist and JES's organization ids into its
`NIDD_ALLOWED_ORG_IDS`, and deploy; run the script with `env_flag=()` and the URL
`https://api.pidgeiot.com/internal/thingspace/nidd`, then steps 7 and 8; then delete staging's
`THINGSPACE_ACCOUNT_NAME` and its four API secrets (`wrangler secret delete <NAME> --env
staging`), set its allowlist back to `""`, and deploy staging. From then on staging has NIDD off
entirely: a Nidd create answers 403, a callback 503, a send 503 `not_configured`, and staging's
credentials can neither reach a production device nor read production's listener password. The
synthetic suite runs on dev after cutover, and on staging again only with a second ThingSpace
account.

## 9. Postgres

**No schema change, no migration file, no `ensure_*` helper.**

- The IMEI rides the existing `pigeons.connector` JSONB column (`infra/init-db.sql:55`), written
  by `insert_pigeon_pg_db` (`dovecote/src/helpers/pigeons.rs:342`) and, on refresh, by
  `update_pigeon_pg_db` with `Some(&connector)` (`dovecote/src/lib.rs:735`). The claim key does
  not: it authenticates every downlink for the device's life (4.4), so for a `Nidd` connector both
  writers store `connector_without_secrets` (6.5), with the token blank and `claim_key` null. No
  Postgres statement reads `connector` (the only ones that touch it are the writes at
  `dovecote/src/helpers/pigeons.rs:358` and `:457`), so nothing loses a value it used. The other
  variants' tokens and PSKs are still mirrored in clear today; stripping them too is a separate,
  pre-existing change.
- No reverse index: the uplink reaches its Durable Object by name (4.3), never through Postgres,
  so no NIDD path meets the Hyperdrive read-after-write trap (CLAUDE.md, Hyperdrive note).
- Claim, de-duplication and push state are the device's own and live in its Durable Object (6.1).
  Delivery reports are logged, and a missed one only marks that state (6.4).
- The only Postgres statement on the uplink path is the free-tier fuse at the gateway, the same
  `check_ingest_fuse` the HTTP telemetry route runs (`dovecote/src/lib.rs:981-988`), raced against
  one second there (5.1, step 6). Placing it at the gateway for a one-hop DO surface is the
  departure from `device_ingest_paused`'s rustdoc that 6.3 step 6 names; tasks 1.6 and 1.10 update
  that rustdoc and CLAUDE.md. Billing is
  unchanged: `count_billable_messages` (`dovecote/src/helpers/usage.rs:386`) stamps
  `last_billable_activity` for a Nidd pigeon exactly as for any other.

**One behavioural change, in `POST /flock/pigeons`, for `Nidd` only: a clean slate inside the
mirror insert.** A name-derived id is reused when an IMEI is deleted and registered again,
possibly by another organization. The delete route's cleanup is best-effort
(`dovecote/src/lib.rs:2004-2025`), and with `unique_id()` a leftover was unreachable; with
`id_from_name` the next create lands on the same id with a fresh ACL, and a leftover history row
would be read by pigeon id alone (`dovecote/src/helpers/telemetry.rs:183-186`), a leftover alert
definition would resolve its recipients through the new flock, and a leftover dictionary would be
served. So leftovers are made unreachable by construction:

- For a `Nidd` pigeon, `insert_pigeon_pg_db` runs `DELETE FROM pigeons WHERE id = $1` first,
  inside the transaction it already opens (`dovecote/src/helpers/pigeons.rs:345`), so the mirror
  row is written clean or not written at all. The `ON DELETE CASCADE` foreign keys
  (`infra/init-db.sql:104`, `:114`, `:131`, `:181`, `:211`) clear any leftover's shadow, ACL,
  telemetry history and alert rows in the same transaction. It is safe because the DO has just
  answered 201, so no live pigeon holds the id.
- If that transaction fails for a `Nidd` create, or any other step after the DO's 201 does (the
  organization's ACL grant, the parse of the DO's answer), the route undoes the create through the
  DO's own `/pigeon/delete`, as the new owner, and answers 503, so no pigeon ever exists over an
  id whose leftovers were not cleared. This is the one place the best-effort mirror rule gives
  way, and only for this reason; the operator retries.
- The log-dictionary GET (`dovecote/src/lib.rs:2823-2891`) answers 404 when the R2 object's
  `uploaded()` time predates the pigeon's `created_at`, which the DO's authorization check
  (`/pigeon/authz/check`, `dovecote/src/objects/pigeons.rs:375`) returns in a response header and
  `PigeonAccess` (`dovecote/src/helpers/pigeons.rs:14-16`) carries. A dictionary uploaded for an
  earlier pigeon under the same id is never served. The create route still deletes it
  best-effort, through `delete_log_dictionary(env, pigeon_id)` factored out of the delete route
  (`dovecote/src/lib.rs:2017-2025`), whose comment at `:2013-2016` is rewritten: a leftover object
  is no longer unreachable by the ACL alone once ids can repeat, and the `created_at` check is what
  keeps it so.

The residual gap: a queue message enqueued for the old pigeon in the seconds before its delete and
consumed after the recreate would land in the new pigeon's history.

**If decision D1 goes the other way** (operator assignment instead of the claim key), Postgres
gains the one table the failure-driven design specified, created by
`infra/migrations/2026-09-NN-nidd-devices.sql` in the house style of
`infra/migrations/2026-09-02-pigeon-suspension.sql` (`SET ROLE dovecote;` ... `RESET ROLE;`,
`dovecote_staging` on staging, idempotent), by `infra/init-db.sql` for fresh databases, and lazily
by an `ensure_nidd_tables(client)` beside `ensure_pigeons_board_column`
(`dovecote/src/helpers/pigeons.rs:319`), called from the create path:

```sql
CREATE TABLE IF NOT EXISTS nidd_devices (
  imei TEXT PRIMARY KEY CHECK (imei ~ '^[0-9]{15}$'),
  org_id UUID REFERENCES organizations(id) ON DELETE SET NULL,
  user_id UUID,
  pigeon_id TEXT UNIQUE,
  assigned_at TIMESTAMPTZ,
  bound_at TIMESTAMPTZ,
  CHECK (org_id IS NULL OR user_id IS NULL)
);
```

An operator assigns each line by SQL; create claims it with one conditional
`UPDATE nidd_devices SET pigeon_id = $2, bound_at = now() WHERE imei = $1 AND pigeon_id IS NULL
AND (org_id = $3 OR user_id = $4) RETURNING imei`, before the DO exists; delete releases it. The
routing would still be by name, so the uplink path stays free of Postgres either way.

**Related, pre-existing, outside NIDD:** `check_ingest_fuse` anchors its period on
`date_trunc('month', now())` (`dovecote/src/helpers/usage.rs:626`), so Hyperdrive never caches it
("queries that use functions designated as volatile or stable by PostgreSQL are not cached",
https://developers.cloudflare.com/hyperdrive/concepts/query-caching/, read 2026-09-24), despite the
doc comment above it (`:599-602`) relying on that cache. Passing the month start as a parameter
would make the fuse cacheable for every ingest surface. A separate, small change.

## 10. fancier

**Deploy order first: fancier ships the variant before the first Nidd pigeon exists.** One unknown
variant fails a whole 48-id chunk in `api::pigeons::list`
(`fancier/src/api/pigeons.rs:50`, `from_value::<Vec<Pigeon>>`), and the flock renders its
load-failure state, so a tab holding an older bundle breaks the moment a Nidd pigeon appears. The
same "fancier first" order the Terms gate uses.

- **Badge** (`components/connector_badge.rs:6-17`): `Connector::Nidd(_)` renders
  `badge badge-info badge-outline badge-sm` "NIDD". Never `badge-neutral`, white on white in this
  theme (CLAUDE.md, connection-state note).
- **Create form** (`CreatePigeonModal`, `views/pigeons.rs:685`): an option "NIDD (Verizon,
  NB-IoT)" in the `selected:`-bound select (`:807-826`), and an explicit `"Nidd"` arm in the
  string match (`:744-748`), whose `_ => Https` would otherwise provision HTTPS silently. While
  NIDD is selected, one IMEI input: `name: "imei"`, `inputmode: "numeric"`, `maxlength: 15`,
  checked with `capsules::imei_is_valid` before submit ("That IMEI's check digit is wrong"), with
  the hint "The 15 digits on the modem label, or from `AT+CGSN`. The device must be NB-IoT on a
  Verizon NIDD line." It must fit the modal's `max-w-xs` at 320 px (`:707`).
- **Create errors**: `api::pigeons::create` (`api/pigeons.rs:129`) returns
  `Result<(String, Connector), String>` carrying the server's message, the pattern `move_to_flock`
  already uses (`:152`), so a 403 (NIDD not enabled for this environment or this organization) or
  a 409 reads as what it is instead of "Failed to register
  pigeon. Please try again." (`views/pigeons.rs:761`). The result is built from the 201 body and
  never confirmed by a refetch (CLAUDE.md, Hyperdrive note).
- **The reveal** (`helpers/device_credentials.rs:32-95`), four rows for `Nidd`, in build order:
  - Device token, `CONFIG_PIGEON_TOKEN`: "Only for firmware downloads over a second, IP PDN. NIDD
    frames never carry it; a NIDD-only build leaves it empty."
  - Claim key, `CONFIG_PIGEON_NIDD_CLAIM_KEY`: "The device presents it once per boot to claim this
    pigeon, and checks every message from the platform with it. Refreshing the token mints a new
    one, and the device must be rebuilt with it."
  - IMEI, no symbol: "The modem this pigeon answers to. The carrier reports it; nothing to build
    in."
  - Device endpoint, `CONFIG_PIGEON_ENDPOINT`: "The APN of the Non-IP PDN, as a URI. The scheme
    has to match a NIDD build."

  `has_psk` (`:25`) becomes `has_write_once_secret`, true for a `Nidd` connector carrying a claim
  key, so the reveal keeps its shown-once warning. The per-variant tests (`:98-215`) gain a
  `nidd()` fixture and its stripped-read case. The Kconfig names in the `target` column become
  true once section 14 lands; until then the reveal says so.
- **`ConnectorInfo`** (`views/pigeon.rs:471`): a `Nidd` arm beside `:509`, `:540`, `:595` with
  rows Protocol "NIDD (Verizon ThingSpace, NB-IoT)", IMEI, APN (`capsules::NIDD_APN`) and Endpoint
  with the copy button the other arms have. The sentence "its token authenticates it on every
  device transport" (`:502`) becomes true for the IP variants only and says so; the refresh
  confirmation (`:832-839`) gains "For a NIDD pigeon the claim key rotates too, and the device is
  refused until it is rebuilt with the new one."
- **`EditShadowModal`** (`views/pigeon.rs:1446`): for a `Nidd` pigeon, a live "N of 1319 bytes"
  count of `serde_json::to_string` of the parsed `target_config` (the same serialization dovecote
  measures), save disabled above it. The server's 413 stays authoritative and is shown as sent.
- **Unchanged by design**: `UpdatePigeonModal` (`views/pigeon.rs:1749`) grows no connector field;
  the shell, firmware and re-push controls stay visible (the shell already answers 409 without a
  device WebSocket, and a firmware assignment works on a board with an IP PDN, which
  `ConnectorInfo` states); marketing and pricing copy that lists transports
  (`views/how_it_works.rs:41`, `views/documentation.rs:58,182`, `views/features.rs:29`,
  `views/index.rs:475`, `views/pricing.rs:544`, `fancier/public/llms.txt:11-18`) stays as it is
  until a NIDD device has worked end to end in the field, per the sales wording rules; D9 covers
  the pricing promise.

Tests: `cargo test -p fancier --target x86_64-unknown-linux-gnu` (the workspace default target is
wasm32). `cargo fmt` only, never `dx fmt`, which collapses rsx comments across the repo.
Verification: the SSG-built artifact under `wrangler dev`, a Nidd pigeon created against local
dovecote, the 400, 403 and 409 messages, the modal's byte count and the 413, the reveal and
`ConnectorInfo`, in both themes at 320 px, by eye.

## 11. The SDK, and the licence

### 11.1 What dovecote uses

dovecote depends on the owner's crate, pinned by git revision:

```toml
thingspace-sdk = { git = "https://github.com/justins-engineering/thingspace-sdk-rust", rev = "<patched revision>", default-features = false, features = ["worker"] }
```

Only `objects/thingspace.rs` touches it: `get_access_token`, `get_session_token`, `send_nidd`, and
the models `LoginResponse`, `Session`, `SessionRequestBody`, `NiddMessage`, `NiddRequest`,
`DeviceID` and `Error`. The reader's probe linked `send_nidd` against worker 0.8.6, the version in
dovecote's lock, for wasm32 (`nidd/understand/wasm-dep-probe/`). The SDK asks for `worker = "0.8"`,
which unifies with it.

The inbound callback is parsed by dovecote's own model (6.6), not the SDK's `NiddCallback`: that
model drops `username` and `password`, has no `niddConfigResponse` variant, and requires exactly
one top-level device id (`thingspace-sdk-rust/src/models/nidd/callback.rs:6-25`); the reader's serde
probe confirmed every configuration callback fails to parse. The callback is the device
authentication boundary, so the code that reads it belongs under dovecote's tests.

New crates in dovecote's wasm graph: `const_format`, `iso8601` (with `nom`) and `base64ct`, the
last a second base64 beside dovecote's `base64 0.22` (`cargo tree`, reader map
`sdk-and-verizon.md` section 1). The reason under the minimal-dependency rule: it is the owner's
own client for this API, named in the request, and the alternative is a second copy of its three
request builders inside dovecote, about 150 lines over `worker::Fetch` (decision D3).

### 11.2 The patch, before dovecote depends on it

At `9d920a4`:

| # | Change | Why | Needed by dovecote |
|---|---|---|---|
| P1 | `Error` keeps the HTTP status: `Error::Api { status: u16, code: Option<String>, message: Option<String> }`, parsed leniently from all three of Verizon's error shapes (`{"fault":{...}}`, `errorCode`/`errorMessage`, `error`/`error_description`) and falling back to the status alone | Every worker function parses a 4xx body into one model and propagates the parse failure (`src/api/worker/access.rs:57-60`, `src/api/worker/devices.rs:113-116`), so an expired bearer or a refused login arrives as `Error::Worker`, indistinguishable from a dropped connection; the latch must tell a gateway `fault` from an M2M refusal, and both from 429/5xx | yes |
| P2 | `get_access_token` returns `Err` instead of panicking: the fixed 96-byte buffer, its `assert!`s (`src/api/request_helpers.rs:25`, `:38`) and `expect("Failed to encode login field")` (`src/api/worker/access.rs:34`) replaced by a `String::with_capacity` Basic value | A panic inside a Durable Object resets it | yes |
| P3 | Remove `Display` for `LoginResponse` (`src/models/login.rs:27-34`) and for `Session` (`src/models/session.rs:30-38`) | They print the access token and the session token; one stray `{}` would log either, and dovecote uses both | yes |
| P4 | `#[serde(default)]` on `LoginResponse.scope` and `token_type`, and `expires_in` defaulting to 3600 (`src/models/login.rs:5-14`) | Which fields Verizon returns is unverified; a missing one would fail every login without ever counting as a rejection | yes |
| P5 | `send_nidd(&NiddMessage)` (the `&mut` is unused, `src/api/worker/devices.rs:86-90`), refusing a message over 1358 decoded bytes or a delivery time outside 2..=2592000 before any request, with rustdoc | Fail before the network, with a typed error | recommended |
| P6 | For the SDK's other users: `NiddCallback` gains `username`/`password: Option<String>` with a `Debug` that redacts the password, `deviceIds: Vec<DeviceID>` defaulting empty, optional `callbackCount`, and a `niddConfigResponse` variant; `list_callback_listeners` answers `Ok(vec![])` for Verizon's empty body; the example worker stops logging tokens and raw callback bodies (`examples/cf-worker/wasm-serv/src/cache/access.rs:22,44,99`, `src/lib.rs:37-49`), and its unauthenticated `api` routes (`src/lib.rs:25-29`) stop being the default feature (`examples/cf-worker/wasm-serv/Cargo.toml:38`) | Correctness for anyone else building on the crate | no |
| P7 | `crate-type = ["rlib"]` for the library, the cdylib only in the example (`Cargo.toml:15`) | A dependent otherwise also builds a stray `thingspace_sdk.wasm` | no |

A `CHANGELOG.md` line per change, tests on Verizon's documented bodies, and the stale "uses ureq"
crate doc (`src/lib.rs:2`) fixed on the way. The owner pushes; dovecote pins the resulting commit.

### 11.3 The licence, stated plainly

- The SDK is `AGPL-3.0-only` (`thingspace-sdk-rust/Cargo.toml:6`).
- pidgeiot's `LICENSE` is the AGPL v3 text; `README.md:80` says "AGPL-3.0" with neither "only"
  nor "or later", and no pidgeiot crate declares a `license` field.
- AGPL code may link AGPL code, so depending on the SDK is compatible whichever way pidgeiot
  reads. If pidgeiot is "or later", the dovecote binary that links the SDK is, as a combined work,
  effectively AGPL-3.0-only. fancier does not link it. The owner holds both copyrights, so this
  constrains nobody but third parties.
- The build gate: `about.toml` accepts only permissive licences (`about.toml:24-35`), and
  `[private] ignore` skips only unpublished workspace members (`:43-44`); the SDK is a git
  dependency, so `generate-oss-notices.sh` (`fancier/scripts/generate-oss-notices.sh:43`) would
  refuse it and fail the release build. The fix is a per-crate table, whose licences "are appended
  to the global list" for that crate alone; "Crate specific configuration _must_ come last in the
  config file" (https://github.com/EmbarkStudios/cargo-about/blob/main/docs/src/cli/generate/config.md,
  read 2026-09-24). At the end of `about.toml`, with `[private]`'s "Must come last" comment
  corrected to "must follow the top-level keys":

```toml
# JES's own ThingSpace client, under the same licence as this repository. Accepted for that crate
# alone, so no other AGPL dependency reaches either inventory unreviewed.
[thingspace-sdk]
accepted = ["AGPL-3.0-only"]
```

Task 1.8 also rewrites the two comments that call the policy "a whole-project decision, not a
per-crate one" (`about.toml:4-5`, `fancier/scripts/generate-oss-notices.sh:38-40`), which the table
would otherwise contradict: the policy stays one whole-project list, with one reviewed exception
for one dependency.

The `/open-source` page then lists it like any other dependency. Whether pidgeiot states "only"
or "or later" is the owner's call (D3) and does not block this work.

## 12. Failure modes

| # | Failure | What happens | Why that is acceptable |
|---|---|---|---|
| 1 | Callback password or account name unset (a deploy gap) | 503, logged as lost with its request id | Honest, and the uplink is lost: ThingSpace retries once at once and never later (B6). Recovery is a support resend by request id from the 30-day archive [CB], if that archive holds it. Deploy the secrets before the registration |
| 2 | Callback from an address outside the allowlist | 403, address logged | Not ThingSpace |
| 3 | Wrong callback password | 403, logged with the body's request id and attempt | Lost, as in row 1: no resend follows the one immediate retry (B6). A rotation is 8.4's case: the replaced password stays accepted for a day |
| 4 | Another ThingSpace customer's callback reaches us (their registration names our URL) | 200, dropped at the account gate, logged | A resend cannot change it |
| 5 | Browser Integrity Check blocks ThingSpace's client | Nothing reaches the Worker, no log line at all | B5: none of 62 ThingSpace requests was challenged; the Configuration Rule of 8.5 step 7 stays as insurance |
| 6 | Body over 8 KiB | 413 | Over three times the largest legitimate body |
| 7 | Body not JSON, or no password | 400 | Kept in ThingSpace's archive for a support resend after a parser fix |
| 8 | Authenticated body of an unknown shape | 200, logged by `CallbackAuth`'s request id and the parse error's category and column | Nothing to do with it; a resend is identical |
| 9 | IMEI no pigeon is bound to | DO 404, callback 200, logged by derived pigeon id | An unprovisioned line; a resend cannot help |
| 10 | Pigeon not claimed, or a frame from a line other than the pinned one (device booted before its pigeon existed, stale claim key after a refresh, a SIM swap, a forger) | 200, frame dropped, `UNCLAIMED 0` (or `1` answering a failed `HELLO`) at most once an hour | The device sends `HELLO` again, or after a failed one stops billable sends until it reboots; nothing is stored or pushed |
| 11 | ThingSpace retries a callback we stored or are still storing (our 2xx lost, or not sent within about 4 s), or support resends one | De-duplication hit, 200; a retry arriving while the first attempt awaits its enqueue waits for it and answers `duplicate` once it stored | Never stored or billed twice |
| 12 | A reading first stored on ThingSpace's retry | Stamped at its arrival, 1.2 to 1.7 s after the first attempt, or, for a retry that waited out a slow first attempt that failed, once that attempt settled | Seconds against readings minutes apart; nothing to correct |
| 13 | Telemetry merge or enqueue fails | 503, key not recorded, any waiting retry told to go on, logged as lost | Honest; the one immediate retry may land it, re-applying the merged values unchanged, though its rate alerts diff against this attempt's newest values (6.3 step 7). Otherwise lost, recoverable only by a support resend |
| 14 | Pigeon DO unreachable | 503, logged as lost | As row 13 |
| 15 | Fuse lookup fails, or takes over a second | Fail-open, logged: the existing rule inside `check_ingest_fuse`, and the gateway's race | A Postgres blip must not brick ingestion or hold the acknowledgement |
| 16 | Account over its free-tier allowance | 200, dropped, `PAUSED 3600` at most once an hour | A 429 would buy a retry that meets the same fuse; the notice makes the device back off |
| 17 | Malformed frame, unknown type, telemetry over a cap | 200, logged with type and length | A firmware bug; a resend is byte-identical |
| 18 | Delivery report, configuration result | 200, logged; a `DeliveryFailed` or `Queued` report first marks the owed push pending in the DO | The next uplink carries the push; nothing else on the device's path depends on them, and a report lost on the way is covered by the 30 s lapse |
| 19 | Verizon refuses the login with an M2M error code, or the OAuth endpoint answers 400 or 401 | Latch set, ops email, every send 503 until a secret changes value or `THINGSPACE_LOGIN_EPOCH` is bumped | At most two strikes per credential set and epoch, with one environment configured, against Verizon's five; the epoch clears a false latch without a rotation |
| 20 | Any other login without a session token (429, 5xx, a timeout, a gateway fault on the retry) | Counted; send 503; the second consecutive one latches | Its fate at Verizon is unknown, and an abandoned request may still have counted |
| 21 | Session idle-expired or replaced, OAuth token expired or stale (a gateway `fault` on login or send) | One re-mint or re-login, one retry | Invisible to callers |
| 22 | ThingSpace refuses a send (a 4xx other than 408 and 429) | 502, logged, tried again on the next uplink past the 30 s window | The same bytes fail the same way, which costs a ThingSpace call but no radio access per uplink until the config changes; 408 and 429 are 503 and retried on the next uplink |
| 23 | Send fails on 503 (unconfigured, latched, unreachable) | `pushed_*` reset; re-sent on the next uplink | The device is known awake then |
| 24 | Device asleep, or awake with no connection, when a push is sent | Lost. B8: `Queued`, not delivered at the next wake, `DeliveryFailed` 30 minutes later. B7 retest, PSM and eDRX off: paged 12.6 and 13.8 s after the send, after ThingSpace had reported `DeliveryFailed` or `Queued` at about 10 s, and never delivered. The report marks the push pending and the device's next uplink carries it as the reply | Every reply sent while the device held its uplink's connection arrived (D13) |
| 25 | Dashboard edits pile up while the device sleeps | One push per 15 minutes; the newest rides the next push or the next uplink's reply | Bounds carrier cost and radio accesses |
| 26 | The same or an older `SHADOW` arrives again (a repeated reply, a buffered push landing late) | The device keeps the highest `target_version` | Device rule, 7.4 |
| 27 | `target_config` over what one `SHADOW` frame carries at the new version (never below 1319 bytes) | 413 at the PUT, nothing written | A target the device could never receive must not exist |
| 28 | Two creates race for one IMEI | Both reach the same DO, which serializes them; the second answers 409 | Uniqueness without an index |
| 29 | An account tries to register an IMEI it does not hold | Outside `NIDD_ALLOWED_ORG_IDS`: 403 before any IMEI lookup, so it can neither probe nor squat. Inside it (JES's own organizations): the rightful create answers 409, no data or downlink crosses, and the organization holding the pigeon deletes it | While D2 keeps NIDD to JES's devices, only JES can hold a Nidd pigeon; D1 is revisited before that changes |
| 30 | Delete, then the same IMEI registered again | Same DO id; the mirror insert deletes any leftover in its own transaction, a failed transaction or any other failure after the DO's 201 undoes the create, and a dictionary older than the pigeon is never served | The seconds-long queue window of section 9 is the residual |
| 31 | dovecote rolled back past the Nidd release while a Nidd pigeon exists | Old code reads the row as an empty `Https` connector (`capsules/src/lib.rs:241`) | Task 0.4 ships first, so a rollback lands on a `refresh_token` that refuses rather than rewrites; the runbook rule stays: never roll back past the Nidd release with Nidd pigeons live |
| 32 | A browser tab holding a pre-Nidd fancier bundle | The flock list fails to parse until reload | fancier deploys first |
| 33 | `NiddService` re-registered elsewhere on the account | Uplink silently goes elsewhere | The runbook's step 2 at every deploy; D11 |
| 34 | Callback latency against an unpublished deadline | The synchronous path is one Postgres read bounded at one second, one DO hop and one enqueue; `ms=` logged per callback | A callback unanswered after about 4 s draws a retry while it runs (13.3, where one attempt took 4.96 s); the retry waits for the first attempt and answers `duplicate` once it stored |
| 35 | Carrier and ThingSpace see frame contents, and the claim key, which also keys the downlink tag, once per boot | Accepted for v1 | They carry every frame already. For uplink the key is no use without a Verizon source address and the listener password, which every API-credential holder can read back; that residual is D12. Application-layer encryption would cost bytes on every frame and is left out of v1 |
| 36 | A secret reaching a log | No log line carries a body, frame, password, token, account name, IMEI, ICCID, IMSI or a serde error's `Display` | Reviewed across every `console_*!` in the change; a unit test proves a numeric IMEI never reaches the parse-error line |
| 37 | Both deployed environments configured | Only during bring-up. At cutover staging loses its account name and API secrets, and `send` answers 503 wherever the allowlist is empty, so only the registered environment receives or sends | [CBBP] allows one endpoint per service per account, and [SEND] requires the listener for sending |
| 38 | A downlink frame not from dovecote (any holder of the API credentials, a replayed old frame) | The device drops a frame whose tag fails; a replayed `SHADOW` loses to a newer version; a replayed `PAUSED` holds at most 86400 s | The claim key is kept in the pigeon's DO and the firmware, never in Postgres (section 9) or a log |
| 39 | An `UNCLAIMED` planned for a frame processed before the same wake's `HELLO` | Argument 0: the device sends `HELLO` again (hourly bound) rather than stopping | Only a failed `HELLO` draws argument 1 |
| 40 | The object resets (a deploy, an exceeded limit) while a telemetry uplink awaits its enqueue | The gateway answers 503 `dispatch_failed`, logged as lost; the in-memory claim ends with it, so the retry stores the uplink | Stored twice if the enqueue had landed before the reset, as before the claim existed; a claim that outlived the attempt would lose the uplink instead |

## 13. Tests and the staging verification plan

Three tiers, each gating the next. Tier 1 needs no device and no Verizon; tier 2 needs the bench
SIM and the account but not the device library; tier 3 is the device transport end to end.

### 13.1 Tier 1: no device

**Unit tests**, on the host target from the repo root (`dovecote/.cargo/config.toml` pins wasm32
inside `dovecote/`, producing a test binary that cannot run):
`cargo test -p capsules`, `cargo test -p dovecote --target "$(rustc -vV | sed -n 's/^host: //p')"`
(`fancier/scripts/build-release.sh:28-39`), `cargo test -p fancier --target
x86_64-unknown-linux-gnu`.

- capsules: the five tests of section 3.
- `helpers/nidd.rs`: golden bytes for frame 3, the `STATUS` frames and `HELLO` of section 7.2, tags
  included, through a host HMAC checked against RFC 4231; that no platform frame holds two adjacent
  NUL bytes, over versions, arguments, configs and keys chosen to produce them under the old binary
  header; the header budget against `NIDD_MAX_TARGET_CONFIG_BYTES`; type decode of every frame and
  of an empty one; `callback_line` taking the ICCID before the IMSI, from the inner list only;
  `CallbackAuth` reading `requestId`; `parse_error_line` over an IMEI sent as a JSON number,
  asserting the line holds none of its digits; `imei_key` for 14, 15 and 16 digits, a bad check
  digit and junk; `callback_imei` with `IMEI`, `imei` and `Imei`, from the inner list and the top
  level; `NiddCallback` over Verizon's documented bodies (MO; MT `Delivered`, `Queued`,
  `DeliveryFailed`; configuration `ConfigCreated` and a failure; no top-level `deviceIds`; no
  `callbackCount`; an unknown variant), as fixtures with placeholders where Verizon's examples carry
  credentials; `dedupe_key`; `remember` trimming at 64; `NiddInFlight` answering each waiting
  retry; `notice_due`; a truth table for `shadow_push_due` and `shadow_reply_due` (hold, in flight,
  lapse, a push reported missed through `mark_pending`, failed send, unclaimed, converged);
  `delivery_missed`.
- `objects/thingspace.rs`: the fingerprint changes when any one secret or the epoch changes and
  not otherwise; `classify_login` latches at once on an M2M error code and on an OAuth 400 or 401,
  answers a gateway `fault` (`900901`, `900902`) with one re-mint and retry, and counts every
  other outcome without a session token, a timeout included, latching on the second in a row;
  `classify_send` drops the session on all three `.SessionToken.` codes of [ERR] and answers 503
  for 408, 429 and 5xx.
- `helpers/coap_service.rs`: the existing allowlist tests (`:62-99`) keep passing, plus one for
  the ThingSpace var.
- fancier: the `Nidd` rows in `device_credentials`, and the `api_doc` structure tests over the new
  `docs/api.md` text.
- thingspace-sdk: P1 against Verizon's three error shapes; P2 with over-long keys returning `Err`;
  P3 for both `LoginResponse` and `Session`, which the build proves by compiling with neither
  `Display` impl; P4 with a token response missing `scope`.

**Local synthetic suite**, `wrangler dev --env dev` with loopback allowlisted. A script builds
bodies in ThingSpace's shape around the frames of section 7, reads the callback password from
`dovecote/.dev.vars` without echoing it, and sets a `User-Agent`:

1. Wrong password 403; allowlist emptied 403; password removed 503; an 8193-byte body 413; a
   non-JSON body 400.
2. Create a Nidd pigeon with a valid test IMEI: 201 with a claim key. Again: 409. A bad check
   digit: 400. With `THINGSPACE_ACCOUNT_NAME` unset: 403. From an organization outside
   `NIDD_ALLOWED_ORG_IDS`, and from a personal flock: 403 for that same IMEI, never 409. The
   Postgres mirror row's connector carries no claim key.
3. A GET of that pigeon through the existing routes, which proves a name-derived id survives
   `id_from_string` in `get_pigeon_do!` (`dovecote/src/lib.rs:376`).
4. Telemetry before `HELLO`: 200, nothing stored, an `UNCLAIMED 0` notice planned (its send
   answers 503 `not_configured`, since dev never holds API secrets, and says so in the log).
5. `HELLO` with a wrong key: still unclaimed, `UNCLAIMED 1` planned. With the right key: claimed,
   the line pinned, a `SHADOW` planned. A telemetry frame naming another ICCID: dropped as
   unclaimed, the claim kept; a good `HELLO` from that ICCID moves the pin.
6. Flat and batched telemetry: values on the dashboard and history rows with the right ages (dev
   writes history directly, `dovecote/src/objects/pigeons.rs:1906-1938`); `callbackCount: 2`
   stores the reading at its arrival, not aged for the attempt.
7. The same body and `requestId` twice: one history row, the second answered `duplicate`. Then the
   same frame and `requestId` as two overlapping attempts, the first held inside its history write
   (dev's stand-in for the enqueue) by a table lock: the retry stays unanswered while the first is
   held, and once released the first is stored and the retry answers `duplicate`, one history row.
   And with every telemetry store failing (a non-loopback `DEVICE_API_HOST` makes dev count as
   deployed, and dev binds no queue): 503, logged as lost, no key kept; its retry, once the store
   works, is stored. Dev bills telemetry on no surface, so these checks count history rows; the
   billing half, one billed message per reading sent, is checked on staging after the deploy (the
   runbook's tier 3 record).
8. A behind shadow report: stored, one billable message, a `SHADOW` planned. A converged one:
   `STATUS STORED`. The same report again: not billed. Telemetry from the converged device: no
   reply.
9. A dashboard write: `SHADOW` planned. A second within 900 s: held. An oversized one: 413. A
   `DeliveryFailed` report while a version is owed: the push marked pending, and the next
   telemetry's reply is the `SHADOW`.
10. Token refresh: the next telemetry is refused as unclaimed.
11. Delete, then recreate with the same IMEI: the old Postgres history for that id is gone before
    the new pigeon's first reading, and a log dictionary uploaded before the recreate answers 404.
12. Delivery report and configuration bodies: 200, one log line each, none naming the IMEI or
    ICCID; a body carrying the IMEI as a JSON number logs none of its digits.
13. A dev account forced over its allowance: `PAUSED` planned once, not again within the hour.

Dev has no Hyperdrive query cache (`[env.dev]` uses a `localConnectionString`), which does not
matter here: no NIDD path reads Postgres before a write it then relies on.

**Staging synthetic.** Deploy fancier, then dovecote, with `wrangler deploy --env staging` (staging
deploys are pre-approved), not a version upload: an uploaded version runs `verify_cf_access`
before the router (`dovecote/src/lib.rs:682`) and would refuse ThingSpace. Run the same suite,
before cutover only, with a Luhn-valid test IMEI that no line on the account carries (checked
against the portal's line list at B1), never the bench's: staging holds API secrets during
bring-up, so the suite's forged `HELLO` draws a real `SHADOW` send, which must reach no device.
The bench's egress address goes into staging's allowlist by `--var` for the session. A `--var`
stays in the deployed version until the next deploy, so removing it takes a redeploy without it,
which is the suite's last step.

### 13.2 Tier 2: the bench SIM, downlink and a replayed callback

After the Phase 0 gate, with `pigeon-examples/nidd_probe` (task 0.2): the bench test
(`/home/justin/pigeon-nidd/nidd-test`) ported to NCS v3.4.0's `lte_lc` PDN calls (the PDN library it
uses was deprecated in NCS 3.2 and is absent from v3.4.0), its `modem/ltes_lc.h` include typo fixed,
a receive loop that prints each downlink as hex and whether its tag verifies against the claim
key (printing ok or bad, never the key), shell commands that send a canned `HELLO` (the
claim key from `prj.local.conf`, the sample credential convention) or N filler bytes, and the IMEI
and ICCID logged at boot. It is built for the nRF9160 Feather (`circuitdojo_feather/nrf9160/ns`),
never the nRF9151, which Verizon does not support. Flashing is pre-approved; the Feather programs
through the nRF5340-DK acting as J-Link, and its console is chosen by serial, not index. Ordered by
risk:

| # | Check | Pass means |
|---|---|---|
| B1 | Owner, in the ThingSpace portal: the bench line has the NIDD price plan and has seen `ConfigCreated`; the account name; whether the plan carries IP data too; which system holds `NiddService` | NIDD is provisioned; D4 and D6 have their facts |
| B2 | Probe with NB-IoT only: registration, band, RSRP | Verizon NB-IoT serves the bench |
| B3 | Non-IP context on `VZWSCEF` (a new CID if B1 says IP, else the default one), activated; `AT+CGCONTRDP` recorded for the Non-IP MTU; any APN rate-control event; CP CIoT indicated without `AT+CCIOTOPT` | A PDN id; the network's MTU and rate control, if it reports them |
| B4 | Staging listener registered (8.5). Send 1, 17, 1358 and 1500 bytes | Which sizes arrive as callbacks, and their decoded lengths |
| B5 | For one real uplink: method, `Content-Type`, `User-Agent`, source address among the eight, no BIC challenge, the IMEI's form and `kind` spelling in both lists, which other identifiers the inner list carries (ICCID, IMSI) for the line pin, `accountName` present, and device-send to DO-write latency from the two logs. Then the de-duplication gate: two identical `HELLO`s sent from two wakes | The parse of section 6.6, `imei_key` and `callback_line` match reality; both `HELLO`s reach the DO as new, with distinct request ids. If not, a device sequence byte enters the frame layout before task 3.1 |
| B6 | **Replay by ThingSpace**: set staging's callback password wrong, send one frame, restore it within five minutes. Then **replay by us**: post the same callback again from the bench, built from the fields B5 recorded, with the same `requestId` and frame. Then **rotation**: rotate staging's listener (8.5 steps 3 to 5) while one uplink is being refused, and send one more between steps 4 and 5 | The resend arrives about five minutes later with the same `requestId` and `callbackCount` 2 and is stored once, backdated 300 s; our replay answers 200 `duplicate`; which password the rotated resend carries, and whether the uplink sent with no listener is archived, lost or delivered (8.4) |
| B7 | **Downlink**: a Nidd pigeon for the bench IMEI, claimed by a probe `HELLO`; a dashboard write whose `target_config` is exactly 1337 bytes at single-digit versions, so the `SHADOW` frame is 1358; the `STATUS STORED` reply to a probe report; each frame's tag checked by the probe; then one 1359-byte message sent directly through the ThingSpace API from the runbook shell (owner-approved) | Raw bytes, not base64, at `recv`; every tag verifies; `Delivered` callbacks; end-to-end latency; the 1359-byte send refused, and how |
| B8 | PSM (the NCS defaults, 30-minute TAU and 60-second active time) and RAI: a downlink sent inside the active time arrives by paging; one sent while asleep reports `Queued`, then `Delivered` at the next wake, and how long after; whether `NRF_RAI_NO_DATA` is accepted on a raw socket | Section 14's release and wait rules hold |
| B9 | Only if B1 says IP: default context IP, Non-IP on a new CID bound to its PDN; an HTTPS GET to `api-staging.pidgeiot.com` while the raw socket is open | FOTA over the IP PDN is possible, and IP downlink is not swallowed by the raw socket |
| B10 | Twelve frames inside six minutes | Any drops or rate-control events |

**Results, 2026-09-24.** The bench nRF9160 Feather (`NRF9160_SICA_REV2`, modem firmware 1.3.7,
the ThingSpace SIM ending 8276) ran the probe with Non-IP on its own CID against staging dovecote
at `5646507`. `wrangler tail` stayed connected from 21:14Z to 00:40Z with no gap over 347 s, so
every absence below is one the tail could have recorded.

| # | Result | What was measured |
|---|---|---|
| B1 | owner | `NiddService` was held by the SDK example worker's `/vzw/nidd`, and 21 other services by the same deleted host; staging's registration displaced it. The plan carries IP data, 250 KB a month (D6) |
| B2 | pass | Verizon NB-IoT, band 13 (EARFCN 5276), RSRP -103 dBm, registered home 8 s after boot with no reject |
| B3 | pass | Non-IP on CID 1, APN `vzwscef`, beside IPv4v6 on `vzwinternet` at CID 0 (IP MTU 1428). The modem reports no Non-IP MTU; `AT+CGAPNRC` and `AT+CCIOTOPT?` answer `ERROR` on this firmware; no rate-control event |
| B4 | partial | 17, 687, 1022, 1189 and 1273 bytes delivered whole; 1283, 1294, 1315 and 1357 refused by `send()` with `EINVAL` before any radio access; 1274 to 1282 untested. The uplink cap is 1273 (7.5, 14.1) |
| B5 | pass | `POST`, `application/json`, `User-Agent: Verizon's callback service`, HTTP/1.1, TLS 1.3, the listener credentials in the body and no `Authorization` header. Six of the eight published addresses (3.87.163.45, 3.91.119.203 and 54.197.62.209 via IAD and EWR; 54.200.43.232, 35.165.205.14 and 34.216.81.234 via PDX); 137.117.33.109 and 168.62.173.153 never. None of 62 requests met Browser Integrity Check. The parse, the account gate and the ICCID line pin held on real bodies. Four `HELLO`s from four wakes got four distinct request ids. A callback is 564 bytes plus the frame's base64 |
| B6 | fail | No resend after a 403 or a 503: one more attempt 1.15 to 1.69 s later (median 1.26 s, from another address in 5 of 17 pairs), then nothing for the three hours the tail stayed up (5.1, 12). Our replay of a stored callback answered 200 `duplicate`. An uplink sent one second after the listener was deleted never arrived. The rotation script's registration was refused 401 96 s after its login (8.5). After each re-registration ThingSpace sent a password other than the registered one for 13 min 55 s and 9 min 27 s (8.4) |
| B7 | fail, fixed, retest pass | Every frame dovecote built (four `SHADOW`, two `STATUS`) refused 400 `AdjacentNullCharacters`. Frames free of adjacent NULs, sent directly with dovecote's tag, arrived whole with raw bytes at `recv` and verified tags: a 14-byte `STATUS` 3.61 s after the API call, a 1358-byte `SHADOW` 4.20 s, a 54-byte one 1.62 s, each reported `Delivered`. 1359 bytes refused `TooLong`. Section 7's text header and hex tag remove every NUL. Retest on that codec (2026-09-25, staging `71642edb`): every frame dovecote built reached the device whole with its tag verifying, four `SHADOW`s of 58 to 1358 bytes (the largest carrying a 1337-byte `target_config` at single-digit versions) and four `STATUS STORED`; a 1338-byte write was refused 413. Two of three unsolicited pushes, made while the device held no connection, were lost (D13) |
| B8 | partial | NCS's 30-minute TAU refused `+CME ERROR: 50`; 190 minutes granted with a 60 s active time (14.1). Inside the active time a `STATUS` arrived by paging 8.75 s after the API call. A downlink sent while the device slept went `Queued`, was not delivered at its next wake, and went `DeliveryFailed` 30 minutes later (its `maximumDeliveryTime` was not recorded; the probe's sender defaults to 600 s); another was never received across six wakes (D13). `RAI_NO_DATA` accepted on the raw socket |
| B9 | pass | Both contexts active; an HTTPS GET over CID 0 beside the open raw socket answered (404, 737 bytes), and NIDD sends kept working |
| B10 | pass at the carrier | Twelve 37-byte frames in six minutes: all accepted, one RRC connection each, each a callback 0.58 to 1.13 s later, no drop and no rate control. At the Worker 7 were stored and 5 refused 403 for a stale password and lost (8.4) |

Latency. Uplink, from `send()` returning to the first callback at the Worker, 38 samples: median
1.12 s, 90th percentile 2.21 s, maximum 6.81 s; from PSM sleep, median 2.21 s. The route's
synchronous work took 29 to 1077 ms. Downlink, from the API call to the probe's receive, over the
four delivered: median 3.91 s, 1.62 to 8.75 s.

Not exercised live: the login latch. One deliberately wrong password spends a real strike on the
account's contact record; the unit tests cover it.

### 13.3 Tier 3: the device transport end to end

With section 14's library and the `nidd_init` sample on the nRF9160 Feather against staging: boot
sends `HELLO` and the pigeon shows claimed in the logs; `pigeon_shadow_get` returns the pushed
shadow; a dashboard write raises `PIGEON_EVENT_SHADOW_UPDATE` inside the active time or at the next
wake; the app applies it and `pigeon_shadow_report` returns 0 on `STATUS STORED`; batched readings
arrive with correct ages; after a token refresh the device's frames are dropped at the platform, any
notice signed with the new key fails its tag, and at the next boot the `HELLO` draws `UNCLAIMED 1`,
which turns billable sends into `-EACCES`; a paused account turns them into `-EAGAIN` with 3600; a
frame with a corrupted tag, sent from the runbook shell, is dropped; FOTA over the IP PDN if B9
passed. Then a 24-hour soak at the contract's cadence with every reading accounted for against the
billing counter, and a field unit at a real site, which is also the NB-IoT coverage check.

**Results, 2026-09-25.** The bench nRF9160 Feather ran `nidd_init` from `pigeon` `nidd-transport`
and `pigeon-examples` `nidd-init`, built with a bench configuration that adds the AT shell and the
library's debug log, against staging dovecote `b8158d7c`, with `wrangler tail` capturing the
Worker's side except for one 35-minute gap. Of 43 checks 36 passed, one only after the sample fix
below, and none failed. Seven were not run or have not finished: the warning for an eDRX cycle
longer than the active time, which this network gave no chance to fire; the lock's `-EBUSY` and
the run-time `-EMSGSIZE`, which the bench could not provoke through the public API, so the code
was read instead; FOTA over the IP PDN, whose signed image outweighs the bench plan's 250 KB of IP
data a month (D6); the field unit; and the soak's two verdicts. One sample defect was found and
fixed on the way: a report that timed out was repeated at once rather than at the next wake. What
the text above does not carry:

- **Timings.** `HELLO` went out 5.6 to 10.9 s after reset; its reply came 1.3 to 4.7 s later, and
  a report's `STORED` 1.0 to 2.5 s after the report, on the connection the `HELLO` or the wake
  opened. An uplink from PSM saw its RRC connection 1.2 to 1.6 s after `send()` at 16 of 22 wakes,
  2.7 to 5.1 s at five and 35.7 s once; the modem held the frame and it arrived whole, but its
  readings were stamped 35 s late, since `age_secs` resolves at receipt. One PDN activation after
  a re-attach found no RRC connection within 30 s and failed; the next one worked.
- **A frame that lands after the connection closed can still arrive.** A push sent 0.6 s before
  its wake's connection went idle and a reply sent 1.7 s after were paged 3.8 and 2.2 s after the
  send, inside the 60 s active time, and delivered. A frame sent to a device in PSM went `Queued`
  or `DeliveryFailed` and was never delivered later, including one sent 0.6 s before the device's
  own uplink connected. Tier 2's worst-case push, sent with the 86400 s `maximumDeliveryTime`
  dovecote asked before D13 was implemented, reported `DeliveryFailed` "timeout, could not deliver
  data" 24 hours after it was sent, having reached none of the device's connections that day; a
  push sent with the current 30 s drew `Queued` and no final report in the next eight hours
  (outside a 35-minute gap in the tail).
- **RAI.** `RAI_ONGOING` is accepted on the raw socket and does not lengthen a telemetry-only
  connection (3.7 to 4.1 s, the probe's 3.5 to 4.6 s without it). After `RAI_NO_DATA` Verizon
  released the radio 2.6 to 3.2 s later at a wake and 1.2 to 6.0 s later at a boot.
- **Sockets.** mfw 1.3.7 refuses `SO_KEEPOPEN` (`EINVAL`) at every open; `AT+CFUN=4` ends the
  blocking `recv` with `ENETDOWN`, and later sends re-create the PDN and the socket with the 60 s,
  then 120 s backoff. The close in `pigeon_nidd_stop()` ends that `recv`: a `reboot` target
  restarted the board about 2 s after its report went out. The receive thread's stack peaked at
  744 of 2048 bytes after a boot's `HELLO`, a repeat report sent from that thread, and `PAUSED`
  and `UNCLAIMED 0` notices; a repeat report that must first re-activate the PDN was not run.
- **PSM and eDRX.** A 0 s active time is grantable (`CONFIG_LTE_PSM_REQ_RAT="00000000"`) and
  draws the warning. Verizon granted no eDRX alongside PSM (`1001` requested, `+CEDRXRDP` empty),
  so the warning for a cycle longer than the active time cannot fire on this network.
- **ThingSpace retries a callback that has not answered in about 4 s**, even when the first attempt
  then answers 200. On the bench a first attempt took 4.96 s, its retry arrived 4.0 s after it, and
  both stored the same six readings and billed them, because `nidd_uplink` checked `seen` before
  the telemetry enqueue awaited and recorded the key only after it. B6's one retry 1.2 to 1.7 s
  after a refusal describes refusals only. Fixed since: a retry that finds its key held by an
  attempt still awaiting the enqueue waits for that attempt's outcome (6.3 steps 2 and 7), and
  tier 1 step 7 checks it.
- **An oversize target** (320 bytes, one over what the default device keeps) passed the `PUT`,
  was dropped by the device, and came again in the reply to the next uplink, as `docs/api.md`
  says, until a small write replaced it.
- **A `PAUSED` hold costs readings.** The frame that drew `PAUSED` returned 0 from its flush,
  since the modem took it, and none of its readings was stored. Through the one-hour hold the
  device made no radio access, and over the hold and the wake that ended it the batch buffer kept
  the newest 6 of 20 readings, as its depth allows; those 6 were stored with correct ages, and
  nothing was billed while paused.
- **The runbook shell** must fire after the device's RRC connection is up: of three frames sent
  during the connection's setup from PSM, two failed at once and one was `Queued` and never
  delivered; every frame sent after the console's `NIDD: radio connected` arrived.
- **The soak** started at 01:52Z on 2026-09-26 on the refreshed key, at the sample's 20-minute
  wake (target `telemetry_interval` 1200), with every reading tallied against staging's
  history and billing every 30 minutes. It ends about 02:53Z on 2026-09-27, and its verdicts are
  read from the tally then.

## 14. The device transport follow-on: `pigeon_nidd.c`

A fourth connector in `~/pigeon`, `PIGEON_CONNECTOR_NIDD`, NCS and nRF91 only, in one new file,
`src/pigeon_nidd.c`. It is a build choice like the other three, so only a device built for NIDD
depends on it, which is the 2026-09-18 ruling. The shape is the MQTT transport's: a cached pushed
shadow, and a report folded into the cache when it is confirmed
(`pigeon/src/pigeon_mqtt.c:1090-1180`).

### 14.1 The device contract (published in `docs/api.md` under "NIDD sizes and cadence")

- At most four radio accesses an hour, uplink and downlink together [NUG]. This is the
  application's obligation, stated in `pigeon.h` and in `docs/api.md`'s "NIDD sizes and cadence":
  the library has no wake cadence of its own (`pigeon/zephyr/Kconfig` has no interval or wake
  symbol; its only intervals are `PIGEON_LOG_UPLOAD_MAX_INTERVAL_MS`, `:514`, and
  `PIGEON_WS_PING_INTERVAL_SEC`, `:869`), and it does not enforce the limit, because a paged
  downlink is an access the library cannot count, so a spacing guard would misjudge in both
  directions. The `nidd_init` sample's 20-minute wake leaves one access an hour for a paged
  push, and a reply rides its uplink's connection and costs none; 15 minutes is the floor.
- Hold the connection for the reply, then release it. Any frame from a claimed device that is
  behind draws the owed `SHADOW`, and only a frame sent while the device holds the connection its
  uplink opened reliably arrives: in the B7 retest every reply did, 1.1 to 4.7 s after that
  connection came up, while two pushes to the idle device were paged 12.6 and 13.8 s after the
  send, after ThingSpace had given up (D13). So the transport never requests release right after
  a send. Once the reply owed to a `HELLO` or report has arrived it sets `NRF_RAI_NO_DATA`; after
  a wake of telemetry alone, which draws a reply only while the device is behind, it leaves the
  release to the network, which let a connection go 2.7 to 5.3 s after its last downlink and 3.5
  to 4.6 s after a plain uplink: about Verizon's 5 seconds [NUG]. Tier 3 saw a page succeed twice,
  a push and a late reply delivered 3.8 and 2.2 s after the send inside the active time (13.3);
  the rule stands, since a page is not something the device can count on.
- PSM and eDRX set explicitly at every init, and the grant checked. The modem keeps `AT+CPSMS`
  and `AT+CEDRXS` in NVM across images and a full erase (B8): the bench board came up on eDRX
  with a 163.84 s cycle and PSM off, left by an earlier image, and Phase 0 was granted a 0 s
  active time for a PSM request (12 hours, 0 s) retained the same way. So the transport writes
  both before attaching and never inherits them. NCS's default request, a 30-minute TAU, was
  refused `+CME ERROR: 50` on Verizon NB-IoT, as were 1, 2 and 3 hours; a bare `AT+CPSMS=1` and
  190 minutes (`00010011`) were accepted, and the network granted a TAU of 11400 s with a 60 s
  active time. The transport logs the granted TAU and active time and warns on an active time of
  0, which leaves no window in which a downlink can page the device. eDRX is written off unless
  the application asks for it: a device in eDRX is paged only on its paging occasions, and with
  a 163.84 s cycle a 60 s active time may hold none, so a push sent inside it would never be
  paged. An application that enables eDRX keeps its cycle shorter than the active time. On the
  bench Verizon granted no eDRX at all to a device that also asked for PSM (13.3).
- Uplink frames capped at 1273 bytes, the largest the bench modem accepted (B4, mfw 1.3.7); it
  refused 1283 in `send()` with `EINVAL` before any radio access, and 1274 to 1282 are untested,
  so 1273 is the cap until a modem firmware measures more. dovecote accepts up to 1358 either way.
- Telemetry batched with `age_secs` inside one frame (1272 body bytes). The core never splits a
  flat set to fit a transport: it hands the transport one pre-built body
  (`pigeon_transport_report_telemetry`, `pigeon/src/pigeon_internal.h:140-142`) from a buffer of
  `PIGEON_TELEMETRY_BODY_MAX` bytes (`pigeon/src/pigeon_core.c:75`), which grows with
  `CONFIG_PIGEON_TELEMETRY_MAX_KEYS` (`pigeon_internal.h:60-66`), and splits only escape-heavy
  sets that overflow that same buffer (`pigeon_internal.h:53-55`). At 7 keys the buffer is
  about 1158 bytes with its NUL, at 8 it is 1323 and at 9 it is 1488. So the build refuses a key
  count whose flat body could exceed the frame, 8 keys included (14.2), and `pigeon_nidd.c` never
  has to split pre-built JSON.
- `HELLO` at every boot, again ahead of the next billable frame when one drew no reply within
  `CONFIG_PIGEON_NIDD_REPLY_WAIT_SEC`, and after an `UNCLAIMED 0` notice (at most hourly). Nothing
  polls: the repeat rides the connection that billable frame opens. A lost reply is not only a
  lost claim: a pigeon converged at boot gets its target only in the `SHADOW` answering `HELLO`
  (`shadow_reply_due` needs a push outstanding), so without the repeat it would run its built-in
  defaults for the whole boot while the dashboard showed it converged.
- Every platform frame's tag verified against the built-in claim key; a frame that fails is
  dropped and logged.

### 14.2 Kconfig and build

```kconfig
config PIGEON_CONNECTOR_NIDD
  bool "Non-IP Data Delivery through the carrier (nRF91, NB-IoT)"
  depends on NRF_MODEM_LIB
  select LTE_LINK_CONTROL
  select LTE_LC_PDN_MODULE
  select LTE_LC_PSM_MODULE
  select LTE_LC_EDRX_MODULE
  select LTE_LC_RAI_MODULE
  select PSA_CRYPTO
  select PSA_WANT_ALG_HMAC
  select PSA_WANT_ALG_SHA_256
  select PSA_WANT_KEY_TYPE_HMAC
  imply LTE_RAI_REQ

if PIGEON_CONNECTOR_NIDD
config PIGEON_NIDD_CLAIM_KEY          # string, 32 lowercase hex: what the dashboard's reveal names
config PIGEON_NIDD_DEDICATED_CID      # bool, default y: Non-IP on a new CID, IP left on the
                                      # default one; n only on a NIDD-only plan, which then has
                                      # no IP at all and cannot enable FOTA
config PIGEON_NIDD_RAI_IDLE_MS        # int, default 500, range 0 to 5000: quiet time after an
                                      # owed reply before releasing the radio
config PIGEON_NIDD_REPLY_WAIT_SEC     # int, default 30, range 1 to 300: pigeon_shadow_report's wait
config PIGEON_NIDD_SHADOW_WAIT_SEC    # int, default 60, range 1 to 600: how long
                                      # pigeon_shadow_get waits for a HELLO's reply
config PIGEON_NIDD_THREAD_STACK_SIZE  # int, default 2048
endif
```

As built (`~/pigeon` branch `nidd-transport`). `LTE_LINK_CONTROL` is selected, not depended on:
a dependency is a Kconfig dependency loop back into the connector choice, since
`NRF_MODEM_LIB_NET_IF` selects `LTE_LINK_CONTROL` and depends on the connection manager, whose
selectors include `NET_SOCKETS`, which the CoAP connector's `COAP` selects. `PSA_WANT_ALG_HMAC`
alone selects nothing; the probe needed `PSA_CRYPTO`, `PSA_WANT_ALG_SHA_256` and
`PSA_WANT_KEY_TYPE_HMAC` beside it on TF-M, so the connector selects all four. `LTE_RAI_REQ` is
implied so an application can still turn it off; with the RAI module in and it off, lte_lc writes
`AT%RAI=0` at every modem init.

Existing symbols (`pigeon/zephyr/Kconfig`): the CoAP prompt "CoAP over NIDD/Cellular" (`:16`)
becomes "CoAP over DTLS or TLS", since nothing in pigeon's CoAP path touches NIDD;
`PIGEON_FOTA` (`:525`) gains `|| (PIGEON_CONNECTOR_NIDD && PIGEON_NIDD_DEDICATED_CID)` and
`PIGEON_FOTA_HTTPS_ENDPOINT` (`:584`) gains `PIGEON_CONNECTOR_NIDD`, the situation its help text
already describes for MQTT, and the `if` holding `PIGEON_HTTPS_SEC_TAG` and
`PIGEON_HTTPS_NATIVE_TLS` (`:209`) widens to `(PIGEON_CONNECTOR_MQTT || PIGEON_CONNECTOR_NIDD) &&
PIGEON_FOTA`, since the image fetch reads both; `PIGEON_TELEMETRY_BATCH` (`:1135`) gains
`PIGEON_CONNECTOR_NIDD`, with its buffer defaulting to 1024 on NIDD; `PIGEON_LOG_UPLOAD`
(`:456`) stays off NIDD in v1; `PIGEON_ENDPOINT`'s help (`:1282`) gains `nidd://VZWSCEF`.
`pigeon/CMakeLists.txt` gains
`zephyr_library_sources_ifdef(CONFIG_PIGEON_CONNECTOR_NIDD src/pigeon_nidd.c)`, and the
`pigeon_https.c` condition (`:41`) gains `OR (CONFIG_PIGEON_CONNECTOR_NIDD AND CONFIG_PIGEON_FOTA)`.
`pigeon.h` gains the enum value (`:17-21`), the events guard (`:528`) and
`pigeon_nidd_start`/`pigeon_nidd_stop`; `pigeon_init` (`pigeon/src/pigeon_core.c:231-274`) gains the
case, checks the `nidd://` scheme and configures the PDP context while the modem is offline.

Build-time checks, each a `BUILD_ASSERT` naming the Kconfig to change:
`PIGEON_TELEMETRY_BODY_MAX <= 1273` (the value counts the NUL, so the flat body is at most 1272
bytes plus the type byte; it fails at the default 8 keys and holds at 7, so a NIDD build defaults
`CONFIG_PIGEON_TELEMETRY_MAX_KEYS` to 7), naming `CONFIG_PIGEON_TELEMETRY_MAX_KEYS` as the one to
lower; the telemetry batch body fits 1272 bytes; `CONFIG_PIGEON_SHADOW_CONFIG_MAX + 64 <= 1272`
(the report body);
`sizeof(CONFIG_PIGEON_NIDD_CLAIM_KEY) == 33`; an NB-IoT network mode (`LTE_NETWORK_MODE_NBIOT`,
`_NBIOT_GPS`, or a dual mode preferring NB-IoT).

### 14.3 `pigeon_nidd.c`

As built on `~/pigeon` branch `nidd-transport`; where the build departed from the first design,
the reason is given in place.

- **Configure** (from `pigeon_init`, modem offline): parse `nidd://<APN>` and import the claim key
  into PSA, refusing anything but 32 lowercase hex characters (the form the reveal shows and
  `HELLO` sends, so no case conversion exists). With a dedicated CID, first set CID 0 to
  `LTE_LC_PDN_FAM_IPV4V6` with the subscription's APN, since an image without a dedicated context
  may have left it Non-IP (the probe's order, proven in tier 2), then `lte_lc_pdn_ctx_create`;
  else CID 0 with `lte_lc_pdn_default_ctx_events_enable`. Then `lte_lc_pdn_ctx_configure(cid,
  <APN from the endpoint>, LTE_LC_PDN_FAM_NONIP, NULL)`. Then PSM and eDRX, always written
  (14.1): `lte_lc_psm_req(true)` and `lte_lc_edrx_req(IS_ENABLED(CONFIG_LTE_EDRX_REQ))`, which
  writes eDRX off unless the application requested it. There is no `lte_lc_psm_param_set` call:
  lte_lc's modem-init hook already holds the application's `CONFIG_LTE_PSM_REQ_RPTAU` and `_RAT`
  in whichever format the application chose, and a second write from the library would override
  an application that sets them in seconds. NCS parses its own 30-minute default before pigeon's
  Kconfig, so the library cannot re-default it; a NIDD application sets
  `CONFIG_LTE_PSM_REQ_RPTAU="00010011"`, 190 minutes (B8), and the library's documentation says
  so. A refused PSM or eDRX write is logged, naming the Kconfig to change, and never fails
  `pigeon_init`: a power setting must not keep the device off the network. A refused PSM request
  also turns PSM off (`lte_lc_psm_req(false)`), since a refused `AT+CPSMS` leaves whatever an
  earlier image wrote (B8), such as the 0 s active time Phase 0 was granted. After registration,
  log the grant from `LTE_LC_EVT_PSM_UPDATE` and `LTE_LC_EVT_EDRX_UPDATE`, warning on a 0 s
  active time or an eDRX cycle longer than it, and log each RRC transition, since the carrier's
  guideline counts radio accesses. With eDRX written off the modem reports no eDRX, so no eDRX
  line follows `NIDD: eDRX off`. At a detach (`AT+CFUN=4`, power-off) lte_lc reports the PSM
  timers as -1 and the library logs `NIDD: network did not grant PSM`, which there means the
  network is gone, not that it refused.
- **Start** (after registration): `lte_lc_pdn_activate` for a dedicated CID, then a wait of up to
  60 s for `LTE_LC_EVT_PDN_ACTIVATED` before `lte_lc_pdn_id_get`, because `AT+CGACT` answering is
  not the PDN being up; `zsock_socket(AF_PACKET, SOCK_RAW, 0)`; `SO_BINDTOPDN`, because a raw
  socket on a shared PDN intercepts downlink meant for other sockets
  (https://github.com/nrfconnect/sdk-nrfxlib/blob/main/nrf_modem/doc/sockets/raw_sockets.rst, read
  by the readers on 2026-09-24); `SO_KEEPOPEN` best-effort only, because the option exists on
  mfw_nrf91x1 2.0.1 and later and mfw_nrf9151-ntn but not on the nRF9160's mfw 1.3.7, which
  refuses it with `EINVAL`, and where a socket whose PDN went down answers `ENETDOWN` (its blocking
  `recv` did after `AT+CFUN=4`) and must be closed; `SO_SNDTIMEO` of 30 s, because every send
  holds the module mutex and the modem's default is no timeout; the receive thread; then
  `HELLO`. Log the IMEI once, so the operator can match it to the dashboard. An open that fails, at
  start or later, is retried by the next send after a backoff of 60 s doubling to 1920 s, since
  activating a PDN is a radio access; a `HELLO` that fails at start, or draws no reply within
  `CONFIG_PIGEON_NIDD_REPLY_WAIT_SEC`, is sent before the next billable frame. The next send after
  the PDN went down (a `DEACTIVATED` or detach event, or a send answering `ENETDOWN` or
  `ENETUNREACH`) re-activates it and re-creates the socket, and one after
  `LTE_LC_EVT_PDN_CTX_DESTROYED` (lte_lc frees every non-default context at `CFUN=0`) re-creates the
  context first.
- **Send** (one module mutex, bounded and answering `-EBUSY` as the other connectors do): refuse
  a billable frame while unclaimed or paused before any radio access, send a `HELLO` still due
  first, frame into a static 1273-byte buffer, `SO_RAI` to `RAI_ONGOING` (the socket layer's name
  for `NRF_RAI_ONGOING`), `zsock_send`. Every send cancels a scheduled release. When the reply owed
  to a `HELLO` or report arrives and nothing else is owed, a delayable work item sets
  `RAI_NO_DATA` after `CONFIG_PIGEON_NIDD_RAI_IDLE_MS` of quiet, unless that reply raised
  `PIGEON_EVENT_SHADOW_UPDATE` for a version the application will report on the same connection;
  a wake of telemetry alone leaves the release to the network (14.1). B8 found `RAI_NO_DATA`
  accepted on a raw socket, and tier 3 found `RAI_ONGOING` accepted too. The `lte_lc` handler
  never takes the mutex (atomics and log lines only), because an lte_lc PDN call made under it
  completes on notifications delivered from the handler's own context; the release work item
  takes it without waiting and skips when busy.
- **Receive thread** (blocking `zsock_recv` into 1358 bytes): first the tag, the last 16
  characters, the hex of the first 8 bytes of HMAC-SHA256 over every byte before them keyed by
  `CONFIG_PIGEON_NIDD_CLAIM_KEY`, through PSA
  Crypto, which the library already uses for SHA-256 (`pigeon/src/pigeon_psk.c:54-55`); a frame
  whose tag fails is dropped and logged, compared in constant time. Then the header's two decimal
  integers up to its newline; a version above 2147483647 is dropped rather than wrapped into
  `int32_t`, in a `SHADOW` header and in `STATUS STORED`'s argument alike. Then: a `SHADOW` older
  than the cached one is dropped except for its `current_version`; a newer one is cached (dropped
  and logged if its config exceeds `CONFIG_PIGEON_SHADOW_CONFIG_MAX - 1`, after which the
  platform, seeing the device still behind, sends it again in reply to every uplink until a
  smaller target replaces it), gives the shadow-wait semaphore and raises
  `PIGEON_EVENT_SHADOW_UPDATE`. A `current_version` at or above a pending
  report confirms it, as does `STATUS STORED` with an argument at or above it, so a late
  `STORED` for an older report cannot confirm a newer one. A `current_version` below the version
  this boot reported means the report was lost: report again, unless it is still inside its
  wait or the same `SHADOW` brought a newer target, whose report the application sends instead
  (7.4). `STATUS PAUSED s` makes billable sends answer `-EAGAIN` for `s` seconds, capped at 86400;
  a newer `PAUSED` replaces the hold and `PAUSED 0` ends it. Readings taken meanwhile wait in
  the batch buffer, which keeps the newest `CONFIG_PIGEON_TELEMETRY_BATCH_DEPTH` and drops older
  ones. `STATUS UNCLAIMED 0` sends one `HELLO` if none went in the last hour and leaves a waiting
  report pending, since the `HELLO` re-claims and the application reports again at its next wake.
  `STATUS UNCLAIMED 1`, the answer to a failed `HELLO`, makes billable sends answer `-EACCES`
  until the next boot, the device's version of a 401. Anything else is ignored. A receive that
  ends closes the socket and raises `PIGEON_EVENT_DISCONNECTED`; `PIGEON_EVENT_CONNECTED` is
  raised when the thread starts reading a new socket. The event callback runs on this thread, so
  `pigeon_shadow_report` called from it answers `-EDEADLK` at once rather than timing out on a
  reply the same thread must receive.

| Transport call | Behaviour | Returns |
|---|---|---|
| `pigeon_transport_report_telemetry` | Refused early while paused or unclaimed; else one `TELEMETRY` frame, `res` zeroed as CoAP and MQTT do | 0 when the modem took it; `-EAGAIN` with `res->retry_after_sec`; `-EACCES` |
| `pigeon_transport_upload_logs` | Not defined: `PIGEON_LOG_UPLOAD` cannot be enabled on NIDD, so nothing references it (the CoAP connector's precedent) | links nowhere |
| `pigeon_shadow_report` | One `SHADOW_REPORT`, then a wait up to `REPLY_WAIT` for its confirmation; a late one folds into the cache when it arrives | 0 confirmed; `-ETIMEDOUT` (re-report next wake, harmless); `-EAGAIN`; `-EACCES`; `-EDEADLK` from the event callback |
| `pigeon_shadow_get` | A copy of the cached target; with none cached, a call waits only while a `HELLO`'s reply is owed, up to `SHADOW_WAIT` after that `HELLO` (an `UNCLAIMED 1` ends the wait), and otherwise answers `-EAGAIN` at once, so a refused key or an oversize target does not stall every wake. `current_version` is the newest version the platform has named, `current_config` the config this device last reported this boot (`""` before its first report), `updated_at` 0 | 0; `-EAGAIN` |
| `pigeon_transport_download_firmware` | Not in this file: `pigeon_https.c` over the IP PDN | as today |

Static RAM at defaults, measured on the `nidd_init` build: a 1273-byte send buffer and a
1358-byte receive buffer, three 320-byte configs (the cached target, the last report, and
`pigeon_shadow_get`'s copy, which keeps "valid until the next call" true while the receive thread
rewrites the cache), a 2048-byte stack and about 410 bytes of thread, lock and state: 6053
bytes, the sum of every `pigeon_nidd*` data and bss symbol by `nm -S`. On the bench the receive
thread's stack peaked at 744 of its 2048 bytes (13.3).

`CONFIG_PIGEON_WATCHDOG` is fed only by a delivered flush, so a NIDD application that enables it
needs a timeout above its wake interval plus any `PAUSED` hold; the library cannot see the
application's cadence, and `nidd_init` leaves it off.

### 14.4 With the departure board in mind

PidgeIoT's origin was putting departure boards on NIDD, so the question deserves a straight answer.

- **The board is LTE-M only**
  (`/home/justin/embedded-departure-board/app/boards/circuitdojo_feather_nrf9160_ns.conf:49`,
  `CONFIG_LTE_NETWORK_MODE_LTE_M=y`), and Verizon's NIDD is NB-IoT only [NIDD][SEND]. A NIDD
  board would be a different radio build, and nobody has shown Verizon NB-IoT coverage at the
  Massachusetts sites (B2 and a site survey would).
- **The board refreshes departures every 30 seconds** by default, runtime-clamped to 5 to 45
  seconds (`/home/justin/embedded-departure-board/app/Kconfig:27-35`): 120 radio accesses an hour
  against the guideline's four. Verizon allows that "in certain circumstances" [NUG], which would
  be a negotiation, not a setting.
- **Size is not the constraint.** A stop's next departures are on the order of 100 to 200 bytes as
  compact JSON (an estimate), far inside 1358.
- **The origin never carried sign data over NIDD.** dusty-loft's encoder for a stop was an empty
  stub (`justins-engineering/dusty-loft@5d07cbe:src/nidd_client.c:22`); only "Hello world!" went
  down and printed uplinks came up (reader map `device-and-origin.md` 3.1).

So the LED departure board stays on LTE-M IP, and its bandwidth path is the CoAP migration
(`docs/design/coap-nrf91-migration.md`). The low-power e-paper variant the owner ruled in on
2026-09-18 is planned on the nRF9151, which Verizon does not support, so as planned it cannot carry
NIDD on Verizon; a NIDD sign would need a core Verizon supports. Where NIDD could fit such a sign
is a shelter display showing scheduled times, updated by exception a few times an hour. For that
case the envelope reserves platform frame `0x83` for application data, and the platform would need
a way to address a downlink that is not a shadow; neither is in v1 (decision D10).

### 14.5 Sample and estimate

`pigeon-examples/nidd_init` (NCS workspace only, `circuitdojo_feather/nrf9160/ns`; never the
nRF9151, which Verizon does not support): NB-IoT only, PSM requested at 190 minutes and a 60-second
active time (NCS's default 30 minutes was refused, B8) with eDRX written off, RAI, batched
telemetry, a 20-minute wake that records readings and flushes (which is what keeps the sample inside
14.1's four accesses an hour), the claim key in `prj.local.conf`, and a `PIGEON_EVENT_SHADOW_UPDATE`
handler that applies and reports. The probe of 13.2 stays beside it as the carrier-side diagnostic.
As built (branch `nidd-init`): the wake is the shadow's `telemetry_interval`, 1200 s until the
shadow sets it and never under 900 s (a lower value is applied as 900 and reported so), with four
readings a wake and the first wake a full interval after `HELLO`; a report that times out waits
for the next wake, where the first build sent it again at once, which at the default 30 s wait
costs a radio access of its own (found in tier 3); `reboot` is acted on only after this boot's
report of that version was confirmed; and the build refuses every board but
`circuitdojo_feather/nrf9160/ns`.
`~/pigeon`'s docs gain the connector, and the frame table of section 7 with `docs/api.md` named as
the authority, the way `loft` names it for `CoapPskLookup`. Estimate: 20 to 32 hours for the
library, 4 to 8 for the sample, 8 to 16 on the bench.

## 15. Sequencing and work breakdown

Hours are ranges for one engineer, bench and deploy waiting included; owner hours are separate.

**Gate:** nothing merges to `main` until tasks 0.1 to 0.3 pass and task 0.5 is done. Task 0.5 is
independent of NIDD and urgent: it closes an exposure that exists today. If the bench nRF9160
Feather cannot attach to Verizon NB-IoT, the connector cannot be proven and the work stops at
Phase 0. The nRF9151 is no fallback: Verizon does not support the board.

Phase 0, the gate and the rollback guard:

| # | Repo | Task | Hours | Depends |
|---|---|---|---|---|
| 0.1 | owner | B1 in the ThingSpace portal: plan, `ConfigCreated`, account name, IP data, the current `NiddService` holder | 0.5 to 1 | none |
| 0.2 | pigeon-examples | `nidd_probe` (13.2) | 4 to 8 | none |
| 0.3 | bench | B2, B3 | 2 to 4 | 0.1, 0.2 |
| 0.4 | pidgeiot | `refresh_token` refuses an unparseable stored connector (6.5); deployed alone, staging then production | 1 to 2 | none |
| 0.5 | owner | Now: delete the SDK's example worker deployment, which is public at https://thingspace-sdk.justinsengineeringservices.workers.dev/ (its `POST /api/send_nidd` reached its handler with no authentication on 2026-09-24, answering 400 "Bad 'Content-Type' header" with nothing sent), and its KV namespace; its default `api` build (`thingspace-sdk-rust/examples/cf-worker/wasm-serv/Cargo.toml:38`) serves listener list, register and deregister, a device list and `send_nidd` with no authentication (`src/lib.rs:25-29`), and the list returns every listener password [LIST]. Confirm the 2023 middleware's host (dusty-loft) holds no credentials; read the worker's logs for `/api/*` hits; then rotate the UWS password and the OAuth key pair in the ThingSpace portal and update `secrets.env` | 1 to 2 | none |

Phase 1, the platform, on branch `nidd-connector`:

| # | Repo | Task | Hours | Depends |
|---|---|---|---|---|
| 1.1 | thingspace-sdk | P1 to P5, tests, `CHANGELOG.md`; the owner pushes (P6, P7 add 2 to 3) | 3 to 6 | none |
| 1.2 | pidgeiot/capsules | `NiddConfig`, variant arms, constants, `imei_is_valid`, rustdoc, tests | 2 to 3 | none |
| 1.3 | pidgeiot/dovecote | `helpers/nidd.rs` and its tests | 4 to 6 | 1.2 |
| 1.4 | pidgeiot/dovecote | `ThingSpaceSession`: tokens, mutex, `classify_login` with the epoch and the two-failure latch, ops email, send with `classify_send` and the allowlist check; binding, `v2` migration, vars, comment block | 6 to 9 | 1.1 |
| 1.5 | pidgeiot/dovecote | Callback route, shared allowlist helper, gateway fuse raced against one second, the line id, `nidd_uplink_via_do` | 5 to 7 | 1.3 |
| 1.6 | pidgeiot/dovecote | `Pigeons`: `pigeon_nidd` and its wipe, the uplink path with the line pin and `UNCLAIMED`'s argument, frame signing, the two handler refactors, the push rule in `update_shadow`, the connector arms, create by name with 409; `device_ingest_paused`'s rustdoc names the NIDD callback as the second gateway-checked surface and why (6.3 step 6) | 12 to 17 | 1.3, 1.4 |
| 1.7 | pidgeiot/dovecote | Create route: enable switch, org allowlist, IMEI check, `id_from_name`, the `?` at `lib.rs:1509-1512`; the clean slate inside `insert_pigeon_pg_db`'s transaction and the undo on its failure, the secret-free Nidd mirror, the dictionary's `created_at` check, `delete_log_dictionary` and the comment at `lib.rs:2013-2016` | 3 to 4 | 1.6 |
| 1.8 | pidgeiot | Dependency pin, `about.toml` table, the two whole-project comments (11.3), a `generate-oss-notices.sh` run | 1 to 2 | 1.1 |
| 1.9 | pidgeiot/fancier | Section 10 | 7 to 11 | 1.2 |
| 1.10 | pidgeiot/docs | `docs/api.md` (section 5), CLAUDE.md's connector paragraph and its free-tier-fuse paragraph (the NIDD callback is the second gateway-checked surface, for the cost reason of section 17), `README.md:77`, the `docs/infra/thingspace-nidd.md` runbook | 4 to 6 | 1.5, 1.6 |
| 1.11 | verification | Tier 1 local suite (13.1) | 3 to 5 | 1.5 to 1.10 |

Phase 2, staging with the bench SIM:

| # | Repo | Task | Hours | Depends |
|---|---|---|---|---|
| 2.1 | pidgeiot | Staging deploys, fancier then dovecote; staging secrets; the staging synthetic suite | 2 to 3 | Phase 1 |
| 2.2 | owner | 8.5 registration and the BIC rule | 1 to 2 | 2.1, D4 |
| 2.3 | bench | B4 to B10 | 6 to 12 | 2.2, 0.3 |

Phase 3, the device library:

| # | Repo | Task | Hours | Depends |
|---|---|---|---|---|
| 3.1 | pigeon | Kconfig, `pigeon_nidd.c` with tag verification and the `PAUSED` cap, core and CMake hooks, asserts, docs | 22 to 35 | 2.3 (B4, B5, B7, B8) |
| 3.2 | pigeon-examples | `nidd_init` | 4 to 8 | 3.1 |
| 3.3 | bench | Tier 3 and the 24-hour soak | 8 to 16 | 3.2 |

Phase 4, production, each step on the owner's word:

| # | Repo | Task | Hours | Depends |
|---|---|---|---|---|
| 4.1 | pidgeiot | Production secrets, allowlist and org list; deploy fancier, then dovecote; re-register the listener (8.5); delete staging's account name and API secrets and empty its allowlist | 2 to 3 | 3.3, D2, D4 |

Independent and optional: pass the month start into `check_ingest_fuse` as a parameter
(section 9), 1 to 2 hours, not counted below.

Totals: 65 to 105 engineering hours for the platform through a proven staging loop (Phases 0 to
2); 36 to 62 for the device library and production (Phases 3 and 4); 101 to 167 in all. Owner: 3.5
to 7 hours.

Order of deploys: 0.5 now, before anything else; 0.4 alone; the Phase 0 gate; the SDK patch pushed
and pinned; fancier with the variant (staging, and production before any production Nidd pigeon);
dovecote with the NIDD code but NIDD off (no `THINGSPACE_ACCOUNT_NAME`), which changes nothing
observable; staging secrets, registration and tier 2; the device library and tier 3; production
last. No Postgres migration at any step.

## 16. Decisions for the owner

| # | Decision | Recommended answer | Alternative and its cost |
|---|---|---|---|
| D1 | What stops one account binding another's device | **The claim key** (4.4): built into the firmware, sent once per boot, gating uplink and downlink. No Postgres, no per-line operator work, and "refresh revokes" holds for NIDD | Operator assignment (section 9): JES assigns every line to an account by SQL before create. No firmware secret, and no 409 an account can squat, but SQL per line and a table to keep in step. Revisit before D2 opens NIDD to customers: the claim key stops data crossing, but an open create would let any account probe and squat IMEIs, which v1's org allowlist (4.6) prevents only while NIDD is JES's alone |
| D2 | Whose ThingSpace account, and for whom | **JES's one account, for JES's own devices, until counsel answers** whether running customer lines on it is making "the Services available to any third party" (https://thingspace.verizon.com/legal/terms-of-service.html); and before the first customer line, whether Verizon joins `docs/legal/subprocessors.md` with the DPA's advance notice. Until then `NIDD_ALLOWED_ORG_IDS` lists JES's own organizations only | Customers bring their own ThingSpace accounts: a per-organization credential store and settings form dovecote does not have, a feature of its own |
| D3 | The SDK, and the licence statement | **Depend on the patched SDK**, pinned by revision, accepted per crate in `about.toml`; and **state pidgeiot's licence explicitly** in `README.md:80` and each crate's `license` field. `AGPL-3.0-only` matches what the dovecote binary effectively is once it links the SDK; "or later" also works | Vendor the three calls (about 150 lines over `worker::Fetch`): no new crates, no licence table, but a fork of the owner's client |
| D4 | Displace whatever holds `NiddService` today | **Yes**, once B1 has named the holder and task 0.5 is done. The 2023 middleware and the SDK's example worker are retired in intent, but the example worker is still deployed and public, with unauthenticated routes that read the listener password and send to any line (task 0.5) | Keep it: then NIDD uplink cannot reach dovecote at all, since Verizon allows one endpoint per service per account |
| D5 | A second UWS user for staging and dev | **Yes, if the account allows one**: then no staging mistake can spend production's lockout budget | One shared user: the latch still caps it at one strike per environment, two or three of Verizon's five |
| D6 | The SIM plan for field units | **NIDD with IP data**, if Verizon sells it: the IP PDN carries HTTPS firmware download, and B9 proved it works beside the Non-IP PDN. The bench SIM's plan carries IP data, but 250 KB a month, less than one signed application image (319 to 489 KB), so field plans need a larger IP allowance for firmware | NIDD only: no remote firmware path at all; a bad build is a site visit |
| D7 | Downlink delivery window (`maximumDeliveryTime`) | **30 seconds**, for pushes and replies alike. A push that misses the device's connection is not worth holding: nothing buffered was seen delivered later (B8's `Queued` push failed 30 minutes on; the B7 retest's, sent with 86400 s, reached none of the device's next three connections in 31 minutes), and the device's next uplink brings it as a reply. A reply lands 0.5 to 4 s after the send call | Longer holds nothing that has been seen to arrive, and lets a stale `SHADOW` land after a newer one (harmless, 7.4). A day-long window with a day-long re-push, as first designed, left a device that sends only telemetry waiting a day for its config |
| D8 | Confirm every converged shadow report with `STATUS STORED` | **Yes**: one extra downlink per shadow change keeps the library's rule that a report is the one confirmed call | No reply when converged: saves that downlink, and the device can no longer tell a stored report from a lost one |
| D9 | Price NIDD | **No downlink metering in v1**, NIDD kept off the pricing and marketing pages until the carrier price is known, and the "every transport in the free tier" promise (`fancier/src/views/pricing.rs:544`) reviewed before NIDD is listed. Downlinks per organization are logged, so the decision will have data | Meter downlinks now, against a carrier price nobody has seen |
| D10 | The departure board on NIDD | **No**: it stays on LTE-M IP (14.4). NIDD is for low-duty sensors and, possibly, an e-paper variant on a core Verizon supports, which the nRF9151 is not | Pursue it: a Verizon exception to the 4-an-hour guideline, an NB-IoT build, and an application-data downlink the platform does not have |
| D11 | An automated check that `NiddService` still points at us | **Not in v1**: the runbook's step 2 at every deploy. Revisit when a paying NIDD device exists | Hourly from the existing cron: about 72 ThingSpace calls a day per environment, and the environment that does not hold the listener reads "drifted" forever |
| D12 | Forged uplink by someone holding the listener password and posting from a Verizon address | **Accept for v1, with the line pin**: against such a forger the password is the only uplink secret (4.2), every API-credential holder can read it back [LIST], and it is rotated on any suspicion (8.4). They could store readings and shadow reports in a claimed pigeon whose IMEI and ICCID they know; they could never steer the device, whose downlink is signed | An uplink MAC keyed by the claim key plus a device sequence number on every frame: 12 more bytes an uplink and a replay window in `pigeon_nidd`, after which the claim key alone authenticates uplink and the listener password is only a filter |
| D13 | A push to a device holding no connection | **Implemented: any uplink from a device that is behind draws the owed `SHADOW` as its reply**, sent while the connection that uplink opened is still up (`shadow_reply_due`, 6.4); a `DeliveryFailed` or `Queued` report marks the push pending, and a push unconfirmed 30 s after it went out counts as lapsed. B7 retest, PSM and eDRX off: two unsolicited pushes to the RRC-idle device were paged 12.6 and 13.8 s after the send began, after ThingSpace had reported `DeliveryFailed` "Backend service error" at 10.4 s and `Queued` "Buffered, device not reachable" at 11.1 s; the paged connections carried nothing, and the buffered push reached none of the next three connections. The third push, made while a telemetry uplink held the connection, arrived 2.77 s after the send began, and every reply to an uplink arrived whole with its tag verifying. B8: a push sent while the device slept went `Queued` and failed 30 minutes later | Keep a day-long re-push: a device that sends only telemetry waits a day for its config, or its next shadow report |

## 17. Numbers

Order of magnitude, not precision. Cloudflare unit prices at the overage rates, read 2026-09-24:
Durable Objects $0.15 per million requests, $1.00 per million SQLite rows written, $0.001 per
million rows read, $12.50 per million GB-s
(https://developers.cloudflare.com/durable-objects/platform/pricing/); Workers $0.30 per million
requests and $0.02 per million CPU ms (https://developers.cloudflare.com/workers/platform/pricing/);
Queues $0.40 per million operations, three per message
(https://developers.cloudflare.com/queues/platform/pricing/). Hyperdrive has no per-query charge on
Workers Paid (https://developers.cloudflare.com/hyperdrive/platform/pricing/, read by the cost
review on 2026-09-24). The arithmetic is `nidd/synth/cost.py` in the job directory.

**Per uplink, steady state:** one Worker request (the callback), one Durable Object request, three
SQLite rows read and two written (`pigeon_nidd` read twice, the telemetry blob once, each written
once), one queue message, one Postgres query at the gateway (the fuse, uncached, section 9), and
the queue consumer's existing history insert, billing tally and alert lookup. No downlink and no
ThingSpace call.

**Per device-day on Cloudflare** (DO duration estimated at 100 ms active at 128 MB per uplink,
CPU at 10 ms per uplink including the consumer; both estimates):

| Cadence | Uplinks | Billable readings | Cloudflare cost per device-day | Per device-month |
|---|---:|---:|---:|---:|
| Readings every 5 minutes, sent every 15 (the guideline's ceiling) | 96 | 288 | $0.00039 | $0.012 |
| One reading an hour | 24 | 24 | $0.00010 | $0.003 |
| Readings every 5 minutes, unbatched (outside the guideline, for comparison) | 288 | 288 | $0.00115 | $0.035 |

At the first cadence, rows written ($0.00019) and queue operations ($0.00012) are most of it.
Inside the included allowances (10 million Worker requests, 1 million DO requests, 50 million rows
written and 1 million queue operations a month) all of it is zero until the fleet is in the
thousands. Billing counts readings, so batching cuts our cost without cutting the customer's bill.

**Why the fuse sits at the gateway.** An active outbound connection "keeps a Durable Object in
memory and causes it to incur duration charges for up to 15 minutes per connection"
(https://developers.cloudflare.com/durable-objects/platform/pricing/). If a Hyperdrive connection
opened from the pigeon's DO counts, an in-DO fuse on a device waking every 15 minutes would keep the
object billed continuously: at most about 324,000 GB-s, some $4 per device-month, against the $0.012
above; 15 minutes is a ceiling, so this is an upper bound. The gateway is a Worker, billed by CPU
time, so the question never arises for the uplink path. The shadow-report tail still opens one from
the DO, a few times per shadow change.

**Downlinks per event:** 0 per steady-state uplink; 2 per shadow change (the push, then
`STATUS STORED`), 3 when the push misses and the next uplink's reply carries it; 1 per boot (the
`HELLO` reply); at most 1 an hour while paused or unclaimed; at most 1 per uplink for a device
that never converges, inside the connection that uplink opened. Each downlink is one ThingSpace
`send_nidd`, plus an OAuth token at most hourly and a session login after 15 idle minutes. NIDD
carrier pricing is not published on any page read [NIDD][SEND][CB].

**Radio accesses an hour**, the number Verizon's guideline limits to four [NUG]: 3 at the sample's
20-minute wake, leaving one for a paged push; 4 at the 15-minute floor, where a push's page goes
over briefly and a reply, riding its uplink's connection, does not; 120 for a departure board at
its 30-second default.

**Bytes per frame** (section 7): a four-key batch of three readings 294; ten keys, one reading 188
batched or 149 flat; a shadow report 78; a `HELLO` 33; a `SHADOW` 58 to 1358 with its tag, 179 with
a firmware target; a `STATUS` 21 to 30. Base64 adds a third on the ThingSpace legs: a callback is
564 bytes plus the frame's base64 (B5), 2264 around a 1273-byte uplink. On the radio, a device at
the first cadence with ten keys (540 bytes a wake) sends about 52 KB a day.

## 18. UNVERIFIED and sources

### UNVERIFIED

- **U1.** Any callback acknowledgement deadline. The brief's 2 seconds appears on none of [CB],
  [CBBP], [REG], [SEND] or [NIDD]; [CB] was re-read live on 2026-09-24 and states none. The web
  search budget was exhausted before a wider search. The design answers fast anyway and logs `ms=`.
- **U2.** Settled by B5: `POST`, `application/json`, `Verizon's callback service`, never
  challenged by Browser Integrity Check in 62 requests.
- **U3.** Partly settled. B5: four identical `HELLO`s from four wakes got four distinct request
  ids, so no device sequence byte is needed. B6: a refused callback draws one retry 1.2 to 1.7 s
  later and no five-minute resends, so backdating by attempt was dropped. Tier 3: a callback still
  unanswered after about 4 s is retried, with `callbackCount` 2, while the first attempt runs on
  (13.3). Still unknown: whether a refused callback's retry carries `callbackCount` 2 (the refusal
  lines now log it, 5.1 step 4).
- **U4.** Settled by B4 and B7 for one modem: an nRF9160 on mfw 1.3.7 accepts a 1273-byte uplink
  and refuses 1283 in `send()`, 1274 to 1282 untested; ThingSpace counts 1358 before base64 and
  refuses 1359 (`TooLong`). Other modems and firmware are unmeasured.
- **U5.** The IMEI's form in callbacks (15 or 16 digits), the `kind` spelling, and which kind a
  delivery report's top-level `deviceIds` carries when the downlink was sent by IMEI (B5, B7).
- **U6.** Which fields Verizon's OAuth response carries; the HTTP status for a wrong UWS password;
  the status and code for an expired session; whether a new session login invalidates older
  session tokens (which decides whether staging and production on one UWS user fight each other);
  whether `expires_in` counts down on a repeat OAuth request inside the hour.
- **U7.** Whether registering `NiddService` replaces an existing registration or errors; the runbook
  deregisters first either way. Whether an uplink sent while no listener is registered (between
  steps 4 and 5) is archived or lost, and whether a resend carries the password it was first sent
  with (B6's rotation variant, 8.4). Whether a SOAP registration holds `NiddService` on this
  account, which [LIST] cannot show. B6 settled two: deregister-then-register works, and an uplink
  sent with no listener is lost. Which password the stale callbacks carried is unlogged; the
  `password=previous` line settles it at the next rotation.
- **U8.** Whether the account allows a second UWS user (D5), or a second account for staging.
- **U9.** That the installed cargo-about honours a per-crate `accepted` table for a hyphenated
  crate name; its documentation says so, and task 1.8 settles it by running the script.
- **U10.** That an id from `id_from_name` round-trips through `id_from_string` in the existing route
  macro (`dovecote/src/lib.rs:390`); both are `ObjectId` in worker 0.8.6, and tier 1 step 3 proves
  it.
- **U11.** Settled for the bench (B1 to B3): the line carries NIDD and 250 KB a month of IP data,
  and Verizon NB-IoT serves the bench on band 13. Coverage at Massachusetts sites is still
  unknown. The only
  claim on hand about US NB-IoT is an unsourced commit message in the departure board's history.
- **U12.** That any version of `/home/justin/pigeon-nidd/nidd-test` ever ran: no capture exists,
  and the tree on disk does not build against the NCS it pins (reader map `device-and-origin.md`
  1.5).
- **U13.** Settled by B8, the B7 retest and tier 3: `NRF_RAI_NO_DATA` is accepted on a raw socket,
  and a downlink ThingSpace buffered was never delivered later, neither at B8's next wake nor,
  sent with an 86400 s `maximumDeliveryTime`, at any connection in the day before it reported
  `DeliveryFailed` (D13, 13.3). Paging an RRC-idle device took 12.6 and 13.8 s in the B7 retest,
  after ThingSpace's roughly 10 s attempt, and 2.2 and 3.8 s in tier 3, inside it; what decides
  which is unmeasured.
- **U14.** Whether a Hyperdrive connection opened from a Durable Object keeps it billed for up to 15
  minutes (section 17); the design keeps it off the uplink path either way.
- **U15.** NIDD carrier pricing, per message or per byte.
- **U16.** That Verizon's eight callback addresses are complete and current. They are Amazon and
  Microsoft cloud addresses (4.2), so one Verizon releases could be assigned to an unrelated party,
  whom the account gate does not stop; whether any has already been released is unknown.
- **U17.** Whether running customer lines on JES's account is permitted under the Terms (D2): owner
  and counsel, not engineering.
- **U18.** None seen (B3, B10: twelve frames in six minutes, no drop, no rate-control event).
- **U19.** The sizes of a departure payload (section 14.4) are an estimate, not a measurement.
- **U20.** Whether the deployed `thingspace-sdk` example worker holds live ThingSpace credentials
  (its send route answered like the `api` build at `9d920a4` on 2026-09-24; nothing that would
  use a credential was called), and whether the 2023 middleware still runs anywhere with
  credentials. Task 0.5 settles both.
- **U21.** Whether the IMEI in a callback is what the modem reported or the line's provisioned
  record. B5 settled the other half: the inner `deviceIds` carry the ICCID, and the line pin took
  it.
- **U22.** Whether the ThingSpace portal can send NIDD messages, and which portal roles can:
  another holder of downlink, harmless to a device without the claim key's tag.

### Sources read on 2026-09-24

Verizon ThingSpace:

- [NIDD] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-non-ip-data-delivery.html
- [SEND] https://thingspace.verizon.com/documentation/apis/connectivity-management/api-reference/send-nidd-to-devices.html
- [CB] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-callback-services.html
- [CBBP] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-callback-services/best-practices.html
- [REG] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Register_Callback_Listener/
- [LIST] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/List_Callback_Listeners/
- [DEREG] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Deregister_Callback_Listener/
- [LOGIN] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Start_Connectivity_Management_Session/
- [CRED] https://thingspace.verizon.com/documentation/apis/connectivity-management/getting-started/getting-credentials.html
- [ERR] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Synchronous_Error_Messages/
- [NUG] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/network-usage-guidelines.html
- [REACH] https://thingspace.verizon.com/documentation/apis/connectivity-management/working-with-verizon/about-device-reachability.html
- [ACT] https://thingspace.verizon.com/resources/documentation/connectivity/API_Reference/Activate_Devices/
- [TOS] https://thingspace.verizon.com/legal/terms-of-service.html

Cloudflare, Nordic and tooling:

- https://developers.cloudflare.com/durable-objects/api/state/
- https://developers.cloudflare.com/durable-objects/platform/pricing/
- https://developers.cloudflare.com/workers/platform/pricing/
- https://developers.cloudflare.com/queues/platform/pricing/
- https://developers.cloudflare.com/hyperdrive/concepts/query-caching/
- https://developers.cloudflare.com/hyperdrive/platform/pricing/
- https://github.com/EmbarkStudios/cargo-about/blob/main/docs/src/cli/generate/config.md
- https://github.com/nrfconnect/sdk-nrfxlib/blob/main/nrf_modem/doc/sockets/raw_sockets.rst
- https://github.com/nrfconnect/sdk-nrf/blob/main/samples/cellular/nidd/README.rst

Also read on 2026-09-24:

- https://rdap.arin.net/registry/ip/3.87.163.45 and https://rdap.arin.net/registry/ip/137.117.33.109
  (two of Verizon's eight callback addresses: Amazon's `AMAZON-IAD`, Microsoft's `MICROSOFT`)
- https://thingspace-sdk.justinsengineeringservices.workers.dev/ and its `/api/send_nidd`, probed
  by the security review without any credential (task 0.5)

[NIDD], [SEND], [CB], [CBBP], [REG], [LOGIN], [CRED], [NUG], the Durable Object state and pricing
pages, the Workers, Queues and Hyperdrive query-caching pages and the cargo-about page were read
by this document's writer, and [DEREG] and the two RDAP records by its fix pass; the rest by the
reader, review and skeptic agents the same day, whose saved
copies of the Verizon pages sit under `nidd/understand/vz/` in the job directory.

Internal: pidgeiot at `fcc093c` (every `file:line` above), `thingspace-sdk-rust` at `9d920a4`,
`~/pigeon` at `fd81344`, NCS v3.4.0 at `/home/justin/pigeon-examples-ncs`,
`/home/justin/pigeon-nidd/nidd-test`, `/home/justin/embedded-departure-board`, and
`justins-engineering/dusty-loft` for the origin.
