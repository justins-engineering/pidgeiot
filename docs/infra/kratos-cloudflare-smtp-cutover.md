# Kratos courier: useSend → Cloudflare Email Service

Production Kratos sends verification, recovery and settings mail through
useSend over Amazon SES. Cloudflare Email Service offers the same thing as
authenticated SMTP submission on an account we already pay for, and removes
both useSend and AWS SES from the subprocessor chain.

Kratos consumes SMTP as one connection URI from the process environment, so
the cutover is **one value in `/opt/kratos/.env` plus a restart**. No DNS
change, no config-file edit, no migration. The DNS that makes it work is
already published (see Preconditions) and authorizes both providers at once,
which is what makes rollback a config revert rather than a DNS wait.

Everything here is applied on the VPS (`debian@15.204.254.3`) by the owner.
Background and the phase plan live in the #65 scoping report
(`~/pidgeiot-business/cf-email-migration-2026-09/scoping-report.md`).

## Status: not yet run

This is Phase 4, and it goes last, alone, in its own window. Phase 3 —
rehearsing the same courier path on the dev stack against the real Cloudflare
SMTP endpoint — is the gate, and its record belongs in the Rehearsal section
at the bottom before anyone runs the steps here.

## What changes

| | |
|---|---|
| `/opt/kratos/.env` | `COURIER_SMTP_CONNECTION_URI` replaced; old line kept commented |
| `/opt/kratos/.env` | `COURIER_SMTP_FROM_ADDRESS` added, only if the current sender is off-domain |
| `kratos.service` | restarted; unit file itself unchanged |

Nothing else. `kratos.yml`, the identity schema, the courier templates, the
tunnel and every DNS record stay as they are.

## The connection URI

```
smtps://api_token:<CLOUDFLARE_EMAIL_SMTP_TOKEN>@smtp.mx.cloudflare.net:465/
```

- `smtps` is **implicit TLS** with certificate verification, which is what
  port 465 speaks. This is from the pinned version's own config schema
  (`embedx/config.schema.json`, v26.2.0): the scheme alone decides, and there
  is no `legacy_ssl` parameter in this version. Do not add `skip_ssl_verify`.
- The username is the literal string `api_token`. It is not a placeholder.
- The password is a Cloudflare API token scoped **Email Sending: Edit**,
  stored in the gitignored `secrets.env` as `CLOUDFLARE_EMAIL_SMTP_TOKEN`.
- The token sits in the URI's userinfo, so it must be percent-encoded if it
  carries any character outside `A-Za-z0-9._~-`. Check before pasting; a
  reserved byte truncates the password silently rather than erroring.

## Sender address

Use **`account@noreply.pidgeiot.com`**. Cloudflare refuses a `From` outside an
onboarded sending domain, and the one-sending-domain plan keeps every
transactional sender — Kratos courier and dovecote alerts alike — on
`noreply.pidgeiot.com`, whose DKIM key signs with `d=noreply.pidgeiot.com` and
therefore aligns strictly under DMARC.

`/opt/kratos/kratos.yml` is not world-readable, so read the current
`courier.smtp.from_address` with sudo as step 0. If it already names an
address on `noreply.pidgeiot.com`, change nothing — the domain is onboarded
and the existing value keeps working. Only set `COURIER_SMTP_FROM_ADDRESS` if
it does not, because Kratos's built-in default (`no-reply@ory.kratos.sh`) and
any address on another domain are both rejected at submission.

## Preconditions

DNS, verified 2026-09-01 — all already published, nothing to add:

- `cf-bounce._domainkey.noreply.pidgeiot.com` — `v=DKIM1; k=rsa`, **2048-bit**
  modulus, which meets the acceptance gate. Cloudflare publishes one
  account-level key: this is byte-identical to the key on
  `noreply-staging.pidgeiot.com`, so a rotation would move both domains at
  once.
- `cf-bounce.noreply.pidgeiot.com` — `v=spf1 include:_spf.mx.cloudflare.net
  ~all`. Cloudflare's Return-Path lives on this subdomain, which is why
  onboarding never touched the `include:amazonses.com` record on
  `noreply.pidgeiot.com` itself.
- `_dmarc.noreply.pidgeiot.com` — `v=DMARC1; p=reject;`, created by
  onboarding. See the hazard note below.

Also required before starting:

- Read the **account daily sending quota** in the Cloudflare dashboard. It is
  deliberately undocumented, over-quota sends are rejected for retry, and
  Kratos's own retry is what would turn that into delayed signups rather than
  a visible failure.
- The Phase 3 rehearsal has passed and its record is filled in below.

### The p=reject hazard, and why the dual run is still safe

Onboarding a sending domain makes Cloudflare publish `_dmarc.<domain>` with
`p=reject` and **no `rua=`**. On `noreply.pidgeiot.com` that replaced the
organizational fallback to the apex's `p=none`, so every message from the
domain is now subject to rejection at receivers, including the mail the
current useSend/SES path is still sending.

That path survives it: `noreply.pidgeiot.com` is configured as an SES custom
MAIL FROM domain (`MX 10 feedback-smtp.us-east-1.amazonses.com` plus the
`include:amazonses.com` SPF record), so SES mail's envelope-from is on the
same domain as its `From` and SPF is **strictly aligned**. DMARC passes on the
SPF identifier without needing DKIM. Cloudflare's mail passes the other way
round — DKIM `d=noreply.pidgeiot.com` aligned strictly, SPF authorized under
`cf-bounce.` and aligned relaxed — which is exactly the verdict Email Routing
recorded on the staging probe (`dmarc=pass header.from=noreply-staging.
pidgeiot.com policy.dmarc=reject`).

So both providers pass under `p=reject` and either may send during the
overlap. Two consequences worth carrying forward: **do not remove the
`include:amazonses.com` record or the `MX` on `noreply.pidgeiot.com` while
rollback is still a live path** — that pair is the whole reason the old sender
still authenticates — and the missing `rua=` means nothing reports what
`p=reject` is actually rejecting. Adding `rua` to the two new records is
Phase 5 work, and it is now more urgent than the apex hardening that phase was
originally about.

## Procedure

Run as the owner on the VPS.

1. **Read the current sender**, so the from-address decision above is made on
   fact rather than assumption:

   ```sh
   sudo grep -nE 'from_address|from_name|local_name' /opt/kratos/kratos.yml
   ```

2. **Snapshot the env file** beside itself, same owner and mode:

   ```sh
   sudo cp -a /opt/kratos/.env /opt/kratos/.env.pre-cloudflare
   ```

3. **Edit `/opt/kratos/.env`** (root:root 0600 — keep it that way):
   - comment out the existing `COURIER_SMTP_CONNECTION_URI=` line rather than
     deleting it; that commented line *is* the rollback,
   - add the new `COURIER_SMTP_CONNECTION_URI=` per the URI section,
   - add `COURIER_SMTP_FROM_ADDRESS=account@noreply.pidgeiot.com` only if
     step 1 showed an off-domain sender.

   `EnvironmentFile=` does no shell parsing: no quoting, no `export`, no
   interpolation. The value is the rest of the line verbatim, `&` and `?`
   included.

4. **Restart and watch the courier start:**

   ```sh
   sudo systemctl restart kratos
   systemctl is-active kratos
   journalctl -u kratos -n 30 --no-pager | grep -i courier
   curl -fsS http://127.0.0.1:4433/health/ready
   ```

   Expect `Courier worker started.` in the journal and `{"status":"ok"}` from
   the health endpoint. A courier that never announces itself is the
   `--watch-courier` failure below, not a slow start.

## Verification

Run all of it before calling the window closed.

1. **Send one real email through a recovery flow**, not a registration:
   recovery exercises the identical courier path without minting an identity,
   and identity IDs are load-bearing here (gotcha 2 in `CLAUDE.md`). Start it
   at <https://pidgeiot.com/recovery> for a test identity whose mailbox you
   can read. The code should arrive within about a minute.

2. **Confirm from the queue's own state** — this is the `--watch-courier`
   proof, and it is the check that would have caught the original queued-mail
   outage:

   ```sh
   curl -fsS 'http://127.0.0.1:4434/admin/courier/messages?page_size=5'
   ```

   Expect the newest row at `"status":"sent"` with `send_count` ≥ 1. Rows
   stuck at `"queued"` with `send_count` 0 and no error anywhere else are the
   signature to rule out. A row that reached `"abandoned"` after several
   sends is the opposite failure — Cloudflare rejecting, most likely quota or
   an unonboarded `From`.

3. **Read the headers on the mail that arrived.** Required verdicts:
   `dkim=pass header.d=noreply.pidgeiot.com`, `dmarc=pass`, and a `From:` on
   `noreply.pidgeiot.com`. A `spf=none` for the header-From domain is
   expected and harmless — Cloudflare's SPF lives on the `cf-bounce.`
   Return-Path domain and DMARC passes on the DKIM identifier.

4. **Complete the flow with the code**, so the proof covers delivery of a
   usable message rather than merely dispatch.

5. **Cloudflare dashboard**: the send appears in the Email Sending activity
   log. There are no delivery webhooks, so this log and the courier queue are
   the whole observability surface.

6. **Sign-in sanity** at <https://pidgeiot.com>: flock and pigeon lists
   populate. A 200 with empty lists is the `cookies.domain` regression, not an
   empty account — unrelated to this change, but it is the cheap check that a
   restart broke nothing else.

## Rollback

Nothing in DNS changed and both providers remain authorized, so this restores
the exact prior world:

```sh
sudo cp -a /opt/kratos/.env.pre-cloudflare /opt/kratos/.env
sudo systemctl restart kratos
```

Then re-run verification steps 1 and 2 against the old provider. Keep
`.env.pre-cloudflare`, the `include:amazonses.com` SPF record and the SES
`MX` in place until well after the cutover has proven itself under real
signup volume.

## Standing gotchas

- **`--watch-courier` is load-bearing.** It is in `ExecStart`
  (`infra/systemd/kratos.service`) and must stay there. Without it a
  single-instance Kratos queues all outbound mail forever and logs nothing.
  It also means exactly one Kratos may run at a time.
- **Kratos's `local_name` defaults to `localhost`** in the HELO/EHLO command.
  If Cloudflare refuses the session at the greeting, `COURIER_SMTP_LOCAL_NAME`
  is the first thing to set; the rehearsal below records whether the default
  was accepted.
- **Over-quota sends are rejected for retry**, so the visible symptom is a
  courier row with a rising `send_count`, not an outage. Read the quota first.
- **`/opt/kratos/kratos.yml` is not world-readable**, so every inspection of
  it here needs sudo.

## Rehearsing on the dev stack

Production Kratos serves every deployed environment, so the rehearsal runs on
the dev docker-compose stack instead, pointed at the real Cloudflare endpoint
and sending from the already-onboarded `noreply-staging.pidgeiot.com` to
`staging-catch@pidgeiot.com`, which the mail catcher parks in KV for an agent
to read back ([`staging-mail-catcher.md`](staging-mail-catcher.md)).

The wiring is deliberately **not committed** — no config in this repo may
carry a live courier token or point dev at a real relay by default. Two local
files, both listed in `.git/info/exclude`:

- `infra/docker-compose.smtp-rehearsal.yml` — overrides the `kratos` service's
  environment only, interpolating `CLOUDFLARE_EMAIL_SMTP_TOKEN` from the
  invoking shell so the value is never written to disk.
- `infra/smtp-rehearsal-test.py` — drives a verification flow end to end and
  prints the evidence this runbook's Rehearsal record wants.

```sh
set -a; . ./secrets.env; set +a
docker-compose -f infra/docker-compose.yml \
  -f infra/docker-compose.smtp-rehearsal.yml up -d --force-recreate kratos
python3 infra/smtp-rehearsal-test.py
```

Only the `kratos` container is recreated, so a concurrent dev session keeps
its database and loses nothing but MailSlurper capture for the duration.
`docker-compose -f infra/docker-compose.yml up -d --force-recreate kratos`
puts it back.

## Rehearsal record (Phase 3)

Fill in from the dev-stack rehearsal before running the procedure above:
courier row status and `send_count`, catcher round-trip latency, the
`Authentication-Results` verdicts on the caught MIME, whether the flow
completed from a code read out of stored mail, and anything that had to
differ from what this runbook says.
