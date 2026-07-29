# SEO Audit Guide — static-prerendered (SSG) Dioxus/WASM sites

This is an executable audit checklist derived from Google Search Central documentation
(sources cited per check). It is written for any Dioxus SSG site served as static
files; every site-specific fact lives in the **Site Context** section below — swap
that section (and only that section) to audit a different site. Run each check
against the live site with `curl` and/or the site's source tree. Report format is
specified at the end.

---

<!-- SITE CONTEXT: replace this section when auditing a different site.
     Everything below this comment, down to END SITE CONTEXT, is the only
     site-specific part of this guide. All checks reference these facts/variables. -->

## Site Context — pidgeiot.com

Shell variables used by every check command:

```sh
BASE=https://pidgeiot.com
# Indexable pages (must match the build's per-page head-tag map and the sitemap):
PAGES="/ /features/ /pricing/ /documentation/ /api-reference/ /architecture/ /getting-started/ /demo/ /open-source/ /about/ /privacy/ /terms/"
# Representative noindexed-by-design routes (auth shells, auth flows, error pages):
NOINDEXED="/dashboard/ /flocks/ /session/ /settings/ /login/ /registration/ /recovery/ /verification/ /error/ /unauthorized/"
# Source tree and the build step that injects head tags / regenerates the sitemap:
SRC=/home/justin/pidgeiot/fancier/src
BUILD_SCRIPT=/home/justin/pidgeiot/fancier/scripts/build-release.sh   # Python block = head-tag + sitemap source of truth
PUBLIC_DIR=/home/justin/pidgeiot/fancier/public                       # verbatim passthrough: robots.txt, llms.txt, favicon.ico
SSG_OUT=/home/justin/pidgeiot/target/dx/fancier/release/web/public    # built output (prefer auditing the live site)
# Auth-guard placeholder string that must never appear on an indexable page:
PLACEHOLDER="Verifying session"
# Stable social-card image URL:
OG_IMAGE=$BASE/og.png    # 1200x630 png
```

Site facts:

- Served by Cloudflare Workers static assets (`fancier/wrangler.toml`, `[assets]`
  only) with `not_found_handling = "single-page-application"` — any unmatched path
  serves the landing page's `index.html` with **HTTP 200**.
- Canonical URL form: **https, apex domain, trailing slash** (the static host
  307-redirects `/features` → `/features/`).
- Head-tag inventory injected by `$BUILD_SCRIPT` into every prerendered
  `<route>/index.html`: per-page `<title>` + meta description, `rel=canonical`
  (indexable pages only), `noindex, nofollow` robots meta (every route NOT in the
  indexable map), OG/Twitter tags on all pages, and a `sitemap.xml` regenerated from
  the same page map. House convention: titles ≤60 chars, descriptions 120–160 chars.
- JSON-LD inventory: landing page (`/`) only — one `application/ld+json` script with
  an `@graph` of Organization + WebSite + SoftwareApplication (includes an `Offer`
  with price 0, "free during early access"). No other page carries structured data.
- `robots.txt` is currently `Allow: /` plus a `Sitemap:` line; an `llms.txt` also
  exists at the root. The sitemap has no `lastmod`/`changefreq`/`priority`.
- Site is single-language English; no hreflang anywhere, by design.
- Known live observation (2026-07-29): indexable pages carry a SECOND generic
  `<title>` and meta description emitted by the prerender itself, in addition to the
  injected per-page ones — see the duplicate-head-tag hazard in the architecture
  notes; checks 2.1/2.4 will fire on this.

<!-- END SITE CONTEXT -->

---

## Dioxus-SSG architecture notes (generic — keep for any Dioxus site)

These quirks are properties of the Dioxus SSG + static-hosting architecture, not of
any one site. They motivate several checks below:

- **Head tags must be in the prerendered HTML.** Dioxus's `document::Title`/`Meta`
  components historically materialize only client-side after hydration — invisible
  to non-JS fetchers and link unfurlers. A build step that injects head tags into
  every prerendered `index.html` (see Site Context for this site's) is the standard
  fix; that step, not the Rust components, is the source of truth for what crawlers
  see.
- **Duplicate head tags are a real hazard class.** The SSG prerender can *also* emit
  its own generic `<title>`/meta tags alongside the build-injected ones, and
  hydration can inject more. Checks 2.1 and 2.4 count tags for exactly this reason;
  when comparing values, use the *first* occurrence per page and report extras as
  findings.
- **SPA fallback serves 200 for unknown URLs.** Static hosts configured with an SPA
  not-found fallback answer every unmatched path with the fallback page's HTML and
  HTTP 200 — a soft-404 vector (check 1.6) unless the fallback's head neutralizes
  it (noindex, or canonical to its real URL).
- **Static hosts redirect to a canonical path form.** Directory-index hosting
  typically redirects `/page` ⇄ `/page/` (often with a 307). One form must be chosen
  as canonical and used consistently in canonicals, sitemap, and internal links
  (checks 3.1–3.4).
- **Auth-gated routes prerender as placeholder shells.** Client-only session checks
  never resolve during the synchronous SSG render, so gated pages prerender a
  "verifying"-style placeholder — harmless *only* if those routes are noindexed and
  the placeholder never leaks onto indexable pages (checks 1.4, 5.1).

---

## 1. Crawlability, rendering & indexing

**1.1 — Every indexable page serves its real content in the initial HTML, without JavaScript.**
- Verify: for each page, `curl -s "$BASE$p" | grep -c "<h1\|<h2"` is ≥ 1 and
  `curl -s "$BASE$p" | wc -c` is far above an empty-shell size (sub-1KB); spot-check
  that distinctive page copy appears in the raw HTML.
- Why: prerendering is what makes a JS-heavy site indexable by non-JS fetchers;
  "server-side or pre-rendering is still a great idea … not all bots can run
  JavaScript." — https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics

**1.2 — robots.txt does NOT disallow any page that carries a `noindex` meta.**
- Verify: `curl -s $BASE/robots.txt` — confirm no `Disallow:` line covers any route
  in `$NOINDEXED` (a future "helpful" Disallow would be a regression: it would hide
  the noindex from Google).
- Why: "If a page is disallowed from crawling through the robots.txt file, then any
  information about indexing or serving rules will not be found and will therefore be
  ignored" — noindex only works on crawlable pages. — https://developers.google.com/search/docs/crawling-indexing/robots-meta-tag

**1.3 — robots.txt does not block CSS, JS, or WASM assets.**
- Verify: `curl -s $BASE/robots.txt | grep -i "disallow"` must not match `/assets`,
  `/wasm`, or any resource path the pages load.
- Why: Google must "access the same resources as the user's browser"; hidden critical
  resources can hurt how pages are understood and ranked. — https://developers.google.com/search/docs/fundamentals/seo-starter-guide

**1.4 — Every noindexed route carries `noindex` in the *initial* prerendered HTML, not injected by JS.**
- Verify: `for p in $NOINDEXED; do curl -s "$BASE$p" | grep -o '<meta name="robots"[^>]*>'; done`
  — each shows `noindex, nofollow`. Cross-check the injection logic in
  `$BUILD_SCRIPT` (anything not in the indexable map must get the tag).
- Why: "When Google encounters the `noindex` tag, it may skip rendering and JavaScript
  execution" — so a JS-added noindex is unreliable; it must be in the served HTML. — https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics

**1.5 — No indexable page carries a robots `noindex` (regression guard for the inverse).**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -q 'name="robots"' && echo "FAIL $p"; done`
  — nothing printed (indexable pages get no robots meta at all in this design;
  absence = default `all`, which is correct).
- Why: `noindex` removes the page from Search entirely; default is no restrictions. — https://developers.google.com/search/docs/crawling-indexing/robots-meta-tag

**1.6 — Unknown URLs are not indexable soft-404s.**
- Verify: `curl -s -o /dev/null -w "%{http_code}\n" $BASE/no-such-page-xyz/` — the SPA
  fallback returns **200** with the fallback page's HTML. Pass condition: that HTML
  must carry `noindex` OR a canonical pointing at the fallback page's own real URL
  (check `curl -s $BASE/no-such-page-xyz/ | grep -o 'canonical[^>]*\|name="robots"[^>]*'`).
  If it serves an indexable head whose canonical is the fallback page's real URL,
  Google consolidates unknown URLs onto that page — acceptable; report which case
  holds. Genuinely dynamic app routes served via the same fallback must likewise
  resolve to a noindexed or canonicalized state, never a unique indexable 200.
- Why: pages that "don't exist" should return meaningful status codes, or SPAs should
  add `noindex` for error states to avoid soft-404s. — https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics

**1.7 — Routing uses real History-API URLs, never `#` fragments.**
- Verify: `for p in $PAGES; do curl -s "$BASE$p"; done | grep -o 'href="#/[^"]*"' | sort -u`
  must be empty; also `grep -rn 'href="#/' $SRC/` is empty.
- Why: Google resolves URLs, not fragments; use the History API with real URLs instead
  of fragment-based routing. — https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics

---

## 2. Titles & snippets

**2.1 — Every indexable page has exactly one `<title>` in the served HTML.**
- Verify: `for p in $PAGES; do echo "$p $(curl -s "$BASE$p" | grep -o '<title>' | wc -l)"; done`
  — every count must be exactly 1 (see the duplicate-head-tag hazard in the
  architecture notes; a prerender-emitted second title is a finding).
- Why: "Every page must have a title element" with content describing that specific
  page; competing titles let Google pick the wrong one. — https://developers.google.com/search/docs/appearance/title-link

**2.2 — Titles are unique across all indexable pages.**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -oP '(?<=<title>).*?(?=</title>)' | head -1; done | sort | uniq -d`
  must print nothing (compare first occurrence per page).
- Why: identical/boilerplate titles make pages indistinguishable; every page needs
  distinct title text. — https://developers.google.com/search/docs/appearance/title-link

**2.3 — Each title is descriptive and concise, brands with a short suffix, and does not repeat keywords.**
- Verify: read the titles from 2.2's output. Fail on: vague terms alone ("Home"),
  repeated words/phrases, site name as the bulk of the title on non-homepage pages,
  or titles "that vary by only a single piece of information" page to page. Any
  house-style character band (see Site Context) is convention, not a Google rule —
  do not fail on length alone (Google truncates to device width; no hard limit).
- Why: title best practices: descriptive and concise, no keyword stuffing, no
  boilerplate, brand "concisely" with a delimiter. — https://developers.google.com/search/docs/appearance/title-link

**2.4 — Every indexable page has exactly one meta description, unique per page.**
- Verify: count: `curl -s "$BASE$p" | grep -c 'name="description"'` = 1 per page;
  uniqueness: `for p in $PAGES; do curl -s "$BASE$p" | grep -oP '(?<=name="description" content=")[^"]*' | head -1; done | sort | uniq -d`
  prints nothing.
- Why: descriptions should be a "short, relevant summary" and distinct per page —
  Google may use them for the snippet; duplicated ones are useless to users. — https://developers.google.com/search/docs/appearance/snippet

**2.5 — Descriptions are summaries, not keyword lists, and are relevant to the page they sit on.**
- Verify: read 2.4's output against each page's actual content. Fail on generic
  filler, keyword strings, or a description that describes a different page.
- Why: "long strings of keywords don't give users a clear idea of the page's content";
  avoid generic or repetitive descriptions. — https://developers.google.com/search/docs/appearance/snippet

**2.6 — Each indexable page has one clearly-prominent main heading consistent with its title.**
- Verify: `curl -s "$BASE$p" | grep -o '<h1[^>]*>[^<]*</h1>'` — expect exactly one
  `<h1>` whose text is on-topic with the `<title>` (not necessarily identical).
- Why: Google generates title links from the title element *and* prominent headings;
  a distinctive main title helps Google pick the right text. Do NOT audit heading
  order/count beyond this — see the "do not bother" list. — https://developers.google.com/search/docs/appearance/title-link

---

## 3. URL structure & canonicalization

**3.1 — Every indexable page has exactly one `rel=canonical`, absolute, in `<head>`, pointing at its own URL in the site's canonical form.**
- Verify: `for p in $PAGES; do echo "$p -> $(curl -s "$BASE$p" | grep -oP '(?<=rel="canonical" href=")[^"]*')"; done`
  — each must equal `$BASE$p` exactly, in the canonical form named in Site Context
  (scheme, host, trailing slash); also `grep -c 'rel="canonical"'` = 1 per page.
- Why: use absolute URLs in the canonical link element, one per page, in the head;
  self-referential canonicals are recommended. — https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls

**3.2 — Canonical, sitemap, and redirect signals agree (no mixed signals).**
- Verify: every `<loc>` in `curl -s $BASE/sitemap.xml` must exactly match some page's
  canonical from 3.1 (same scheme/host/trailing slash). When both are generated from
  one page map in the build's head-injection step (see Site Context), a mismatch
  means that step regressed.
- Why: "Don't mix methods inconsistently" — don't specify different canonicals via
  sitemap versus `rel=canonical`. — https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls

**3.3 — Non-canonical URL variants redirect to the canonical form.**
- Verify (substitute a real page from `$PAGES`):
  `curl -s -o /dev/null -w "%{http_code} %{redirect_url}\n" $BASE/<page-no-slash>` →
  redirects to the trailing-slash form; the plain-`http://` form of `$BASE` →
  redirects to https; the `www.` (or apex, whichever is non-canonical) host variant →
  redirects to the canonical host. Record status codes: Google documents *permanent*
  redirects (301/308) as the strongest canonicalization signal; static hosts often
  emit 307 for the slash redirect — since every destination also self-canonicalizes
  (3.1), report a 307 as informational, not a failure.
- Why: redirects are the strongest canonical signal and HTTPS is preferred over HTTP
  when Google picks a canonical. — https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls

**3.4 — Internal links use the canonical URL form consistently.**
- Verify: `for p in $PAGES; do curl -s "$BASE$p"; done | grep -oP '(?<=href=")/[a-z-]+[^"]*' | sort -u`
  — internal links to indexable pages should all use the canonical form (each
  non-canonical internal link costs a redirect hop; report inconsistency with the
  link and the page carrying it).
- Why: "Avoid linking to duplicates internally — link to canonical versions
  consistently." — https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls

**3.5 — Client-side code never rewrites the canonical to a different URL after load.**
- Verify: `grep -rn "canonical" $SRC/` — if any Dioxus `document::Link` sets a
  canonical, its value must be identical to the one the build injects for that
  route. No match at all = pass.
- Why: "You shouldn't use JavaScript to change the canonical URL to something else
  than the URL you specified … in the original HTML." — https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics

**3.6 — Public URLs are descriptive words, not opaque identifiers.**
- Verify: inspect the indexable page list and the route table in the frontend source
  (`grep -rn "Routable\|#\[route" $SRC/` to find it) — all public route segments
  must be human-readable words (`/contact-us/`, not `/p?id=42`).
- Why: descriptive URLs help users and may display as breadcrumbs in results; avoid
  random identifiers. (Keywords in URLs have almost no ranking effect — this check is
  about readability, not keyword placement.) — https://developers.google.com/search/docs/fundamentals/seo-starter-guide

---

## 4. Sitemap & robots.txt

**4.1 — `sitemap.xml` exists at the site root, is valid UTF-8 XML with the sitemap-protocol namespace, and is within limits.**
- Verify: `curl -s $BASE/sitemap.xml | head -c 400` — starts with
  `<?xml version="1.0" encoding="UTF-8"?>` and declares
  `xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"`;
  `curl -s $BASE/sitemap.xml | grep -c '<loc>'` well under 50,000; validate parse:
  `curl -s $BASE/sitemap.xml | python3 -c "import sys,xml.dom.minidom; xml.dom.minidom.parseString(sys.stdin.read())"`.
- Why: sitemap format requirements — UTF-8, protocol namespace, ≤50MB/50,000 URLs;
  root placement is recommended so it can affect all site files. — https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap

**4.2 — Every sitemap URL is fully-qualified, returns 200, and is an indexable canonical page.**
- Verify: `curl -s $BASE/sitemap.xml | grep -oP '(?<=<loc>)[^<]+' | while read u; do echo "$u $(curl -s -o /dev/null -w %{http_code} "$u")"; done`
  — all 200; every URL must be absolute (`$BASE/...`) and must appear in the
  indexable `$PAGES` set (a noindexed URL in the sitemap is a contradictory signal —
  fail).
- Why: "Use fully-qualified, absolute URLs" and include only canonical/preferred
  versions in the sitemap. — https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap

**4.3 — robots.txt exists and declares the sitemap with an absolute URL.**
- Verify: `curl -s $BASE/robots.txt` contains `Sitemap: $BASE/sitemap.xml`
  (source file: see Site Context's passthrough directory).
- Why: robots.txt is a supported sitemap submission method:
  `Sitemap: https://example.com/my_sitemap.xml`. — https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap

**4.4 — If `<lastmod>` is present, it reflects real content changes; `changefreq`/`priority` are absent or ignored-by-design.**
- Verify: `curl -s $BASE/sitemap.xml | grep -c 'lastmod\|changefreq\|priority'` —
  0 passes trivially. If `lastmod` is ever added, it must be wired to actual
  page-content changes (Google only uses it "if it's consistently and verifiably
  accurate"); never add `changefreq`/`priority` (Google ignores both).
- Why: lastmod accuracy requirement; "Google ignores `<priority>` and `<changefreq>`
  values." — https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap

---

## 5. Content & headings

**5.1 — Prerendered pages contain their real, complete text content (no placeholder leakage into indexable pages).**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -c "$PLACEHOLDER"; done`
  — must be 0 on every indexable page (that string is the auth-guard placeholder and
  belongs only on noindexed shells; see architecture notes).
- Why: "If the content isn't visible in the rendered HTML, Google won't be able to
  index it" — and here the *served* HTML is what most fetchers get. — https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics

**5.2 — Body content is organized into readable sections with helpful headings, unique per page.**
- Verify: dump text (`curl -s "$BASE$p" | python3 -c "import sys,html,re; print(re.sub('<[^>]+>',' ',sys.stdin.read()))"`)
  and read it: paragraphs/sections with headings, no spelling/grammar errors, no text
  block duplicated wholesale across two indexable pages.
- Why: content should be "easy-to-read and well-organized", written naturally, unique
  — "don't copy others' content in part or in its entirety." — https://developers.google.com/search/docs/fundamentals/seo-starter-guide

**5.3 — No keyword stuffing in body copy.**
- Verify: from 5.2's text dump, check no term repeats unnaturally (a quick
  `... | tr ' ' '\n' | sort | uniq -c | sort -rn | head` flags suspects; judge in
  context).
- Why: "excessively repeating the same words over and over (even in variations) is
  tiring for users" and violates spam policies. — https://developers.google.com/search/docs/fundamentals/seo-starter-guide

---

## 6. Images

**6.1 — Every meaningful `<img>` in indexable pages' HTML has descriptive alt text.**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -o '<img[^>]*>'; done | grep -v 'alt="[^"]'`
  — anything printed either lacks `alt` entirely (fail) or has empty `alt=""`
  (acceptable only for purely decorative images — judge each). Also check any inlined
  SVG illustrations (`dangerous_inner_html` pattern in `$SRC/`) have a text
  alternative (`<title>`/`aria-label`) when they carry meaning.
- Why: alt text "explains the relationship between the image and your content" and is
  how search engines understand images; add it via the `alt` attribute. — https://developers.google.com/search/docs/fundamentals/seo-starter-guide

**6.2 — Images sit near the text they relate to.**
- Verify: structural read of the prerendered HTML — each content image should be
  inside/adjacent to the section discussing it.
- Why: "place images near text relevant to the image" — nearby text helps Google
  understand what the image is about. — https://developers.google.com/search/docs/fundamentals/seo-starter-guide

**6.3 — Image links (if any) have alt text serving as anchor text.**
- Verify: `grep -o '<a[^>]*>\s*<img[^>]*>'` output from the pages — the `img` inside a
  link must carry descriptive `alt`.
- Why: "for image links: descriptive `alt` text on the `img` element" is the anchor
  text. — https://developers.google.com/search/docs/crawling-indexing/links-crawlable

---

## 7. Links & anchor text

**7.1 — All navigation is `<a href="...">` with resolvable URLs — no JS-only pseudo-links.**
- Verify: on the prerendered pages, `grep -o '<a [^>]*>' | grep -v 'href='` must be
  empty; `grep -c 'href="javascript:'` must be 0; no `<span>`/`<button>` used for
  navigation (check `$SRC/` components for onclick-navigation patterns — Dioxus's
  `Link {}` component renders `<a href>`, which passes).
- Why: "Google can only discover your links if they are `<a>` HTML elements with an
  `href` attribute"; `onclick`-only and `javascript:` links are not reliably parsed. — https://developers.google.com/search/docs/crawling-indexing/links-crawlable

**7.2 — No empty anchors; anchor text is descriptive, not generic.**
- Verify: `grep -oP '<a [^>]*>\s*</a>'` empty (icon-only links need `title`/aria or
  img alt); scan anchor texts (`grep -oP '(?<=>)[^<>]{1,40}(?=</a>)'`) for "click
  here" / "read more" / bare "here" — fail those.
- Why: anchor text should be "descriptive, reasonably concise, and relevant" to the
  destination; avoid generic text; provide visible link text (or title/alt fallback). — https://developers.google.com/search/docs/crawling-indexing/links-crawlable

**7.3 — Every indexable page is linked from at least one other page on the site.**
- Verify: build the internal link graph:
  `for p in $PAGES; do curl -s "$BASE$p" | grep -oP '(?<=href=")/[a-z-]*/?(?=")' | sed "s|^|$p -> |"; done`
  — every page in `$PAGES` must appear as a destination from some *other* page
  (header/footer nav usually satisfies this; fail any orphan).
- Why: "Every page you care about should have a link from at least one other page";
  links are Google's primary discovery mechanism. — https://developers.google.com/search/docs/crawling-indexing/links-crawlable

**7.4 — External links carry `rel` qualifiers only where warranted.**
- Verify: `grep -oP '<a [^>]*href="https?://[^"]*"[^>]*>'` across the pages, filtered
  to hosts other than the site's own — first-party/trusted destinations (the
  project's own repos, docs) need **no** nofollow. Fail only if the site embeds links
  it doesn't vouch for (or user-generated content) without `nofollow`/`ugc`. Do NOT
  flag trusted external links missing nofollow — that's not what the attribute is
  for.
- Why: use `nofollow` "only when you distrust the source", `ugc` for user-generated
  areas — not as a blanket on external links. — https://developers.google.com/search/docs/crawling-indexing/links-crawlable

---

## 8. Structured data

**8.1 — Each page listed in the Site Context JSON-LD inventory serves valid JSON-LD with schema.org context in a `type="application/ld+json"` script.**
- Verify: `curl -s "$BASE$p" | grep -oP '(?<=<script type="application/ld\+json">).*?(?=</script>)' | python3 -m json.tool`
  succeeds and shows `"@context": "https://schema.org"` with the types the inventory
  lists for that page.
- Why: JSON-LD is Google's recommended format, embedded in a script tag in head or
  body. — https://developers.google.com/search/docs/appearance/structured-data/intro-structured-data

**8.2 — Every claim in the structured data is accurate and corresponds to content visible on the page.**
- Verify: cross-check each JSON-LD property against the page's rendered text — names,
  prices/offers, license, description must each be visibly stated on the page (grep
  the page text for the matching copy) and factually current (fail if the page copy
  changes but the JSON-LD doesn't).
- Why: "don't add structured data about information that is not visible to the user,
  even if the information is accurate"; misleading/inaccurate data is prohibited. — https://developers.google.com/search/docs/appearance/structured-data/intro-structured-data

**8.3 — Each JSON-LD type carries its required properties, completely rather than maximally.**
- Verify: for the types in the inventory — e.g. Organization: `name` + `url`;
  WebSite: `name` + `url`; SoftwareApplication: `name`, `applicationCategory`,
  `operatingSystem`, and `offers` with `price`/`priceCurrency`. Definitive
  validation is the Rich Results Test (https://search.google.com/test/rich-results)
  — run the live URL through it if browser access is available; otherwise report
  "not machine-verified" rather than guessing.
- Why: "All required properties must be included for rich result eligibility"; fewer
  but complete properties beat exhaustive incomplete ones. — https://developers.google.com/search/docs/appearance/structured-data/intro-structured-data

**8.4 — Structured data appears only on the pages it applies to.**
- Verify: `for p in $PAGES; do echo "$p $(curl -s "$BASE$p" | grep -c 'ld+json')"; done`
  — counts must match the Site Context inventory exactly, and be 0 on all noindexed
  shells.
- Why: structured data must be "on the page that the information applies to"; blank/
  placeholder pages must not carry it. — https://developers.google.com/search/docs/appearance/structured-data/intro-structured-data

---

## 9. Mobile & page experience

**9.1 — All pages and subresources are HTTPS; no mixed content.**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -o 'src="http://[^"]*"\|href="http://[^"]*"'; done`
  — empty (plain-http URLs inside JSON-LD *text values*, e.g. a license identifier,
  are fine; actual subresource loads are not).
- Why: "Are your pages served in a secure fashion?" is one of Google's page-experience
  self-assessment questions; HTTPS is also preferred for canonical selection. — https://developers.google.com/search/docs/appearance/page-experience

**9.2 — Every page has a viewport meta tag.**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -c 'name="viewport"'; done`
  — all ≥ 1.
- Why: "Does your content display well on mobile devices?" — the viewport meta is the
  baseline requirement for mobile rendering. — https://developers.google.com/search/docs/appearance/page-experience

**9.3 — Core Web Vitals are good on the public pages.**
- Verify: run Lighthouse/PageSpeed Insights against `$BASE/` and one content page;
  record LCP/CLS/INP. Watch specifically for CLS from late CSS or hydration swaps,
  and LCP impact of the multi-MB WASM bundle (prerendered text should make LCP
  independent of WASM — verify it is). If no browser tooling is available, report
  "not measured" — do not estimate.
- Why: Core Web Vitals are the one page-experience component Google states is used by
  its core ranking systems. — https://developers.google.com/search/docs/appearance/core-web-vitals (via https://developers.google.com/search/docs/appearance/page-experience)

**9.4 — No intrusive interstitials or content-blocking overlays on page load.**
- Verify: check the prerendered HTML and `$SRC/` for any modal/dialog rendered open
  on initial load of a public page (cookie walls, newsletter popups).
- Why: "Do your pages avoid using intrusive interstitials?" and content must not be
  blocked by overlays users must dismiss. — https://developers.google.com/search/docs/appearance/page-experience

---

## 10. Social/OG extras (link previews — not a ranking input, but in audit scope)

**10.1 — Every indexable page carries a complete, self-consistent OG/Twitter tag set.**
- Verify: per page, `curl -s "$BASE$p" | grep -oP 'property="og:[^"]*" content="[^"]*"'`
  shows `og:title`/`og:description`/`og:url`/`og:image`/`og:type`/`og:site_name`, plus
  `twitter:card`; `og:url` must equal the page's canonical exactly;
  `og:title`/`og:description` must match the page's own title/description.
- Why: Google may use `og:title` as a title-link source, and these tags drive every
  non-JS link unfurler — the exact gap build-time head injection exists to close. — https://developers.google.com/search/docs/appearance/title-link

**10.2 — The OG image and favicon resolve at stable URLs with correct types.**
- Verify: `curl -s -o /dev/null -w "%{http_code} %{content_type}\n" $OG_IMAGE`
  → `200 image/png`; same for `$BASE/favicon.ico` (must NOT be `text/html` — an SPA
  fallback answering the conventional favicon path with HTML is a known failure mode
  of this architecture); pages include `<link rel="icon">` tags. Deeper
  favicon-in-search requirements: https://developers.google.com/search/docs/appearance/favicon-in-search.
- Why: unfurlers cache og:image URLs long-term (hence a stable unhashed path);
  Google shows favicons in search results and must be able to fetch them.

---

## 11. Internationalization

**11.1 — hreflang annotations exist only if the site has real localized versions (see Site Context).**
- Verify: `for p in $PAGES; do curl -s "$BASE$p" | grep -c 'hreflang'; done` — for a
  single-language site, all 0. Titles/content language must match the page's actual
  language (per the title-link language rule).
- Why: hreflang is for sites with localized page versions; annotations pointing at
  nonexistent alternates are a canonicalization hazard (hreflang clusters influence
  canonical choice). — https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls

---

## Do NOT flag these (Google explicitly says they don't matter)

All from https://developers.google.com/search/docs/fundamentals/seo-starter-guide unless noted:

1. **Missing `meta keywords` tag** — "Google Search doesn't use the keywords meta tag."
2. **Keywords in the domain name or URL path** — "hardly any effect beyond appearing
   in breadcrumbs." Do not propose URL renames for keyword reasons.
3. **TLD choice** — Google doesn't care about `.com` vs others except as a weak
   country-targeting signal.
4. **Word count / content length** — "the length of the content alone doesn't matter
   for ranking purposes." No minimums, no maximums.
5. **Heading order and count** — from Google Search's perspective, heading order
   "doesn't matter" and there is no ideal number of headings. (Semantic order still
   matters for screen readers — file that as accessibility, not SEO.)
6. **Subdomain vs subdirectory** — no ranking advantage either way.
7. **Own-site duplicate/alternate URLs as a "penalty"** — duplicate content on your
   own site is not a spam violation; it's a canonicalization/efficiency matter only
   (covered by section 3). Never report it as a penalty risk.
8. **E-E-A-T as a ranking factor** — Google explicitly says it is not one.
9. **`<changefreq>`/`<priority>` in sitemaps** — Google ignores both
   (https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap).
10. **Title/description character-count "limits"** — no hard limits exist; truncation
    is display-width-based. Any house band in the build is convention; only flag
    titles/descriptions that are vague, stuffed, or duplicated.
11. **Blanket `nofollow` on external links** — nofollow is for untrusted/paid/UGC
    links only (https://developers.google.com/search/docs/crawling-indexing/links-crawlable).
12. **PageRank tuning** — PageRank is one of many signals; do not recommend
    link-sculpting.

---

## Report format

Produce a single findings report, structured as:

1. **Summary line**: N checks run, N pass, N fail, N not-verifiable (and why).
2. **Findings, ranked by impact**, using these tiers:
   - **P0 — blocks or corrupts indexing**: noindex on an indexable page, robots.txt
     blocking pages/assets, missing/broken canonical, sitemap serving errors,
     prerender regression (placeholder text on a public page).
   - **P1 — degrades search appearance or crawl efficiency**: duplicate/missing
     titles or descriptions, contradictory canonical vs sitemap signals, soft-404
     exposure, broken structured data, orphan pages.
   - **P2 — quality/consistency nits**: generic anchor text, missing alt on decorative
     images, redirect-hop internal links, OG inconsistencies.
3. **Each finding must include**:
   - The check number it fails (e.g. "2.2").
   - **Evidence**: the actual command run and its actual output (trimmed to the
     failing lines) — never a paraphrase.
   - **Affected URL(s) and the source file to fix** — for head-tag, canonical,
     robots-meta, and sitemap issues that is almost always the page map / tag builder
     in the build's head-injection step (`$BUILD_SCRIPT`); for root files
     (robots.txt, llms.txt, favicon), the static passthrough directory
     (`$PUBLIC_DIR`); for in-page content/links/images, the component under `$SRC/`.
   - **Proposed fix**: one or two sentences, concrete (what line/value changes), plus
     the Google-doc URL justifying it.
4. **Do not report** anything from the "Do NOT flag" list, checks that pass, or
   speculative improvements with no Google-doc basis. If a check cannot be run
   (e.g. no browser for Lighthouse), list it under not-verifiable rather than
   guessing an outcome.
