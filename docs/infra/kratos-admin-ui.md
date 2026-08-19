# Kratos Admin UI on the VPS

A read/write web console for the production Kratos identity store, running on
the same VPS as Kratos itself, reachable only through the existing Cloudflare
Tunnel with Cloudflare Access in front of it. It replaces the "SSH in and
`curl` the admin API" loop that identity work has needed until now (the
identity import in `.migration/vps-bringup.sh` is the canonical example).

The software is [`dhia-gharsallaoui/kratos-admin-ui`](https://github.com/dhia-gharsallaoui/kratos-admin-ui),
a Next.js app, pinned by digest:

    dhiagharsallaoui/kratos-admin-ui@sha256:eb0c21074664fa48f273235e2ac89e016937135a56586b4bf6714d2869cf67d7

which is the commit-SHA tag `64bff69be80ff4b2e406d057cb9493cee398d705`, the
same image `latest` pointed at when this was deployed. Pin by digest rather
than by tag: upstream publishes a moving `latest`, and this container holds
unauthenticated write access to every production identity.

## The one architectural fact that makes this safe

**The browser never talks to the Kratos admin API.** This matters enough to
state plainly, because the obvious alternative design is unshippable here:
Kratos's admin API on this host binds `127.0.0.1:4434` and is completely
unauthenticated — anyone who can reach that port can read, edit, or delete any
identity, with no credential at all. It can never be published.

This app's client code is compiled against relative paths — `/api/kratos-admin`
and `/api/kratos` (`src/services/kratos/config.ts` picks `clientConfig` whenever
`typeof window !== "undefined"`). Requests land back on the app's own origin,
where a Next.js middleware (`src/proxy.ts`, matching `/api/kratos-admin/:path*`)
strips the prefix and re-issues the request server-side against
`process.env.KRATOS_ADMIN_URL`. There is no `NEXT_PUBLIC_`-prefixed admin URL
anywhere in the app. So `KRATOS_ADMIN_URL=http://127.0.0.1:4434` is resolved
inside the container, and 4434 stays loopback-only.

That is the mode shipped here. No additional reverse proxy was needed.

**Corollary worth knowing:** the same middleware honours a `kratos-admin-url`
cookie and an `x-kratos-admin-url` request header as overrides ahead of the
environment variable. Anyone who can reach this app can therefore make its
server issue requests to an arbitrary URL of their choosing — a server-side
request forgery primitive, by design, since the UI is meant to be pointed at
whatever Kratos you like. It is acceptable here only because the sole route to
the app is through Access with a one-person allow-list. It is a second,
independent reason the hostname must never be published without Access.

## What runs where

Kratos already runs on this host in Docker with `--network host`, binding
`127.0.0.1:4433` (public) and `127.0.0.1:4434` (admin). A container on Docker's
default bridge network cannot reach a host's loopback interface, so the admin
UI uses `--network host` too — the same shape as Kratos, for the same reason.

Host networking means the container's listener *is* a host listener, so nothing
publishes it for us and nothing constrains it for us either. The image's
Dockerfile sets `ENV HOSTNAME="0.0.0.0"`; the unit overrides it to `127.0.0.1`.
**That single environment variable is the loopback binding.** Next.js's
standalone `server.js` passes it straight to `listen()`.

Unit: `/etc/systemd/system/kratos-admin-ui.service`, `enabled` (so it returns
after reboot) with `Restart=on-failure`. The container itself runs `--rm`, so
systemd owns its lifecycle rather than Docker's restart policy — matching how
`loft.service` and `pidgeiot-demo-feeder.service` are managed on this box,
rather than Kratos's bare `docker run --restart unless-stopped`. Sandboxing:
`--read-only` rootfs with tmpfs at `/tmp` and `/app/.next/cache`, `--cap-drop
ALL`, `--security-opt no-new-privileges`, `--memory=512m --cpus=1.0`. All of
that was verified working, not assumed — the app starts and serves under every
one of those restrictions.

## The loopback guarantee

Three independent layers, in order of how much they'd have to fail together:

1. The process binds `127.0.0.1:3000` only (`HOSTNAME=127.0.0.1`). Confirmed
   with `ss -lntp`: a single `127.0.0.1:3000` row, no `0.0.0.0` row.
2. The host firewall's `INPUT` policy is `DROP`, and no rule opens 3000. The
   accepted TCP ports are 22 and 5684.
3. The tunnel's ingress does not mention this hostname, and its catch-all rule
   is `http_status:404` — so even a DNS record pointed at the tunnel by mistake
   would 404 rather than reach the app.

Reachability from off-box is therefore Cloudflare Access, and only Cloudflare
Access.

## Cloudflare side: what is done and what needs the owner

The tunnel is **remotely managed**. It was installed token-first
(`cloudflared service install "$TUNNEL_TOKEN"`, per `.migration/vps-bringup.sh`),
the unit runs `tunnel run --token-file /etc/cloudflared/token`, `/etc/cloudflared`
holds only that token with no `config.yml`, and cloudflared's journal shows it
retrieving remote configuration. Ingress rules live in Cloudflare's control
plane, not on the box — nothing on the VPS can add a hostname.

Tunnel ID: `0c9ebcda-4eec-46de-b2df-3143e57ee8df`.

Changing that config needs a Cloudflare API token scoped for Access, DNS, and
Cloudflare Tunnel. No such token exists on the workstation — no
`CLOUDFLARE_API_TOKEN`/`CF_API_TOKEN` in the environment, no `CLOUDFLARE_*` or
`CF_*` entry in `secrets.env`, and no wrangler OAuth config. (`~/.cloudflared`
holds a 2025-vintage origin certificate and credentials for tunnel
`223d5b67-…`, a *different* tunnel from the one running here, so it is not a
route in either.) Both remaining steps are therefore dashboard work.

**Do them in this order.** The Access application must exist before the
hostname resolves, because adding a public hostname to a tunnel creates its DNS
record immediately — reversing these two steps publishes an unauthenticated
console over every identity in production, for however long the gap lasts.

### Step 1 — create the Access application (do this first)

Zero Trust dashboard → **Access → Applications → Add an application →
Self-hosted**.

- Application name: `Kratos Admin UI`
- Session duration: **24 hours**
- Public hostname: subdomain `kratos-admin`, domain `pidgeiot.com`
- Then **Add policy**:
  - Policy name: `Owner only`
  - Action: **Allow**
  - Include → **Emails** → `code@jes.contact` (the address `KRATOS_EMAIL`
    already uses; add `justin@jes.contact` too if the second identity should
    reach it)
- Leave the default identity provider (one-time PIN) unless an IdP is
  configured; save.

### Step 2 — publish the hostname (only after step 1 is saved)

Zero Trust dashboard → **Networks → Tunnels** → the tunnel above → **Public
Hostname → Add a public hostname**.

- Subdomain `kratos-admin`, domain `pidgeiot.com`
- Type **HTTP**, URL `localhost:3000`

That writes the proxied DNS `CNAME` automatically. The resulting ingress should
read as below — `auth.pidgeiot.com` first, the new rule second, catch-all last:

```json
{"ingress":[
  {"hostname":"auth.pidgeiot.com","originRequest":{"originServerName":"auth.pidgeiot.com"},"service":"http://localhost:4433"},
  {"hostname":"kratos-admin.pidgeiot.com","service":"http://localhost:3000"},
  {"service":"http_status:404"}
]}
```

No `originServerName` on the new rule: that override exists on the Kratos rule
because its origin serves TLS for that name. This origin is plain HTTP on
loopback.

Should a scoped API token ever exist, the same two steps are
`POST /accounts/{account_id}/access/apps` (plus its `/policies`) and
`PUT /accounts/{account_id}/cfd_tunnel/0c9ebcda-4eec-46de-b2df-3143e57ee8df/configurations`
with the JSON above.

### Step 3 — verify from outside

Once the hostname is live, from any machine:

```sh
curl -sI https://kratos-admin.pidgeiot.com/ | head -5
```

Expect a `302` to `https://<team>.cloudflareaccess.com/cdn-cgi/access/login/…`.
A `200` with HTML means Access is not in front of it — pull the public hostname
off the tunnel immediately, then fix the policy.

Then sign in in a browser as the allow-listed identity and confirm the identity
list renders. That last check is the owner's, since it needs a real Access
login.

## Operating it

```sh
systemctl status kratos-admin-ui          # state
journalctl -u kratos-admin-ui -f          # logs (container stdout)
sudo systemctl restart kratos-admin-ui    # restart

# health, from the VPS
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3000/
curl -s http://127.0.0.1:3000/api/kratos-admin/admin/identities?per_page=100 \
  | python3 -c 'import sys,json; print(len(json.load(sys.stdin)))'
```

The second command is the useful one: it exercises the whole path the browser
uses — app origin, middleware proxy, Kratos admin API — and prints the identity
count.

To update, pick a new commit-SHA tag from Docker Hub, resolve it to a digest
with `docker pull` + `docker inspect --format '{{index .RepoDigests 0}}'`,
replace the digest in the unit's `ExecStart` via
`sudo systemctl edit --stdin --force --full kratos-admin-ui.service`, then
`daemon-reload` and `restart`. Re-run the two health commands after.

Note that the UI renders a Hydra section (`hydraEnabled` defaults true) that
will fail against this host — no Hydra runs here. Harmless; ignore it.
