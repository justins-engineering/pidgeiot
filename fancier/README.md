# Fancier
A place for our pigeons to rest. The main website and dashboard for PidgeIoT. A web app created in Dioxus, designed to be used with Cloudflare workers to manage IoT devices.

## Development

### Requirements

- [Bun](https://bun.com/get)
- [Dioxus CLI](https://dioxuslabs.com/learn/0.7/getting_started/)

### Local services

For login/auth and live API data you also need the backend (`dovecote`) and the
local Kratos + Postgres services running. Start the services from the repo root
(see the root README for the full dev setup):

```sh
docker-compose -f infra/docker-compose.yml up --force-recreate
```

### Tailwind CSS

1. Run the following command in the root of the project:
```sh
bun install
```
2. Run the following command in the root of the project to start the Tailwind CSS compiler:

```sh
bunx @tailwindcss/cli -i ./assets/tailwind.css -o ./assets/styling/main.css --watch
```

### Mermaid.js (Architecture Diagram)

- Run the following command in the root of the project to recreate architecture.svg:

```sh
bunx mmdc -i assets/architecture.mmd -o assets/images/architecture.svg -b transparent
```

### Serving The App

Run the following command in the root of your project to start developing with the default platform:

```sh
dx serve --ssg --force-sequential --addr 127.0.0.1 --port 4455
```

### Bundling

1. Run the following command in the root of the project to compile and minify the Tailwind CSS:

```sh
bunx @tailwindcss/cli -i ./assets/tailwind.css -o ./assets/styling/main.css --minify
```

2. Run the following command in the root of your project to bundle the assets:

```sh
dx build --web --ssg --force-sequential --release --debug-symbols=false
```

### Serving The App

Run the following command in the root of your project to bundle the assets:

```sh
bunx wrangler dev --ip 127.0.0.1 --port 4455
```

## Content

### Correcting a competitor price

The comparison table on `/pricing/` renders from
`public/data/pricing-comparison.json`, not from Rust. Edit the figure there,
set its `last_verified` to the date you checked the row's `source` url, set
`status` to `verified` (you fetched it yourself just now) or `single-fetch`
(one look, not re-confirmed since), and deploy the assets. The running page
re-reads that file on load, so no rebuild is needed to correct a number; a
rebuild only refreshes the copy baked into the prerendered HTML, which is
what a reader sees before the page's JavaScript runs.

A figure left longer than the file's own `stale_after_days` renders with a
visible recheck cue, so letting one drift shows on the page rather than
going quiet. The file's `_how_to_update` block says the same thing to
whoever opens it, and `src/helpers/pricing_data.rs` has the tests that fail
if an edit breaks the shape.
