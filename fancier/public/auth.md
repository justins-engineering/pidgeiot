# auth.md

You are an agent. This file exists at `https://pidgeiot.com/auth.md` so agents can
discover how authentication works on PidgeIoT and use the supported paths safely today.

PidgeIoT is an open-source IoT device management platform. Three hosts matter here:

- `https://pidgeiot.com` — the dashboard (this site)
- `https://api.pidgeiot.com` — the HTTP API
- `https://auth.pidgeiot.com` — identity (self-hosted Ory Kratos)

## auth.md Registration

PidgeIoT does not yet advertise programmatic auth.md registration methods, OAuth/OIDC
discovery metadata, or standalone API keys for the dashboard API. Do not attempt
`identity_assertion`, `service_auth`, or `anonymous` registration against PidgeIoT until
this file links to production discovery metadata.

## Current Authentication Paths

There are exactly two credential models — one for humans (and agents acting for a
human), one for devices:

- **Dashboard: Ory Kratos session cookie.** Accounts are created and signed in through
  Ory Kratos self-service browser flows, rendered at
  `https://pidgeiot.com/registration` and `https://pidgeiot.com/login` (backed by
  `https://auth.pidgeiot.com`). Registration requires a working email address (a
  verification code / magic link is emailed). The resulting Kratos session cookie
  (`ory_kratos_session`, scoped to `.pidgeiot.com`) must accompany every dashboard API
  request to `https://api.pidgeiot.com` (`credentials: include` from browser clients).
  A missing/invalid session gets `401`; a valid session without access to the target
  resource gets `403`.
- **Devices: per-pigeon Ed25519 bearer token.** Each provisioned device ("pigeon") gets
  its own Ed25519 keypair; devices send `Authorization: Bearer <token>` — a compact
  binary token (not a JWT), returned exactly once, in the response that mints it
  (pigeon create or token refresh). Refreshing a token cryptographically revokes the
  previous one. There is no way to read a token back later: if it's lost, refresh it.

Token handling: treat device tokens and session cookies as secrets. Never print them in
logs, commits, pull requests, screenshots, generated docs, or error reports.

Free during early access; no payment method is required to create an account.

## API Documentation

- API reference (HTML): https://pidgeiot.com/api-reference/
- API reference (markdown): https://pidgeiot.com/api-reference/index.md
- API catalog (RFC 9727): https://api.pidgeiot.com/.well-known/api-catalog
- Site overview for LLMs: https://pidgeiot.com/llms.txt
- Getting started (no hardware needed): https://pidgeiot.com/getting-started/

## Legal

- Terms: https://pidgeiot.com/terms/
- Privacy: https://pidgeiot.com/privacy/

## Contact

- Feedback endpoint (no auth required, plain JSON): `POST https://api.pidgeiot.com/feedback`
  — see the API reference for the request shape.
- About / operator: https://pidgeiot.com/about/
