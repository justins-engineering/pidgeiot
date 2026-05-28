# PidgeIoT <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-bird-icon lucide-bird"><path d="M16 7h.01"/><path d="M3.4 18H12a8 8 0 0 0 8-8V7a4 4 0 0 0-7.28-2.3L2 20"/><path d="m20 7 2 .5-2 .5"/><path d="M10 18v3"/><path d="M14 17.75V21"/><path d="M7 18a6 6 0 0 0 3.84-10.61"/></svg>

**The "No Compromise" IoT Platform**

PidgeIoT is an open-source, edge-native IoT platform built entirely in Rust. It eliminates the traditional trade-off between edge performance and data sovereignty. By routing device-facing logic through Cloudflare Workers and allowing you to self-host your own data plane, PidgeIoT gives you massive global scale without vendor lock-in.

## <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-layers-icon lucide-layers"><path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z"/><path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12"/><path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17"/></svg> Architecture & Workspace

This project is structured as a Cargo Workspace containing three primary crates:

- <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-birdhouse-icon lucide-birdhouse"><path d="M12 18v4"/><path d="m17 18 1.956-11.468"/><path d="m3 8 7.82-5.615a2 2 0 0 1 2.36 0L21 8"/><path d="M4 18h16"/><path d="M7 18 5.044 6.532"/><circle cx="12" cy="10" r="2"/></svg> **`dovecote` (Backend):** A serverless edge router built with [Cloudflare Workers](https://github.com/cloudflare/workers-rs) and Durable Objects. It handles low-latency ingestion, device provisioning, and session validation.
- <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-binoculars-icon lucide-binoculars"><path d="M10 10h4"/><path d="M19 7V4a1 1 0 0 0-1-1h-2a1 1 0 0 0-1 1v3"/><path d="M20 21a2 2 0 0 0 2-2v-3.851c0-1.39-2-2.962-2-4.829V8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v11a2 2 0 0 0 2 2z"/><path d="M 22 16 L 2 16"/><path d="M4 21a2 2 0 0 1-2-2v-3.851c0-1.39 2-2.962 2-4.829V8a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v11a2 2 0 0 1-2 2z"/><path d="M9 7V4a1 1 0 0 0-1-1H6a1 1 0 0 0-1 1v3"/></svg> **`fancier` (Frontend):** A blazing-fast WebAssembly Single Page Application (SPA) built with [Dioxus](https://dioxuslabs.com/) and styled with TailwindCSS & DaisyUI. This is the human-facing dashboard.
- <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-id-card-icon lucide-id-card"><path d="M16 10h2"/><path d="M16 14h2"/><path d="M6.17 15a3 3 0 0 1 5.66 0"/><circle cx="9" cy="11" r="2"/><rect x="2" y="5" width="20" height="14" rx="2"/></svg> **`capsules` (Shared Models):** The shared data structures, serialization logic, and RPC schemas ensuring the frontend and backend are always 100% in sync.


## <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-rocket-icon lucide-rocket"><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09"/><path d="M9 12a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.4 22.4 0 0 1-4 2z"/><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 .05 5 .05"/></svg> Development Guide

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
docker-compose -f docker-compose.yml up --force-recreate
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
*The Dashboard will be available at [http://127.0.0.1:4455](http://127.0.0.1:4455)*

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

## <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-heart-handshake-icon lucide-heart-handshake"><path d="M19.414 14.414C21 12.828 22 11.5 22 9.5a5.5 5.5 0 0 0-9.591-3.676.6.6 0 0 1-.818.001A5.5 5.5 0 0 0 2 9.5c0 2.3 1.5 4 3 5.5l5.535 5.362a2 2 0 0 0 2.879.052 2.12 2.12 0 0 0-.004-3 2.124 2.124 0 1 0 3-3 2.124 2.124 0 0 0 3.004 0 2 2 0 0 0 0-2.828l-1.881-1.882a2.41 2.41 0 0 0-3.409 0l-1.71 1.71a2 2 0 0 1-2.828 0 2 2 0 0 1 0-2.828l2.823-2.762"/></svg> Contributing
PidgeIoT is open-source. We welcome contributions regarding device protocol support (CoAP, MQTT, custom NIDD over cellular), frontend improvements, or core backend stability. Please open an issue before submitting major architectural pull requests.
