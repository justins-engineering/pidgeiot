# Staging mail catcher

MailSlurper's role for the deployed staging environment. Staging sends real
mail through Cloudflare Email Service (`[[env.staging.send_email]]`,
`dovecote/wrangler.toml`), so any address a fixture or alert names reaches a
live inbox. A catch address routed to the `mailcatch` worker parks the
message in KV instead, where an agent reads it back over HTTP.

Mail stops at the catcher; nothing is forwarded onward.

## Shape

`staging-catch@pidgeiot.com` → Email Routing rule → `mailcatch` worker →
`MAILCATCH_KV` (namespace `b66d2f4175fd4fecab155084616a8637`).

Source: `mailcatch/` (one `.mjs` module, no dependencies, no bundler).
Read surface: `https://mailcatch.justinsengineeringservices.workers.dev`.

Per message: id, from, to, subject, received-at, size and the raw MIME.
The raw body is capped at **256 KiB** — over that the record is kept and
flagged `truncated`, with `size` still reporting the true original length.
Records expire after **7 days**; this is test evidence, not an archive.

Ids sort newest-first under KV's ascending key order, so `list` needs no
sort and the storage key is derivable from the id alone.

## Auth

Every route requires `Authorization: Bearer $MAILCATCH_READ_TOKEN`,
compared against the Worker secret of the same name by digesting both sides
first, so a wrong-length token costs the same comparison as a wrong-value
one. An unset secret denies rather than opens. Authentication happens
before routing, so an unauthenticated caller cannot tell which paths exist —
everything is 401, and 404 only appears once authenticated.

The token is in the gitignored `secrets.env` as `MAILCATCH_READ_TOKEN`.

## Reading mail

```sh
TOKEN=$(grep '^MAILCATCH_READ_TOKEN=' secrets.env | cut -d= -f2-)
B=https://mailcatch.justinsengineeringservices.workers.dev

# newest first, summaries only (optional ?limit=, default 50, max 1000)
curl -s -H "Authorization: Bearer $TOKEN" $B/messages

# one message, raw MIME
curl -s -H "Authorization: Bearer $TOKEN" $B/messages/<id>

# drop one
curl -s -X DELETE -H "Authorization: Bearer $TOKEN" $B/messages/<id>
```

Send a User-Agent from anything that is not curl. Cloudflare's edge answers
the default `Python-urllib/3.x` signature with a **1010** block before the
Worker ever runs, and a 1010 body looks enough like a rejection to read as a
bad token. Any string of your own passes.

Pulling a Kratos code out of a stored message:

```sh
curl -s -H "Authorization: Bearer $TOKEN" $B/messages/<id> | grep -Eo '[0-9]{6}'
```

## Pending: the routing rule

**Not yet created — needs owner approval.** Email Routing is already
enabled on the zone (`ready`, MX at `route[1-3].mx.cloudflare.net`, SPF
`include:_spf.mx.cloudflare.net`), so this adds **no DNS record** and
changes no existing rule; `support@` and `security@` keep forwarding to
`ops@jes.contact`, and the catch-all stays disabled.

```sh
cd mailcatch && bunx wrangler email routing rules create pidgeiot.com \
  --name "staging mail catcher" \
  --match-type literal --match-field to \
  --match-value staging-catch@pidgeiot.com \
  --action-type worker --action-value mailcatch
```

Reverse with `wrangler email routing rules delete pidgeiot.com <rule-id>`.

Literal matchers take one address each. A second test identity needs a
second rule rather than plus-addressing; enabling the zone catch-all would
also work but widens the blast radius to every unmatched address, so
per-address rules are preferred.

## Wiring staging senders at it

No fixture edits are part of this change. Both surfaces converge on one
lever, because an alert recipient must be the account's own verified
address or a member of the owning organization (`normalize_alert_recipients`,
`capsules/src/lib.rs`) — the catch address cannot simply be typed into a
recipient list.

So: **register a staging Kratos identity whose email is the catch address.**
Alerts on flocks that identity owns then address it as an ordinary verified
recipient, and Kratos's own courier mail for that identity — verification,
recovery, settings — lands in the same place. Registration itself is the
courier rehearsal: the verification mail is the first message the catcher
has to produce, and completing the flow from a code read out of stored MIME
proves the whole path.

Existing staging test identities keep their current addresses; this adds one
alongside them rather than repointing them.

## Acceptance test

Run once the routing rule exists.

1. Note the current message count from `GET /messages`.
2. Send a probe through the staging binding — the registration in the
   section above is the natural one, or any staging alert already addressed
   to the catch address.
3. Re-list; the new message appears with the expected `from`
   (`alerts@noreply-staging.pidgeiot.com` for alerts, the Kratos courier's
   sender for auth mail), `to: staging-catch@pidgeiot.com`, and a plausible
   `receivedAt`.
4. Fetch it by id and confirm the raw MIME carries the expected body.

Until that runs, delivery is unproven: the HTTP surface and the email
handler are both verified locally under `wrangler dev`, but no message has
travelled the real Email Routing path.

One risk this test settles: whether Cloudflare accepts mail from Email
Service to an Email Routing address inside the same account, or suppresses
it as a loop. If it is suppressed, the fallback is to point the staging
sender at an address on a domain outside the zone that forwards back in, or
to catch at the SMTP layer instead.
