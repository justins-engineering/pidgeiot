#!/usr/bin/env bash
# Shared release build steps for wrangler.toml and wrangler.staging.toml's
# [build] command — both target the same output directory
# (../target/dx/fancier/release/web/public), so the CSS workaround below only
# needs to live in one place.
set -euo pipefail
cd "$(dirname "$0")/.."

# Formatting gate: fail the release build on unformatted Rust rather than
# shipping drift (rustfmt.toml: tab_spaces=2, max_width=100). Deliberately
# `cargo fmt --check` ONLY — `dx fmt` is NOT enforced: dioxus-cli 0.7.9's
# formatter both rewrites files even under --check and, worse, corrupts
# valid code (mangles match arms inside rsx! — produced 8 compile errors
# across firmware_modal.rs/alerts_panel.rs when run over this crate,
# 2026-07-27). Revisit when a fixed dioxus-cli ships; until then rsx-body
# style is convention, not machine-enforced.
cargo fmt --check -p fancier -p dovecote -p capsules

bunx @tailwindcss/cli -i ./assets/tailwind.css -o ./assets/styling/main.css -m

# Regenerates the /open-source page's crate-license inventory from the
# actual current dependency graph (see generate-oss-notices.sh and
# fancier/src/views/open_source.rs) so it can't drift stale. Requires
# cargo-about (`cargo install cargo-about --locked --features cli`); a
# release build is expected to have it, unlike plain `cargo check`/`dx
# serve`, which read the checked-in generated/oss-licenses.md snapshots
# this overwrites and don't need cargo-about at all.
./scripts/generate-oss-notices.sh

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
# /favicon.ico at the conventional root path: browsers and link-preview
# tools request it unconditionally, and without a real file wrangler's
# SPA fallback answers 200 with text/html — which OpaqueResponseBlocking
# then blocks as an image (console noise on every visit, broken favicon
# in preview tools). The head's <link rel=icon> tags still point at the
# hashed light/dark variants; this is just the conventional-path catchall.
cp ./assets/images/icon-light.ico ./public/favicon.ico

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

# Social/crawler head tags (launch-day fix, 2026-07-28): Dioxus's
# document::Title/Meta components only materialize CLIENT-SIDE after
# hydration -- verified against the live prerendered HTML, which shipped
# with no <title> and zero metas, so link unfurlers (Slack/Discord/X/
# iMessage), none of which run JS, rendered bare previews. Same class of
# problem as the [web.resource] CSS fix above, same class of solution:
# inject the tags statically into every prerendered index.html. og:url is
# derived per page from its output path. Idempotent (skips files already
# carrying og:title). The 1200x630 card lives at a STABLE unhashed URL
# (/og.png) because unfurlers cache og:image URLs long-term; it's copied
# from assets/images/og.png (rendered from og.svg via rsvg-convert -- see
# that file to regenerate).
cp ./assets/images/og.png "$PUBLIC_DIR/og.png"
python3 - "$PUBLIC_DIR" <<'PYEOF'
import sys, pathlib
root = pathlib.Path(sys.argv[1])
TITLE = "PidgeIoT — Open-Source IoT Device Management"
# 158 chars -- validators want og:description in the 120-160 band.
DESC = ("Open-source IoT device management: provision devices, push config, collect "
        "telemetry, update firmware over the air. Rust end to end. Free during early access.")
for f in root.rglob("index.html"):
    html = f.read_text()
    if "og:title" in html:
        continue
    route = "/" + str(f.parent.relative_to(root)).replace("\\", "/").lstrip(".")
    route = "/" if route in ("/", "/.") else route.rstrip("/") + "/"
    tags = (
        f"<title>{TITLE}</title>"
        f'<meta name="description" content="{DESC}">'
        f'<meta property="og:type" content="website">'
        f'<meta property="og:site_name" content="PidgeIoT">'
        f'<meta property="og:title" content="{TITLE}">'
        f'<meta property="og:description" content="{DESC}">'
        f'<meta property="og:url" content="https://pidgeiot.com{route}">'
        f'<meta property="og:image" content="https://pidgeiot.com/og.png">'
        f'<meta property="og:image:width" content="1200">'
        f'<meta property="og:image:height" content="630">'
        f'<meta name="twitter:card" content="summary_large_image">'
        f'<meta name="twitter:title" content="{TITLE}">'
        f'<meta name="twitter:description" content="{DESC}">'
        f'<meta name="twitter:image" content="https://pidgeiot.com/og.png">'
    )
    if "<head>" in html:
        f.write_text(html.replace("<head>", "<head>" + tags, 1))
print("og/title tags injected")
PYEOF
