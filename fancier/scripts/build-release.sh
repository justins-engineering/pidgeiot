#!/usr/bin/env bash
# Shared release build steps for wrangler.toml and wrangler.staging.toml's
# [build] command — both target the same output directory
# (../target/dx/fancier/release/web/public), so the CSS workaround below only
# needs to live in one place.
set -euo pipefail
cd "$(dirname "$0")/.."

# Formatting gate: fail the release build on unformatted Rust rather than
# shipping drift (rustfmt.toml: tab_spaces=2, max_width=100). `cargo fmt
# --check` only — `dx fmt` rewrites files even under --check and corrupts
# valid code (mangles match arms inside rsx!), so it's not enforced here;
# rsx-body style is convention, not machine-enforced.
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
# document::Link call fires post-WASM-boot (the FOUC/CLS root cause: CSS
# was arriving ~15s after navigation start under throttling, entirely
# serialized behind the ~3MB wasm download, producing a ~0.10 layout shift
# on every page load).
#
# dx bug: [web.resource]'s style entries DO get content-hashed and copied
# to assets/main-dxh*.css like any other asset!()-tracked file, but the
# <link> tag dx writes into index.html still uses the literal pre-hash
# path ("assets/styling/main.css"), which never exists in the release
# output. Work around it by placing an unhashed copy at that exact literal
# path via Dioxus's own asset_dir="public" passthrough (Dioxus.toml),
# which copies fancier/public/* verbatim into the output root.
mkdir -p ./public/assets/styling
cp ./assets/styling/main.css ./public/assets/styling/main.css
# Same literal-path bug bites [web.resource] scripts: the <script> tag dx
# writes says "assets/error-shim.js", which only exists in the output via
# this passthrough. The public/ copy is committed (dx serve needs it too,
# same as theme-init.js); refreshing it here makes drift from the source
# impossible in a release build.
cp ./assets/error-shim.js ./public/assets/error-shim.js
# /favicon.ico at the conventional root path: browsers and link-preview
# tools request it unconditionally, and without a real file wrangler's
# SPA fallback answers 200 with text/html — which OpaqueResponseBlocking
# then blocks as an image (console noise on every visit, broken favicon
# in preview tools). The head's <link rel=icon> tags still point at the
# hashed light/dark variants; this is just the conventional-path catchall.
cp ./assets/images/icon-light.ico ./public/favicon.ico

# --ssg prerenders every statically-routable page (see `static_routes`
# server fn, fancier/src/lib.rs) to its own public/<route>/index.html via
# dioxus-server's incremental renderer, so marketing pages have real
# content in the initial HTML response instead of an empty shell hydrated
# by wasm. --force-sequential builds the server target (used only at build
# time to run the prerender) before the client wasm/js bundle; the
# "server" binary itself is never shipped or run in production -- wrangler
# only serves this directory's static files (see wrangler.toml's [assets],
# no [build].main/worker script). Auth-gated routes (/dashboard, /flocks,
# /session, /settings) are included in `static_routes` too (dioxus-router
# only excludes routes with dynamic segments, not layout/auth), but they
# prerender AuthGuard's "Verifying session..." placeholder -- `Session`'s
# state Signal starts at `AuthState::Pending` and the client-only cookie
# check in `use_future` never resolves during the synchronous SSG render,
# so nothing private ever lands in the static HTML.
# Wipe the previous output first: dx never cleans stale hashed assets out
# of the output dir, so successive builds accumulate dead multi-MB wasm
# bundles that every deploy then uploads. The path is recreated by dx below.
rm -rf ../target/dx/fancier/release/web/public
dx build --web --ssg --force-sequential --release --debug-symbols=false

# Second, unrelated dx-cli defect in the same [web.resource] tag writer:
# the CSS/theme-init.js <link>/<script> tags above land in index.html as
# bare relative paths ("assets/...", no leading "/"), unlike the
# auto-injected wasm loader tag, which dx does correctly root
# ("/./wasm/fancier.js"). A relative href resolves against the REQUESTING
# URL's path, not the site root -- fine for "/" or any single-segment
# route, but a direct/bookmarked/refreshed load of a 2+-segment route
# (e.g. /flocks/<id>/pigeons/<id>) resolves it to a nonexistent path
# nested under that route and 404s, leaving the page unstyled. This
# reproduces in the actual prod artifact, not just `dx serve`: wrangler's
# static-assets handler serves this exact index.html verbatim for any
# unmatched path (`not_found_handling = "single-page-application"` in
# wrangler.toml), so the browser — not the server — is what resolves the
# bad relative path. Root-fixing every such href here is simpler and safer
# than a <base href="/"> tag, which would silently affect any other
# relative reference added later; this only touches the two tags actually
# affected, leaving the already-correct wasm loader tag untouched.
#
# --ssg makes this worse, not just present at "/": every prerendered
# public/<route>/index.html carries its own copy of the same two
# relative-path tags, one directory level deep, so ALL of them need the
# same fix -- not just the site-root index.html.
PUBLIC_DIR="../target/dx/fancier/release/web/public"
find "$PUBLIC_DIR" -name "index.html" -print0 | xargs -0 sed -i \
  -e 's#href="assets/#href="/assets/#g' \
  -e 's#src="assets/#src="/assets/#g'

# Social/crawler head tags: Dioxus's document::Title/Meta components only
# materialize CLIENT-SIDE after hydration, so the prerendered HTML ships
# with no <title> and zero metas -- link unfurlers (Slack/Discord/X/
# iMessage), none of which run JS, render bare previews. Same class of
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
# (60KB -> 218KB measured -- both via asset!() and via the public/
# passthrough dir, which it also runs the optimizer over). This image is
# that page's LCP element, so its byte size directly moves mobile LCP.
# getting_started.rs references the literal path.
cp ./assets/images/getting-started-demo-poster.webp "$PUBLIC_DIR/getting-started-poster.webp"

# Belt-and-suspenders re-copy of the public/ passthrough files: dx's
# asset_dir copying is NON-DETERMINISTIC against a warm target/dx cache --
# a build can emit all prerendered HTML while silently dropping EVERY
# loose public/ file (robots.txt, llms.txt, auth.md, _headers,
# .well-known/, ...). The prerendered pages don't depend on this, but
# these files are correctness-critical (robots directives, Link headers,
# agent surfaces), so copy them explicitly and deterministically;
# identical content when dx also copied them, a repair when it didn't.
# `/. ` form includes dot-directories.
cp -r ./public/. "$PUBLIC_DIR/"

# The passthrough above copies the whole directory, which includes
# security.txt's unsigned source. That source is a build input, not a
# document any origin should answer with: serving an unsigned twin beside
# the signed disclosure document invites a reader to trust the copy whose
# signature nobody can check.
rm -f "$PUBLIC_DIR/.well-known/security.txt.unsigned"

# Agent-readable markdown variants (Cloudflare Agent Readiness checklist:
# Markdown). These stable .md paths are BOTH directly fetchable AND the
# backing store for real `Accept: text/markdown` content negotiation:
# worker/markdown.mjs (wired as [build].main in wrangler.toml, scoped via
# run_worker_first to the PAGES routes below) rewrites a
# markdown-preferring request for <route>/ to <route>/index.md. They're
# also advertised via `Link: rel="alternate"; type="text/markdown"`
# response headers (public/_headers) and llms.txt. Reuses existing prose
# rather than authoring parallel copies that could drift: llms.txt IS the
# site overview (-> /index.md), and docs/api.md IS the API reference --
# the exact file /api-reference/ renders via pulldown-cmark (->
# /api-reference/index.md). Every other PAGES route gets a minimal
# generated variant (title/description/links, python block below) from
# the same map that drives titles and the sitemap, so it can't drift.
cp ./public/llms.txt "$PUBLIC_DIR/index.md"
mkdir -p "$PUBLIC_DIR/api-reference"
cp ../docs/api.md "$PUBLIC_DIR/api-reference/index.md"

# Build identity for error reports: dx already content-hashes the wasm
# (fancier_bg-dxh<16 hex>.wasm), which is a perfect per-release id -- no
# new constant, just read it off the artifact. The API host comes from the
# same .env file build.rs baked into the wasm (FANCIER_ENV-aware), so the
# pre-boot JS shim -- which can't see Rust's compile-time config -- reports
# to the same place the app does. Both are injected as window globals in
# the python head pass below.
# The hex run is an unpadded u64, so its length varies -- never assume 16.
BUILD_HASH="$(find "$PUBLIC_DIR" -name 'fancier_bg-dxh*.wasm' -exec basename {} \; \
  | sed -nE 's/^fancier_bg-(dxh[0-9a-f]+)\.wasm$/\1/p' | head -n1)"
API_HOST_VALUE="$(grep -E '^API_HOST' ".env.${FANCIER_ENV:-release}" \
  | sed -E 's/^API_HOST *= *"?([^"]*)"?/\1/' | head -n1)"

python3 - "$PUBLIC_DIR" "$BUILD_HASH" "$API_HOST_VALUE" <<'PYEOF'
import json, re, sys, pathlib
root = pathlib.Path(sys.argv[1])
build_hash, api_host = sys.argv[2], sys.argv[3]

# Indexable marketing/docs pages: per-page title + description so search
# results don't collapse into one duplicate title. The map lives in
# page-meta.json rather than here because fancier reads the same titles at
# runtime (src/helpers/page_meta.rs) to name the browser tab on client-side
# navigation -- a page's tab and its <title> would otherwise be written in
# two languages in two files, free to disagree, with nothing to catch it.
# Anything NOT in the map gets the brand title plus a noindex robots meta:
# that covers the auth-gated app shells (/dashboard, /flocks, ...), the
# Kratos flow pages, and /error//unauthorized, none of which belong in an
# index (they prerender as placeholder shells anyway).
META = json.loads(pathlib.Path("page-meta.json").read_text())
BASE = META["base"]
PAGES = {route: (page["title"], page["description"])
         for route, page in META["pages"].items()}
# The landing page's own title IS the brand line, so the noindex fallback
# reuses it instead of keeping a second copy in step with it.
BRAND_TITLE, BRAND_DESC = PAGES["/"]

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

# Fail the build rather than warn: a warning this far into a long release
# build scrolls past unread, and an over-length title is truncated by the
# search engine rather than rejected, so nothing downstream would ever
# report it either.
violations = [(route, len(title), len(desc))
              for route, (title, desc) in PAGES.items()
              if not (len(title) <= 60 and 120 <= len(desc) <= 160)]
if violations:
    for route, title_len, desc_len in violations:
        print(f"page-meta.json {route}: title {title_len} (max 60), "
              f"description {desc_len} (need 120-160)")
    sys.exit("seo band violations in page-meta.json")

# Cloudflare Web Analytics (RUM), installed manually: baked into the
# artifact instead of edge auto-injection so served HTML is byte-identical
# to what the Playwright hydration checks verify, and local Lighthouse
# runs measure the same page composition as prod. The token is a public
# beacon identifier (always visible in page source), not a secret.
# type=module defers execution; non-render-blocking. Auto-injection must
# stay OFF in the Cloudflare dashboard or pages get a second beacon.
RUM = """<!-- Cloudflare Web Analytics --><script type='module' src='https://static.cloudflareinsights.com/beacon.min.js' data-cf-beacon='{"token": "16f747723d074609936627f7f7daf1cf"}'></script><!-- End Cloudflare Web Analytics -->"""

# The crash panel: static HTML already in every page, hidden, revealed by
# error-shim.js. It cannot be a Dioxus component -- after a Rust panic the
# wasm module is dead and nothing will ever render -- and it styles itself
# inline because the boot-failure case it exists for may be a failure to
# load any CSS at all. Colors are fixed (one deliberate dark card over a
# scrim) rather than theme tokens for the same reason.
BTN = ("font:inherit;padding:8px 14px;border-radius:8px;border:1px solid #4a5670;"
       "background:#2a3247;color:#e8eaf0;cursor:pointer;")
PANEL = (
    '<div id="app-crash" hidden style="position:fixed;inset:0;z-index:2147483647;'
    'background:rgba(15,18,25,.92);color:#e8eaf0;font:15px/1.5 system-ui,sans-serif;'
    'display:flex;align-items:center;justify-content:center;padding:24px;">'
    '<div style="max-width:26rem;width:100%;background:#1b2130;border:1px solid #333c52;'
    'border-radius:12px;padding:24px;">'
    '<h2 style="margin:0 0 8px;font-size:18px;">Something broke</h2>'
    '<p style="margin:0 0 8px;opacity:.85;">The dashboard hit an internal error and stopped. '
    'A technical report was sent automatically; it contains no personal data.</p>'
    '<p id="app-crash-id" style="margin:0 0 16px;font-family:monospace;font-size:12px;opacity:.6;"></p>'
    '<div style="display:flex;gap:8px;flex-wrap:wrap;">'
    f'<button id="app-crash-reload" style="{BTN}">Reload</button>'
    f'<button id="app-crash-report" style="{BTN}">Report this</button>'
    '</div>'
    '<div id="app-crash-form" hidden style="margin-top:12px;">'
    '<textarea id="app-crash-note" rows="4" placeholder="What were you doing when it broke?" '
    'style="width:100%;box-sizing:border-box;font:inherit;padding:8px;border-radius:8px;'
    'border:1px solid #4a5670;background:#12161f;color:#e8eaf0;"></textarea>'
    f'<button id="app-crash-send" style="{BTN}margin-top:8px;">Send</button>'
    '<p style="font-size:12px;opacity:.6;margin:8px 0 0;">If you are signed in, your account '
    'is attached so we can follow up.</p>'
    '</div>'
    '<p id="app-crash-thanks" hidden style="margin-top:12px;">Thanks. Your note is attached '
    'to the report.</p>'
    '</div></div>'
)
# The unsupported-browser notice: the first thing in <body>, in normal
# flow so it pushes the prerendered content down instead of covering it
# -- that content is the whole point, since it is all an old browser will
# get. Revealed by error-shim.js when its capability probe fails. The
# browsers it is for predate cascade layers and oklch(), so the app's
# stylesheet does nothing there: plain colors come first, and the theme
# tokens take over only where the engine can read them (Lockdown Mode,
# a policy that disables wasm) -- each var() carries the same plain
# fallback for a page whose CSS never arrived.
NOTICE = (
    '<div id="app-unsupported" hidden role="status"><div>'
    '<p><strong>This browser can\'t run the live dashboard.</strong> '
    'It needs a current version of Chrome, Firefox, Safari, or Edge. '
    'The rest of this page is still readable below.</p>'
    '<button id="app-unsupported-dismiss" type="button">Dismiss</button>'
    '</div></div>'
)
NOTICE_STYLE = (
    '<style>'
    '#app-unsupported{background:#f3f4f6;color:#1f2937;border-bottom:1px solid #d1d5db;'
    'border-left:4px solid #f59e0b;font:15px/1.5 system-ui,sans-serif;}'
    '#app-unsupported[hidden]{display:none;}'
    '#app-unsupported>div{max-width:64rem;margin:0 auto;padding:10px 16px;'
    'display:flex;flex-wrap:wrap;align-items:center;}'
    '#app-unsupported p{margin:4px 12px 4px 0;flex:1 1 20rem;}'
    '#app-unsupported button{font:inherit;padding:6px 14px;margin:4px 0;border-radius:8px;'
    'border:1px solid #9ca3af;background:#fff;color:#1f2937;cursor:pointer;}'
    '#app-unsupported button:focus-visible{outline:2px solid #1f2937;outline-offset:2px;}'
    '@supports (color:oklch(50% 0 0)){'
    '#app-unsupported{background:var(--color-base-200,#f3f4f6);'
    'color:var(--color-base-content,#1f2937);border-bottom-color:var(--color-base-300,#d1d5db);'
    'border-left-color:var(--color-warning,#f59e0b);}'
    '#app-unsupported button{background:var(--color-base-100,#fff);'
    'color:var(--color-base-content,#1f2937);border-color:var(--color-base-content,#9ca3af);}'
    '#app-unsupported button:focus-visible{outline-color:var(--color-base-content,#1f2937);}'
    '}'
    '</style>'
)
GLOBALS = ""
if build_hash:
    GLOBALS += f'window.__pidgeiot_build="{build_hash}";'
if api_host:
    GLOBALS += f'window.__pidgeiot_api="{api_host}";'
GLOBALS = f"<script>{GLOBALS}</script>" if GLOBALS else ""

# Do NOT add a <link rel="preload"> for the wasm bundle here. It craters
# the Lighthouse mobile score on every page (landing 1.00 -> 0.74, LCP
# 1.5s -> 8.7s locally): preloading makes the wasm arrive early enough
# that hydration begins before the hero's first paint, so Lighthouse's
# Lantern simulation chains the LCP element into the full wasm
# fetch+execute dependency graph -- LCP must never depend on the wasm
# bundle.

for f in root.rglob("index.html"):
    html = f.read_text()
    if "og:title" in html:
        continue
    # The prerender emits titles and a meta description of its own: the
    # generic shell title (Dioxus.toml), the real per-page title that
    # views::wrapper's PageTitle renders (document::Title materializes during
    # the SSG pass, same as document::Meta does), and lib.rs's brand
    # description. Leaving any of them in alongside the injected pair means a
    # page with two titles or two descriptions, and Google may pick the
    # generic one. Strip ALL of them, not just the first: which ones a given
    # page carries depends on what rendered, and a count would silently leave
    # the rest behind.
    html = re.sub(r"<title>.*?</title>", "", html)
    html = re.sub(r'<meta name="description"[^>]*/?>', "", html)
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
        + GLOBALS
        + NOTICE_STYLE
        + RUM
    )
    if "</body>" in html:
        html = html.replace("</body>", PANEL + "</body>", 1)
    html = re.sub(r"<body[^>]*>", lambda m: m.group(0) + NOTICE, html, count=1)
    if "<head>" in html:
        f.write_text(html.replace("<head>", "<head>" + tags, 1))

# Regenerate sitemap.xml from the SAME indexable-page map, so it can never
# drift from what's actually published. Overwrites the public/ passthrough
# copy in the output.
urls = "".join(f"<url><loc>{BASE}{r if r != '/' else '/'}</loc></url>" for r in PAGES)
(root / "sitemap.xml").write_text(
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">' + urls + "</urlset>")

# Markdown variants for `Accept: text/markdown` negotiation (see the shell
# comment above the / and /api-reference/ copies): every PAGES route gets
# a <route>/index.md. The two routes whose variants were already copied in
# from real prose (/ <- llms.txt, /api-reference/ <- docs/api.md) are left
# alone; the rest get a deliberately minimal generated representation --
# title, description, canonical, and pointers to the full HTML and the
# richer agent surfaces -- from this same map, never hand-authored prose
# that would drift from the real pages.
generated_md = 0
for route, (title, desc) in PAGES.items():
    md_path = root / route.lstrip("/") / "index.md"
    if md_path.exists():
        continue
    md_path.parent.mkdir(parents=True, exist_ok=True)
    md_path.write_text(
        f"# {title}\n\n"
        f"{desc}\n\n"
        f"- Canonical (full HTML): {BASE}{route}\n"
        f"- Site overview for agents: {BASE}/llms.txt\n"
        f"- API reference (markdown): {BASE}/api-reference/index.md\n"
        f"- API catalog (RFC 9727): {BASE}/.well-known/api-catalog\n")
    generated_md += 1
print(f"seo tags injected; sitemap regenerated with {len(PAGES)} urls; "
      f"{generated_md} markdown variants generated")
PYEOF
