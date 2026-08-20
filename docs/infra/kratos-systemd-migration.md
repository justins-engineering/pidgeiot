# Kratos: docker container → native binary under systemd

Production Kratos is the last docker-dependent piece of the auth path: loft
and cloudflared run as native systemd services, but `auth.pidgeiot.com` is an
`oryd/kratos:v26.2.0` container, which makes the docker daemon a runtime
dependency of production sign-in. The container is stateless — every identity,
session, and courier row lives in Crunchy Bridge Postgres, and all config is
bind-mounted from the host — so moving to a native binary is a process swap,
not a data migration.

Everything here is applied on the VPS (`debian@15.204.254.3`) by the owner.
The unit file ships in this repo at [`infra/systemd/kratos.service`](../../infra/systemd/kratos.service).

## What runs today (verified against the live VPS)

The container was created by `.migration/vps-bringup.sh` and `docker inspect`
confirms it still matches that script exactly:

- Image `oryd/kratos:v26.2.0`, entrypoint `kratos`, command
  `serve -c /opt/kratos/kratos.yml --watch-courier`, `--network host`,
  restart policy `unless-stopped`, container uid 10000.
- Environment: `DSN` and `COURIER_SMTP_CONNECTION_URI` (values injected from
  `/opt/kratos/.env`, which stores the latter under the key
  `COURIER_SMTP_URI` — the docker run line renames it; step 2 removes that
  indirection).
- `/opt/kratos` bind-mounted read-only: `kratos.yml` (root:root 0644),
  `identity.user.schema.json`, `courier-templates/`, plus the 0600 `.env`.
- The image's declared volumes (`/home/ory`, `/var/lib/sqlite`) exist but
  hold nothing we use — the postgres DSN means no local state, confirming
  the swap is stateless.
- Listeners: `127.0.0.1:4433` (public) and `127.0.0.1:4434` (admin), both
  from `serve.public.host` / `serve.admin.host` in `kratos.yml`. The
  Cloudflare Tunnel targets these loopback ports directly, so **cloudflared
  needs no change at any point in this migration**.

## Step 1 — install the binary

The pinned release artifact and its checksum file both exist and have been
verified end-to-end from this repo (download, `sha256sum -c` OK,
`kratos version` prints v26.2.0). The binary is statically linked (pure Go,
no glibc dependency), and its build commit
`9d7085948039ffb8960160d4979f71527b5cf4d5` is byte-for-byte the same source
revision as the running image's `org.opencontainers.image.revision` label —
the native binary is the code already in production.

```sh
cd "$(mktemp -d)"
curl -fsSLO https://github.com/ory/kratos/releases/download/v26.2.0/kratos_26.2.0-linux_64bit.tar.gz
curl -fsSLO https://github.com/ory/kratos/releases/download/v26.2.0/checksums.txt
grep 'kratos_26.2.0-linux_64bit.tar.gz$' checksums.txt | sha256sum -c -
# expected: kratos_26.2.0-linux_64bit.tar.gz: OK — stop here if not
tar xzf kratos_26.2.0-linux_64bit.tar.gz kratos
sudo install -m 0755 -o root -g root kratos /usr/local/bin/kratos
/usr/local/bin/kratos version
# expected: Version v26.2.0, Build Commit 9d7085948039ffb8960160d4979f71527b5cf4d5
```

## Step 2 — reshape /opt/kratos/.env

The unit loads `/opt/kratos/.env` via `EnvironmentFile=`, which (unlike the
docker run line) cannot rename variables, and which loads **every** line into
the Kratos process environment. Two edits, values untouched:

1. Rename the key `COURIER_SMTP_URI` to `COURIER_SMTP_CONNECTION_URI` — the
   name Kratos itself consumes. One name, one source of truth, no remap layer.
2. Remove the `TUNNEL_TOKEN` line. It was only ever consumed once, by
   `cloudflared service install` at bring-up; cloudflared now runs with
   `--token-file /etc/cloudflared/token` and must not ride along in Kratos's
   environment. Confirm before deleting:

   ```sh
   grep token-file /etc/systemd/system/cloudflared.service   # expect: --token-file /etc/cloudflared/token
   sudo test -s /etc/cloudflared/token && echo token-file present
   ```

While editing: `sudo chown root:root /opt/kratos/.env` (keep mode 0600) —
systemd reads the file as root before dropping to the unit's dynamic user, so
nothing needs looser access, and root-only matches how loft's secret is held.
Format note: systemd env files are plain `KEY=value` lines, surrounding
quotes stripped, **no shell expansion** — eyeball that neither value relies
on shell syntax beyond simple quoting.

This deliberately breaks a rerun of `.migration/vps-bringup.sh`'s kratos
section (it sources the old key name); that script is the docker-era
bring-up and this runbook supersedes it.

## Step 3 — install the unit, but do not enable it yet

```sh
sudo install -m 0644 kratos.service /etc/systemd/system/kratos.service   # from infra/systemd/ in this repo
sudo systemctl daemon-reload
systemd-analyze verify kratos.service   # expect: no output
```

**Do not `enable` or `start` before cutover.** The container still holds
4433/4434, and an enabled unit plus the container's `unless-stopped` policy
would race each other at every boot. (The unit verifies clean from the repo
already; the only local-machine finding is the not-yet-installed binary
path, which step 1 resolves on the VPS.)

## Cutover

Expect a few seconds of auth downtime (login/registration/whoami); existing
dashboard sessions resume as soon as health returns. No `migrate sql` runs
here — same version 26.2.0 against the same database the container used.

```sh
sudo docker update --restart=no kratos   # rollback artifact must never self-start again
sudo docker stop kratos
sudo systemctl enable --now kratos.service
```

The stopped container remains on disk, config and image intact, as the
instant rollback path. Leave it there until well after the courier check has
passed.

## Verification checklist

1. Service state:

   ```sh
   systemctl status kratos --no-pager
   ```

   Expect `active (running)`, main process
   `/usr/local/bin/kratos serve -c /opt/kratos/kratos.yml --watch-courier`
   (sight-check the flag is present).

2. Listeners — same loopback binds as the container:

   ```sh
   sudo ss -ltnp | grep -E ':4433|:4434'
   ```

   Expect both `127.0.0.1:4433` and `127.0.0.1:4434` owned by process
   `kratos` (no docker-proxy, nothing on `0.0.0.0`).

3. Health, loopback then through the tunnel:

   ```sh
   curl -fsS http://127.0.0.1:4433/health/ready
   curl -fsS https://auth.pidgeiot.com/health/ready
   ```

   Expect `{"status":"ok"}` from both. The second passing proves the
   untouched tunnel is fronting the new process.

4. Config actually parsed — the login flow must offer the newly added
   passkey method:

   ```sh
   curl -fsS -H 'Accept: application/json' 'https://auth.pidgeiot.com/self-service/login/browser' | grep -o '"group":"passkey"' | sort -u
   ```

   Expect `"group":"passkey"`.

5. **The courier check — this is the `--watch-courier` proof and it sends
   one real email.** Register a fresh account with a reachable mailbox at
   <https://pidgeiot.com/registration>; the verification-code email must
   arrive within about a minute. Then confirm from the queue's own state:

   ```sh
   curl -fsS 'http://127.0.0.1:4434/admin/courier/messages?page_size=1'
   ```

   Expect the most recent message with `"status":"sent"` and `send_count`
   ≥ 1. The failure signature this exists to rule out: rows stuck at
   `"queued"` with `send_count` 0 and **no error anywhere else** — exactly
   what a missing `--watch-courier` looks like.

6. Journal and dashboard sanity:

   ```sh
   journalctl -u kratos -n 50 --no-pager
   ```

   No errors; then sign in at <https://pidgeiot.com> and confirm flock and
   pigeon lists populate (silently empty lists are the session-cookie
   regression signature, not a blank account).

## Rollback

Config and image are unchanged on disk, so this restores the exact prior
world:

```sh
sudo systemctl disable --now kratos.service
sudo docker update --restart=unless-stopped kratos
sudo docker start kratos
curl -fsS http://127.0.0.1:4433/health/ready   # expect {"status":"ok"}
```

If rollback happened because of the env-file edits in step 2, note the
container is immune to them only via the docker run line's own `-e` mapping —
it was created before the edits and carries its env internally, so a plain
`docker start` is safe. (Re-*creating* the container would need the old key
name back.)

## Upgrades

Same trust chain as the install: fetch the new tarball and `checksums.txt`
from the pinned release tag, `sha256sum -c`, keep the outgoing binary as
`/usr/local/bin/kratos.prev` (the same convention `loft`/`loft.prev` already
uses on this host).

- **Patch release (26.2.x)**: verify, `sudo cp /usr/local/bin/kratos
  /usr/local/bin/kratos.prev`, install the new binary, `sudo systemctl
  restart kratos`, run the health checks above.
- **New minor (26.3+)**: SQL migrations run manually, before the new
  version's first start — the same discipline the image workflow required:

  ```sh
  sudo systemctl stop kratos
  sudo bash -c 'set -a; . /opt/kratos/.env; set +a; /path/to/new/kratos migrate sql -e --yes -c /opt/kratos/kratos.yml'
  # install the new binary (keeping kratos.prev), then:
  sudo systemctl start kratos
  ```

  Migrations are forward-only: once a new minor has migrated the schema,
  `kratos.prev` is no longer a safe fallback. Read the release notes before
  any minor bump.

After this migration, the docker daemon serves only the kratos-admin-ui
unit's container (and the stopped kratos rollback artifact) — auth itself no
longer depends on it.

## Appendix: kratos-admin-ui, same principle, owner's pick

Current state (from the same VPS inspection): the admin UI is already
systemd-supervised — `/etc/systemd/system/kratos-admin-ui.service` wraps
`docker run --rm` around the digest-pinned image with `HOSTNAME=127.0.0.1`,
host networking (required to reach Kratos's loopback admin port), read-only
rootfs, and `--cap-drop ALL`, published only through the tunnel behind
Cloudflare Access. Details in [`kratos-admin-ui.md`](./kratos-admin-ui.md).
Three ways to hold it, no decision made here:

1. **Keep the supervised container (status quo).** The docker daemon stays a
   dependency, but after the Kratos cutover it is no longer in the
   auth-critical path — the UI is a break-glass tool whose outage blocks
   nothing, and the current setup already gets digest pinning, the image's
   filesystem containment, and systemd lifecycle/journal for free. The cost
   is conceptual, not operational: one daemon kept running for one
   convenience tool.

2. **Native bun/node under systemd.** Removes the docker daemon from the
   box's serving set entirely. Upstream ships no native artifact, so the
   Next.js standalone `server.js` and its assets would be extracted from the
   image (or built from source), trading away digest-pinned updates and the
   container's read-only filesystem for a hand-rolled install and a
   loft-style hardened unit — which must be re-derived for a JS runtime
   (`MemoryDenyWriteExecute` is off the table for a JIT), not copied from
   kratos.service.

3. **Plain docker restart-policy container (status quo ante).** Same daemon
   dependency as option 1 with weaker lifecycle handling (docker's restart
   policy instead of systemd supervision, `docker logs` instead of the
   journal). Listed for completeness; it is dominated by option 1.
