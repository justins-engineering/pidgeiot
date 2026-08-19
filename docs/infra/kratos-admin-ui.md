# Kratos Admin UI on the VPS

A read/write web console for the production Kratos identity store, running on
the same VPS as Kratos itself, reachable only through the existing Cloudflare
Tunnel with Cloudflare Access in front of it. It replaces the "SSH in and
`curl` the admin API" loop that identity work has needed until now (the
identity import in `.migration/vps-bringup.sh` is the canonical example).
Scope: a break-glass tool for one administrator, not a support-staff surface —
see "Future: support staff" at the end for why that distinction is load-bearing.

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
route in either.) The remaining steps are therefore dashboard work.

**Do them in this order.** The Access application must exist before the
hostname resolves, because publishing a route on a tunnel creates its DNS
record immediately — reversing the order publishes an unauthenticated console
over every identity in production, for however long the gap lasts.

Cloudflare has been renaming these dashboard sections (Access moved under
"Access controls", tunnel "public hostnames" are now "published application
routes"). Breadcrumbs below match the docs as of this writing; if the wording
has drifted again, the landmarks are the Applications list, the Policies page's
rule-group tab, and the tunnel's own routes tab.

### Step 1 — create a reusable rule group (do this first)

Zero Trust → **Access controls → Policies**, **Rule groups** tab → create:

- Name: `PidgeIoT Admins`
- Include → **Emails** → `code@jes.contact` (verified identical to the address
  `KRATOS_EMAIL` uses; add `justin@jes.contact` if that second identity should
  reach it too)

A group rather than an email typed straight into the policy. Same result today
with one person in it, but membership then lives in one named object that other
applications can reference, so the next internal tool is a one-line reuse
instead of another copy of an email list to keep in sync. See "Future: support
staff" below for the constraint that makes this worth doing now.

### Step 2 — create the Access application

Zero Trust → **Access controls → Applications** → add a **Self-hosted and
private** application:

- Application name: `Kratos Admin UI`
- Public hostname: subdomain `kratos-admin`, domain `pidgeiot.com`
- Policy: **Allow**, named `PidgeIoT admins only`, Include → **Rule groups** →
  `PidgeIoT Admins`
- Session duration: **24 hours**
- Leave the default identity provider (one-time PIN) unless an IdP is
  configured; save.

### Step 3 — publish the route (only after step 2 is saved)

**Networking → Tunnels** → the tunnel above → **Routes** tab → **Add route** →
**Published application**:

- Subdomain `kratos-admin`, domain `pidgeiot.com`, no path
- Service **HTTP**, URL `localhost:3000`

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

Should a scoped API token ever exist, the same steps are
`POST /accounts/{account_id}/access/groups`,
`POST /accounts/{account_id}/access/apps` (plus its `/policies`) and
`PUT /accounts/{account_id}/cfd_tunnel/0c9ebcda-4eec-46de-b2df-3143e57ee8df/configurations`
with the JSON above.

### Step 4 — verify from outside

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

## Future: support staff

This is a **break-glass tool for a single administrator**. It is worth being
explicit about why, because the obvious next request — "give support a way to
look up a customer" — must not be answered by widening the allow-list.

**The capability model is all-or-nothing.** There are no roles, scopes, or
read-only modes inside this UI. Anyone Access admits gets the full Kratos admin
API: read every identity's traits, change credentials, and permanently delete
identities. Deletion is the sharp edge — `flocks.user_id` and every DO-resident
`pigeon_acl` key are keyed on Kratos identity IDs (see the identity-remap
warning in `CLAUDE.md`), so removing an identity here strands that user's
flocks and pigeons in a way no undo in this console can repair. A support hire
who only ever needed to read an email address would be one misclick from that.
So: **adding a support person to `PidgeIoT Admins` is not the way to give them
customer visibility, now or later.** That group means "full production identity
administration", and should keep meaning exactly that.

The group indirection from step 1 is what keeps the door open cheaply. A future
support surface gets its *own* rule group and its *own* Access application; this
one stays a one-person group and never widens.

Two paths are on record for that surface, neither started here:

- **A role-aware support panel in `fancier`**, built on the RBAC dovecote
  already enforces. It is first-party, so the visible fields and the allowed
  mutations are chosen deliberately rather than inherited from whatever the
  admin API happens to expose, and it reuses the session model the dashboard
  already has. This is the direction currently queued.
- **Ory Keto (+ Hydra) for internal authorization**, which buys a real
  permission model off the shelf at the cost of another self-hosted service on
  the identity path.

Either way the design constraint is the same one this section exists to record:
support tooling needs a deliberately narrowed capability set, and this console
has none.
