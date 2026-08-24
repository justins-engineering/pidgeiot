# Incident communications

What we tell customers when PidgeIoT is broken, who says it, and where.

PidgeIoT is run by one engineer. This process is written for that reality:
it has to work while the same person is also fixing the outage, which means
it has to be short enough to follow from memory and cheap enough that
posting an update is never the thing that gets dropped.

## Who posts

The owner. There is nobody else, and during an incident that is a feature:
there is exactly one voice, so there is no risk of two accounts of the same
event disagreeing in public.

Nothing here is automated. The probes on the status page publish machine
readings on their own, but every sentence a customer reads about an
incident is written by hand, on purpose.

## Where, in order

1. **The status page first.** `status.pidgeiot.com`. Always. It is the only
   surface that stays up when the platform does not, it is the link support
   replies point at, and posting there first means every later channel can
   quote one canonical account instead of inventing a second one.
2. **Replies to anyone who has already written in.** Contact-form
   submissions and `support@pidgeiot.com` mail that arrived during the
   window get a short reply linking the incident, once it is resolved or
   once there is something useful to say.
3. **Nowhere else, unless it is severe.** For a `critical` incident that
   lost data or spanned hours, follow up where customers already are.
   Otherwise the status page is enough, and broadcasting a twenty-minute
   blip to every channel makes the platform look less stable than it is.

## Severity ladder

Severity is about customer impact, not about how alarming it looked from
the inside.

| Severity   | Means                                                                 | Example                                                        |
| ---------- | --------------------------------------------------------------------- | -------------------------------------------------------------- |
| `minor`    | Degraded but working. Slow, or one non-essential surface affected.     | Sign-in takes several seconds. Dashboard graphs lag.            |
| `major`    | A surface is unusable, but not the whole platform, and data is safe.   | The dashboard is down; device ingestion is unaffected.          |
| `critical` | Devices cannot report, or data was lost, or everything is down.        | The ingestion API is refusing every device. Telemetry dropped.  |

Two rules that matter more than the table:

- **Device ingestion failing is at least `major`, and `critical` if data
  was actually lost.** A dashboard nobody can load is an inconvenience. A
  fleet that cannot report is the product not working, and the customer
  usually cannot see it themselves.
- **When unsure, post at the higher severity and downgrade later.**
  Downgrading reads as competence. Upgrading two hours in reads as having
  minimised it at the start.

## Cadence

| Severity   | First post                        | Then                          |
| ---------- | --------------------------------- | ----------------------------- |
| `minor`    | Within 30 minutes of confirming   | On change, and at resolution  |
| `major`    | Within 15 minutes of confirming   | At least every 60 minutes     |
| `critical` | As soon as it is confirmed        | At least every 30 minutes     |

Post on the clock even with nothing new. "Still working on it, no change
since the last update" is a real update: silence is read as nobody being
awake, which is the single worst impression an outage can leave.

Do not wait for a root cause to post the first update. The first post says
what is broken and that it is being worked on. That is all it needs.

## What an update says

Four things, in plain language:

1. What is broken, in terms of what the customer cannot do.
2. What it means for their data, if that is in question at all. Say it even
   when the answer is "nothing was lost" -- that is the thing they are
   actually worried about.
3. What is happening next.
4. When the next update lands.

What an update never contains: blame directed at a vendor, speculation
presented as fact, internal identifiers, or any credential, token, hostname
or connection string. Write it as though a prospective customer will read
it, because during an outage that is exactly who does.

Once resolved, the closing update says what happened and what changed so it
does not happen again. For a `critical` incident that is a short paragraph,
not a formal postmortem document, but it must name the actual cause.

## The commands

The status page has no admin UI. Publishing is one `wrangler kv` command
per update, appended -- never edited. The full workflow, key format, field
reference and copy-paste templates live in `status/README.md`, which is
kept next to the code that reads them so the two cannot drift.

The shape, so this page stands alone:

```sh
export STATUS_KV_ID=<production namespace id>

bunx wrangler kv key put --remote --namespace-id "$STATUS_KV_ID" \
  "incident:$(date -u +%Y-%m-%d)-short-name:$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{"title":"...","severity":"major","status":"investigating","surfaces":["api"],"body":"..."}'
```

Every subsequent update reuses the same slug with a new timestamp and needs
only `status` and `body`; title, severity and affected surfaces carry
forward. `--remote` is required, or the update goes to a local simulator
and never reaches the page.

Rehearse against staging (`pidgeiot-status-staging`, its own namespace)
rather than learning the command during a real incident.

## Before you need it

- The status page must be deployed and its probes green *before* launch,
  not during the first outage.
- `support@pidgeiot.com` must forward somewhere the owner reads on a phone.
- Post one throwaway incident on staging end to end, open through resolve,
  so the commands are muscle memory.

## What the automated signal does not cover

The probes check that three public URLs answer. They do not check that
Postgres is healthy, that the telemetry queue is draining, that firmware
downloads work, or that any particular device is connected. A green page
with a broken data plane is entirely possible, and the page says so in
plain terms rather than implying more coverage than it has.

That gap is what manual incidents are for. If a customer reports something
the probes cannot see, and it is confirmed, it goes on the status page --
the probes being green is not a reason to stay quiet.
