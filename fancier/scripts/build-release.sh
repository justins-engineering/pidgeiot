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
# Wipe the previous output first: dx never cleans stale hashed assets out
# of the output dir, so successive builds accumulate dead multi-MB wasm
# bundles that every deploy then uploads (found during the mobile-perf
# pass, 2026-08-09 -- three generations of fancier_bg-*.wasm were riding
# along). The path is recreated by dx below.
rm -rf ../target/dx/fancier/release/web/public
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
# The getting-started click-to-play still frame, same verbatim-copy route as
# og.png and for the same reason: dx's image pipeline re-encodes webp assets
# (60KB -> 218KB measured, dioxus-cli 0.7.10 -- both via asset!() and via
# the public/ passthrough dir, which it also runs the optimizer over). This
# image is that page's LCP element, so its byte size directly moves mobile
# LCP. getting_started.rs references the literal path.
cp ./assets/images/getting-started-demo-poster.webp "$PUBLIC_DIR/getting-started-poster.webp"
python3 - "$PUBLIC_DIR" <<'PYEOF'
import json, re, sys, pathlib
root = pathlib.Path(sys.argv[1])
BASE = "https://pidgeiot.com"
BRAND_TITLE = "PidgeIoT — Open-Source IoT Device Management"
BRAND_DESC = ("Open-source IoT device management: provision devices, push config, collect "
              "telemetry, update firmware over the air. Rust end to end. Free during early access.")

# Indexable marketing/docs pages: per-page title (<=60 chars) + description
# (120-160 chars) so search results don't collapse into one duplicate
# title. Anything NOT in this map gets the brand title plus a noindex
# robots meta -- that covers the auth-gated app shells (/dashboard,
# /flocks, ...), the Kratos flow pages, and /error//unauthorized, none of
# which belong in an index (they prerender as placeholder shells anyway).
PAGES = {
    "/": (BRAND_TITLE, BRAND_DESC),
    "/features/": ("IoT Platform Features — Shadows, Telemetry, OTA | PidgeIoT",
        "Device shadows with report-back, telemetry with queryable history, email alerts, "
        "OTA firmware updates, device logs, and a remote diagnostic shell."),
    "/pricing/": ("Pricing — Free During Early Access | PidgeIoT",
        "All of PidgeIoT is free during early access, no credit card required. Fair, "
        "simple pricing for larger fleets will come later — one product, one price."),
    "/documentation/": ("Documentation — Connect Your First Device | PidgeIoT",
        "How PidgeIoT works and how to connect a device: accounts, flocks, pigeons, "
        "tokens, shadows, telemetry, alerts, and over-the-air firmware updates."),
    "/api-reference/": ("API Reference — Dashboard & Device HTTP APIs | PidgeIoT",
        "The complete PidgeIoT HTTP surface: dashboard routes, device routes, Ed25519 "
        "bearer tokens, shadows, telemetry, logs, firmware, and WebSocket frames."),
    "/architecture/": ("Architecture — Edge-Native IoT, Rust End to End | PidgeIoT",
        "How PidgeIoT is built: Cloudflare Workers and Durable Objects at the edge, a "
        "WASM dashboard, managed PostgreSQL, and self-hosted Ory Kratos identity."),
    "/getting-started/": ("Getting Started — Try It With No Hardware | PidgeIoT",
        "Go from zero to live telemetry in about ten minutes using a simulated Zephyr "
        "device on your own machine — no board, no radio, no hardware required."),
    "/demo/": ("Live Demo — Real Device Data, No Signup | PidgeIoT",
        "Watch live telemetry from a real PidgeIoT device account right now: no signup, "
        "no mock data. Charts and latest values straight from the platform API."),
    "/open-source/": ("Open Source — Licenses & Attribution | PidgeIoT",
        "PidgeIoT is AGPL-3.0 and developed in the open. Full attribution and license "
        "texts for every open-source component the platform ships, auto-generated."),
    "/about/": ("About — Justin's Engineering Services | PidgeIoT",
        "PidgeIoT is built by Justin's Engineering Services, a small Massachusetts "
        "engineering company that got tired of IoT platforms punishing small fleets."),
    "/privacy/": ("Privacy Policy | PidgeIoT",
        "What PidgeIoT collects, where it lives, and what we deliberately don't do "
        "with it: no selling data, no ad tracking, and no tracking cookies."),
    "/terms/": ("Terms of Service | PidgeIoT",
        "The terms for using PidgeIoT during early access: acceptable use, account "
        "responsibility, licensing, and how changes to the service are communicated."),
}

# JSON-LD on the landing page only: Organization + WebSite +
# SoftwareApplication -- the structured answers engines actually consume.
JSONLD = json.dumps({
    "@context": "https://schema.org",
    "@graph": [
        {"@type": "Organization", "name": "Justin's Engineering Services LLC",
         "url": BASE, "logo": f"{BASE}/og.png",
         "sameAs": ["https://github.com/justins-engineering"]},
        {"@type": "WebSite", "name": "PidgeIoT", "url": BASE},
        {"@type": "SoftwareApplication", "name": "PidgeIoT",
         "applicationCategory": "DeveloperApplication",
         "operatingSystem": "Web",
         "description": BRAND_DESC,
         "url": BASE,
         "license": "https://www.gnu.org/licenses/agpl-3.0.html",
         "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD",
                    "description": "Free during early access"}},
    ],
})

for title, desc in PAGES.values():
    if not (len(title) <= 60 and 120 <= len(desc) <= 160):
        print(f"WARNING seo band violation: {len(title)}/{len(desc)} {title!r}")

# Cloudflare Web Analytics (RUM), MANUAL install by decision 2026-08-09:
# baked into the artifact instead of edge auto-injection so served HTML is
# byte-identical to what the Playwright hydration checks verify, and local
# Lighthouse runs measure the same page composition as prod. The token is a
# public beacon identifier (always visible in page source), not a secret.
# type=module defers execution; non-render-blocking. Auto-injection must
# stay OFF in the Cloudflare dashboard or pages get a second beacon.
RUM = """<!-- Cloudflare Web Analytics --><script type='module' src='https://static.cloudflareinsights.com/beacon.min.js' data-cf-beacon='{"token": "16f747723d074609936627f7f7daf1cf"}'></script><!-- End Cloudflare Web Analytics -->"""

# NOTE (tried and rejected, 2026-08-09): do NOT add a <link rel="preload">
# for the wasm bundle here. It was measured to CRATER the Lighthouse mobile
# score on every page (landing 1.00 -> 0.74, LCP 1.5s -> 8.7s locally):
# preloading makes the wasm arrive early enough that hydration begins
# before the hero's first paint in the observed trace, so Lighthouse's
# Lantern simulation chains the LCP element into the full wasm
# fetch+execute dependency graph -- the exact task #62 failure mode
# (LCP must never depend on the wasm bundle), reintroduced from the
# network side instead of the CSS side.

for f in root.rglob("index.html"):
    html = f.read_text()
    if "og:title" in html:
        continue
    # The prerender emits its own generic <title> (Dioxus.toml) and meta
    # description (lib.rs document::Meta, which DOES materialize during the
    # SSG pass, unlike at launch when it was client-only) -- leaving them in
    # alongside the injected per-page pair means two titles/descriptions per
    # page, and Google may pick the generic one (SEO audit F1, 2026-07-29).
    # Strip the shell's pair before injecting ours.
    html = re.sub(r"<title>.*?</title>", "", html, count=1)
    html = re.sub(r'<meta name="description"[^>]*/?>', "", html, count=1)
    route = "/" + str(f.parent.relative_to(root)).replace("\\", "/").lstrip(".")
    route = "/" if route in ("/", "/.") else route.rstrip("/") + "/"
    indexable = route in PAGES
    title, desc = PAGES.get(route, (BRAND_TITLE, BRAND_DESC))
    tags = (
        f"<title>{title}</title>"
        f'<meta name="description" content="{desc}">'
        + ('' if indexable else '<meta name="robots" content="noindex, nofollow">')
        + (f'<link rel="canonical" href="{BASE}{route}">' if indexable else '')
        + f'<meta property="og:type" content="website">'
        f'<meta property="og:site_name" content="PidgeIoT">'
        f'<meta property="og:title" content="{title}">'
        f'<meta property="og:description" content="{desc}">'
        f'<meta property="og:url" content="{BASE}{route}">'
        f'<meta property="og:image" content="{BASE}/og.png">'
        f'<meta property="og:image:width" content="1200">'
        f'<meta property="og:image:height" content="630">'
        f'<meta name="twitter:card" content="summary_large_image">'
        f'<meta name="twitter:title" content="{title}">'
        f'<meta name="twitter:description" content="{desc}">'
        f'<meta name="twitter:image" content="{BASE}/og.png">'
        + (f'<script type="application/ld+json">{JSONLD}</script>' if route == "/" else '')
        + RUM
    )
    if "<head>" in html:
        f.write_text(html.replace("<head>", "<head>" + tags, 1))

# Regenerate sitemap.xml from the SAME indexable-page map, so it can never
# drift from what's actually published (the old checked-in sitemap sat six
# pages stale). Overwrites the public/ passthrough copy in the output.
urls = "".join(f"<url><loc>{BASE}{r if r != '/' else '/'}</loc></url>" for r in PAGES)
(root / "sitemap.xml").write_text(
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">' + urls + "</urlset>")
print(f"seo tags injected; sitemap regenerated with {len(PAGES)} urls")
PYEOF
