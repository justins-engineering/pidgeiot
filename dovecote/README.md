# Dovecote
The home for our pigeons. The main app for managing PidgeIoT. User/device authentication, data management, and API router. Primarily designed to be run as a Cloudflare worker.

## Development

### Requirements
- [Bun](https://bun.com/get)

### Setup

Dovecote's auth/device features need the local Kratos + Postgres services
running first. Start them from the repo root (see the root README for the full
three-terminal dev setup):

```sh
docker-compose -f infra/docker-compose.yml up --force-recreate
```

Then, from `dovecote/`:

1) `bun install`
2) `bunx wrangler dev --ip 127.0.0.1 --port 8787 --env dev`
