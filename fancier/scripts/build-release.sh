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

# --ssg (task #42): prerenders every statically-routable page (see
# `static_routes` server fn, fancier/src/lib.rs) to its own
# public/<route>/index.html via dioxus-server's incremental renderer, so
# marketing pages have real content in the initial HTML response instead of
# an empty shell hydrated by wasm. --force-sequential builds the server
# target (used only at build time to run the prerender) before the client
# wasm/js bundle, which the client-side dx CLI docs recommend for fullstack
# release builds; the "server" binary itself is never shipped or run in
# production -- wrangler only serves this directory's static files (see
# wrangler.toml's [assets], no [build].main/worker script). Auth-gated
# routes (/dashboard, /flocks, /session, /settings) are included in
# `static_routes` too (dioxus-router only excludes routes with dynamic
# segments, not layout/auth), but they prerender AuthGuard's "Verifying
# session..." placeholder -- `Session`'s state Signal starts at
# `AuthState::Pending` and the client-only cookie check in `use_future`
# never resolves during the synchronous SSG render, so nothing private ever
# lands in the static HTML. Confirmed empirically (2026-07-23): no crash,
# no panic, real prerendered text for /, /features, /pricing, etc.
dx build --web --ssg --force-sequential --release --debug-symbols=false

# Second, unrelated dx-cli defect in the same [web.resource] tag writer
# (task #28): the CSS/theme-init.js <link>/<script> tags above land in
# index.html as bare relative paths ("assets/...", no leading "/"), unlike
# the auto-injected wasm loader tag, which dx does correctly root
# ("/./wasm/fancier.js"). A relative href resolves against the REQUESTING
# URL's path, not the site root -- fine for "/" or any single-segment
# route, but a direct/bookmarked/refreshed load of a 2+-segment route
# (e.g. /flocks/<id>/pigeons/<id>) resolves it to a nonexistent path
# nested under that route and 404s, leaving the page unstyled. Confirmed
# this reproduces in the actual prod artifact, not just `dx serve`:
# wrangler's static-assets handler serves this exact index.html verbatim
# for any unmatched path (`not_found_handling = "single-page-application"`
# in wrangler.toml), so the browser — not the server — is what resolves
# the bad relative path. Root-fixing every such href here is simpler and
# safer than a <base href="/"> tag, which would silently affect any other
# relative reference added later; this only touches the two tags actually
# affected, leaving the already-correct wasm loader tag untouched.
#
# --ssg (task #42) made this worse, not just present at "/": every
# prerendered public/<route>/index.html carries its own copy of the same
# two relative-path tags, one directory level deep, so ALL of them need the
# same fix -- not just the site-root index.html.
PUBLIC_DIR="../target/dx/fancier/release/web/public"
find "$PUBLIC_DIR" -name "index.html" -print0 | xargs -0 sed -i \
  -e 's#href="assets/#href="/assets/#g' \
  -e 's#src="assets/#src="/assets/#g'
