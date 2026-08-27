# PidgeIoT 🕊️

**The "No Compromise" IoT Platform**

PidgeIoT is an open-source, edge-native IoT platform built entirely in Rust. It eliminates the traditional trade-off between edge performance and data sovereignty. By routing device-facing logic through Cloudflare Workers and allowing you to self-host your own data plane, PidgeIoT gives you massive global scale without vendor lock-in.

## 🏗️ Architecture & Workspace

This project is structured as a Cargo Workspace containing three primary crates:

- 🗄️ **`dovecote` (Backend):** A serverless edge router built with [Cloudflare Workers](https://github.com/cloudflare/workers-rs) and Durable Objects. It handles low-latency ingestion, device provisioning, and session validation.
- 🖥️ **`fancier` (Frontend):** A blazing-fast WebAssembly Single Page Application (SPA) built with [Dioxus](https://dioxuslabs.com/) and styled with TailwindCSS & DaisyUI. This is the human-facing dashboard.
- 💊 **`capsules` (Shared Models):** The shared data structures, serialization logic, and RPC schemas ensuring the frontend and backend are always 100% in sync.

A Cloudflare Workers runtime is HTTP-only, so the protocols that constrained devices actually speak are terminated by two small services that run on an ordinary VPS in front of `dovecote` and translate onto its device HTTP routes. Each lives in its own repository:

- 📡 **[`loft`](https://github.com/justins-engineering/loft) (CoAP terminator):** `coaps://` over DTLS/UDP and `coaps+tcp://` over TLS/TCP on one port, pre-shared-key authentication, block-wise transfer.
- 📬 **[`pigeonhole`](https://github.com/justins-engineering/pigeonhole) (MQTT broker):** MQTT 5 and 3.1.1 over TLS, certificate or pre-shared-key authentication, one session per device, the device shadow fed back as a retained message.

🕊️ **Device library:** [`pigeon`](https://github.com/justins-engineering/pigeon) is the Zephyr RTOS client module devices run, with sample applications in [`pigeon-examples`](https://github.com/justins-engineering/pigeon-examples).

📖 **API Reference:** the full `dovecote` HTTP surface (dashboard + device routes, auth models, request/response shapes) is documented in [`docs/api.md`](docs/api.md).

## 🚀 Development Guide

### Prerequisites
Before you begin, ensure you have the following installed:
- [Rust & Cargo](https://rustup.rs/) (Latest stable)
- [Bun](https://bun.com/get) or Node.js (for Cloudflare Wrangler)
- [Dioxus CLI](https://dioxuslabs.com/learn/0.7/getting_started/) (`cargo install dioxus-cli`)
- [Docker](https://docs.docker.com/engine/install/)
- [Docker Compose](https://docs.docker.com/compose/install/)

### 1. Start the Local Services (Auth & Mail)
PidgeIoT uses Ory Kratos for identity and session management. Start the local authentication and database containers from the root of the project:

```sh
docker-compose -f infra/docker-compose.yml up --force-recreate
```

- **Kratos Admin UI:** [http://127.0.0.1:3000](http://127.0.0.1:3000)
- **MailSlurper (Local Email Capture):** [http://127.0.0.1:4436](http://127.0.0.1:4436)

### 2. Start the Edge Backend (`dovecote`)
Open a new terminal window and start the Cloudflare Worker locally using Wrangler:

```sh
cd dovecote
bunx wrangler dev --ip 127.0.0.1 --port 8787 --env dev
```
*The API will be available at [http://127.0.0.1:8787](http://127.0.0.1:8787)*

### 3. Start the Web Frontend (`fancier`)
Open a third terminal window and start the Dioxus development server:

```sh
cd fancier
dx serve --addr 127.0.0.1 --port 4455
```
*The Dashboard will be available at [http://localhost:4455](http://localhost:4455) — browse it as `localhost`, not `127.0.0.1`: WebAuthn/passkeys refuse an IP-literal RP id, and the auth cookies are scoped to the host the pages share.*

### 4. Frontend development
For live CSS changes run tailwindCSS in watch mode

```sh
cd fancier
bunx @tailwindcss/cli -i ./assets/tailwind.css -o ./assets/styling/main.css --watch
```

To rebuild the Architecture diagram run
```sh
cd fancier
bunx mmdc -i assets/architecture.mmd -o assets/images/architecture.svg -b transparent
```

## 🤝 Contributing
PidgeIoT is open-source. Contributions to the dashboard, to core backend stability, and to the device library are all welcome. Protocol work belongs in the repository that owns the transport: CoAP in [`loft`](https://github.com/justins-engineering/loft), MQTT in [`pigeonhole`](https://github.com/justins-engineering/pigeonhole), on-device support in [`pigeon`](https://github.com/justins-engineering/pigeon). Cellular NIDD has no implementation anywhere yet and is open to anyone who wants it. Please open an issue before submitting major architectural pull requests.

## 📜 License
AGPL-3.0. See [`LICENSE`](LICENSE); third-party components are listed in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
