#!/usr/bin/env bash
# Shared release build steps for wrangler.toml and wrangler.staging.toml's
# [build] command — both target the same output directory
# (../target/dx/fancier/release/web/public), so the CSS workaround below only
# needs to live in one place.
set -euo pipefail
cd "$(dirname "$0")/.."

bunx @tailwindcss/cli -i ./assets/tailwind.css -o ./assets/styling/main.css -m

# Dioxus.toml's [web.resource] writes a static <link rel="stylesheet"> into
# the generated index.html so the browser can fetch CSS in parallel with
# app.js/wasm, instead of only requesting it after Dioxus's runtime
# document::Link call fires post-WASM-boot (the FOUC/CLS root cause — see
# fancier design-review notes, task #9: CSS was arriving ~15s after
# navigation start under throttling, entirely serialized behind the ~3MB wasm
# download, producing a single ~0.10 layout shift on every page load).
#
# Confirmed empirically against dioxus-cli 0.7.9: [web.resource]'s style
# entries DO get content-hashed and copied to assets/main-dxh*.css like any
# other asset!()-tracked file, but the <link> tag dx writes into index.html
# still uses the literal pre-hash path ("assets/styling/main.css"), which
# never exists in the release output — a dx bug, not a config mistake. Work
# around it by placing an unhashed copy at that exact literal path via
# Dioxus's own asset_dir="public" passthrough (Dioxus.toml), which copies
# fancier/public/* verbatim into the output root.
mkdir -p ./public/assets/styling
cp ./assets/styling/main.css ./public/assets/styling/main.css

dx build --web --release --debug-symbols=false
