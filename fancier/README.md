# Fancier
A place for our pigeons to rest. The main website and dashboard for PidgeIoT. A web app created in Dioxus, designed to be used with Cloudflare workers to manage IoT devices.

## Development

### Requirements

- [Bun](https://bun.com/get)
- [Dioxus CLI](https://dioxuslabs.com/learn/0.7/getting_started/)

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
