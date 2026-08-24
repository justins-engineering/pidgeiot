# status

The PidgeIoT status page: `status.pidgeiot.com`.

A separate Cloudflare Worker with its own KV namespace and no binding to
anything else we run. It exists to be up when the platform is down, which
is only true if it shares nothing with the platform: no Hyperdrive, no
Postgres, no Kratos, no call into dovecote or fancier at request time. The
only thing it reads to serve a page is its own KV.

There is no build step. The source is plain ESM, so `wrangler deploy`
uploads it as-is and a redeploy under pressure needs no toolchain.

```
status/
  wrangler.toml      production + [env.staging]
  src/index.mjs      routes and the cron handler
  src/probes.mjs     what gets checked, and the up/degraded/down ladder
  src/state.mjs      the KV document: current state, history, rollups
  src/incidents.mjs  reads the manually published incident updates
  src/render.mjs     the HTML page and /status.json
  test/              node --test status/test/status.test.mjs
```

## What it serves

| Path           | What                                                      |
| -------------- | --------------------------------------------------------- |
| `/`            | The status page, server-rendered, both colour schemes      |
| `/status.json` | The same data as JSON, CORS-open for external checkers     |
| `/health`      | Liveness of the status page itself, answered without KV    |

## What it checks

Every five minutes a cron fires three GETs. Each target is public,
unauthenticated, and already load-bearing for real traffic, so a green
result means something a customer would recognise:

| Surface | URL                                              | Why this one |
| ------- | ------------------------------------------------ | ------------ |
| `api`   | `api.pidgeiot.com/.well-known/api-catalog`        | dovecote's RFC 9727 catalog. Served from the router with no Hyperdrive or Durable Object behind it, so it is a clean edge-router liveness signal and cannot be slowed by the data plane. Probing an authenticated route would mean parking a device credential in this Worker, which is exactly the coupling this page must not have. |
| `auth`  | `auth.pidgeiot.com/health/ready`                  | Kratos's own readiness endpoint, through the Cloudflare Tunnel that fronts it, so the probe takes the same path a browser login does. |
| `site`  | `pidgeiot.com/`                                   | The prerendered landing page a signed-out visitor loads first. |

The ladder, which the page also explains to readers:

- **up** -- the check returned the expected status within two seconds.
- **degraded** -- it succeeded but took over two seconds, **or** it failed
  once and the retry succeeded.
- **down** -- two consecutive attempts failed. A single failure is always
  retried after a short pause before anything is published, so one dropped
  packet cannot put an outage on the page.

Uptime figures count degraded time as available. Degraded time is still
visible on the bar and in the per-surface line, so nothing is hidden by
that choice.

`api` going green does **not** assert that Postgres is healthy. That is
deliberate and is stated on the page: this signal is honest about the layer
it actually measures. A database problem is something the incident workflow
below exists to communicate.

## Publishing an incident

**This is the incident-publishing workflow.** There is no admin UI. An
incident is an append-only series of updates, one KV key each:

```
incident:<slug>:<iso8601-utc>
```

- `<slug>` is `YYYY-MM-DD-short-name`, e.g. `2026-08-24-api-latency`. It
  must not contain a colon, because the timestamp does and the key is split
  on the first one.
- `<iso8601-utc>` is exactly `date -u +%Y-%m-%dT%H:%M:%SZ`.

You never edit an update. To say something new, append another one. The
page groups updates by slug, orders them by timestamp, takes the current
status from the latest, and carries `title`, `severity` and `surfaces`
forward from the most recent update that stated them, so a follow-up only
has to say what changed.

Set the namespace once per shell:

```sh
# production
export STATUS_KV_ID=<production namespace id>
# staging
export STATUS_KV_ID=aafcb4c91d4d4472aa6723faad479ed2
```

### Open an incident

```sh
bunx wrangler kv key put --remote --namespace-id "$STATUS_KV_ID" \
  "incident:2026-08-24-api-latency:$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{"title":"Elevated API latency","severity":"major","status":"investigating","surfaces":["api"],"body":"Device requests to the API are slow or timing out. We are investigating."}'
```

### Post a follow-up

Only `status` and `body` are needed; everything else carries forward.

```sh
bunx wrangler kv key put --remote --namespace-id "$STATUS_KV_ID" \
  "incident:2026-08-24-api-latency:$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{"status":"identified","body":"A bad deploy of the edge router is the cause. Rolling back now."}'
```

```sh
bunx wrangler kv key put --remote --namespace-id "$STATUS_KV_ID" \
  "incident:2026-08-24-api-latency:$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{"status":"monitoring","body":"The rollback is live and latency is back to normal. Watching for recurrence."}'
```

### Resolve

```sh
bunx wrangler kv key put --remote --namespace-id "$STATUS_KV_ID" \
  "incident:2026-08-24-api-latency:$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{"status":"resolved","body":"Resolved. Queued telemetry was accepted once the rollback landed; no device data was lost."}'
```

Resolving moves the incident out of the banner and into Past incidents. It
stays there as the public record, which is the point -- do not delete it.

### Fields

| Field      | Values                                                        |
| ---------- | ------------------------------------------------------------- |
| `title`    | Short, plain, what a customer would call it.                   |
| `severity` | `minor`, `major`, `critical`. Drives the headline banner.      |
| `status`   | `investigating`, `identified`, `monitoring`, `resolved`.       |
| `surfaces` | Any of `api`, `auth`, `site`.                                  |
| `body`     | One or two sentences. What is broken, what it means, what next.|

An active incident can raise the headline above what the probes see, but
never lower it: a failing probe cannot be painted over by a cheerful
update.

`--remote` is required. Without it wrangler writes to a local simulator and
nothing reaches the live page.

### Inspecting and correcting

```sh
# what has been published
bunx wrangler kv key list --remote --namespace-id "$STATUS_KV_ID" --prefix "incident:"

# read one update back
bunx wrangler kv key get --remote --namespace-id "$STATUS_KV_ID" "<key>"

# remove an update posted in error (rare; prefer a correcting follow-up)
bunx wrangler kv key delete --remote --namespace-id "$STATUS_KV_ID" "<key>"
```

## Deploying

```sh
cd status
bunx wrangler deploy --env staging   # pidgeiot-status-staging.<subdomain>.workers.dev
bunx wrangler deploy                 # production, status.pidgeiot.com
```

Staging is a separate script (`pidgeiot-status-staging`) with its own KV
namespace and `routes = []`, so it can never take over the production
hostname or write into production's state. It probes the same production
URLs on purpose, which makes it a true rehearsal, and `STATUS_ENV` puts a
banner on it so it cannot be mistaken for the real page.

The first production deploy also creates the `status.pidgeiot.com` DNS
record, because the route is declared `custom_domain = true`. Do not
hand-create that record first or the deploy will collide with it.

## Costs

288 cron cycles a day, three cheap GETs each, and exactly one KV write per
cycle. State, history and rollups share a single key precisely so the write
count stays at one. Page reads are cached for 30 seconds at the edge and
read KV with a matching `cacheTtl`, which matters because a status page's
traffic peaks exactly when the platform is least able to help.
