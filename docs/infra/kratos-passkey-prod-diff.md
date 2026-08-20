# Production Kratos: enable passkeys (owner-applied)

Production Kratos config changes are applied by the repo owner, never by
tooling. This document is the exact diff for the production config
(`.migration/kratos.prod.yml` in this repo, `/opt/kratos/kratos.yml` on the
VPS) plus the apply and verify steps. The dev config
(`schemas/kratos/kratos.yml`) already carries the equivalent change.

## Why prod needs no origin rework

Dev had to move its browsing origin to `localhost` because WebAuthn refuses
an IP-literal RP id. Production already runs on real domains, so nothing
about serving changes:

- The WebAuthn ceremony runs on the **dashboard page** (`https://pidgeiot.com`),
  not on `auth.pidgeiot.com`. The RP id must be a registrable suffix of (or
  equal to) the page's host, so it is `pidgeiot.com`.
- `origins` must list the exact page origin the ceremony runs on:
  `https://pidgeiot.com`. `auth.pidgeiot.com` is not a ceremony origin; it
  only serves the flow API and `webauthn.js` (a script node fancier now
  injects; script loading is not CORS-bound, and Kratos ships SRI/crossorigin
  attributes which fancier forwards).
- Cookies and CORS already cover both hosts (`cookies.domain: pidgeiot.com`;
  `https://pidgeiot.com` is in `allowed_origins`).
- The identity schema needs no change: `identity.user.schema.json` (both dev
  and the VPS copy) already maps `passkey.display_name` and
  `webauthn.identifier` onto the email trait. Confirm on the VPS with:
  `grep -A2 '"passkey"' /opt/kratos/identity.user.schema.json`

One durable consequence: enrolled passkeys are bound to the RP id. If
`rp.id` ever changes later, every enrolled passkey is orphaned. `pidgeiot.com`
is the stable choice.

## The diff

Against `.migration/kratos.prod.yml` (`selfservice.methods`):

```diff
   methods:
     password:
       enabled: true
+    passkey:
+      enabled: true
+      config:
+        rp:
+          display_name: PidgeIoT
+          id: pidgeiot.com
+          origins:
+            - https://pidgeiot.com
     totp:
```

## Considered and deferred: the separate `webauthn` method

Kratos's `webauthn` method (security keys as a *second* factor) is a
different method from `passkey` and stays disabled. TOTP + lookup secrets
already cover second factors, and enabling `webauthn` would add another
near-duplicate section to the settings page. Revisit only if a customer
asks for hardware-key 2FA specifically.

## Owner apply steps

1. Update `.migration/kratos.prod.yml` in this repo first (it is the
   canonical copy the VPS file is provisioned from), then mirror the same
   edit into `/opt/kratos/kratos.yml` on the VPS.
2. Restart the container so it re-reads the bind-mounted config, keeping
   every existing flag (notably `--watch-courier`, see the mail-queueing
   gotcha in CLAUDE.md):

   ```sh
   sudo docker restart kratos
   ```

3. Health check from the VPS (Kratos binds loopback only):

   ```sh
   curl -fsS http://127.0.0.1:4433/health/ready
   ```

4. Verify live from a real browser on `https://pidgeiot.com`:
   - Settings shows a "Passkey" section; enroll a passkey with a real
     authenticator (platform or key).
   - Sign out, then sign in with that passkey end to end.
   - Password sign-in still works.

5. Rollback: revert the config edit and `sudo docker restart kratos`.
   Passkeys enrolled while enabled simply stop being offered; password
   credentials are untouched.

Restarting Kratos drops in-flight self-service flows (a user mid-login
retries); established sessions live in Postgres and survive.
