// Markdown content negotiation for the fancier static site (Cloudflare
// Agent Readiness: Markdown; conventions from
// https://developers.cloudflare.com/fundamentals/reference/markdown-for-agents/
// and isitagentready.com's markdown-negotiation SKILL.md).
//
// A request whose Accept header genuinely PREFERS text/markdown over
// text/html (RFC 9110 q-value semantics — see acceptPrefersMarkdown) gets
// the pre-generated markdown variant of the page, when one exists in the
// build output (build-release.sh writes <route>/index.md for every
// indexable page). Everything else — browsers, requests without Accept,
// asset requests — passes straight through to the ASSETS binding
// untouched, except that page-like paths gain `Vary: Accept` so shared
// caches keep the two representations apart.
//
// Existence of a markdown variant is probed against ASSETS itself rather
// than a hardcoded route list that could drift from build-release.sh's
// PAGES map: a real .md asset comes back as text/markdown, while a miss
// falls into wrangler's single-page-application fallback and comes back
// as text/html — so checking the Content-Type of the probe result is a
// reliable existence test.
//
// Which routes pay the (single-digit-microsecond, in-process) worker hop
// at all is decided by wrangler.toml's `run_worker_first` path list, not
// here — /assets/*, /wasm/* etc. keep zero-worker direct serving in
// production. This module stays correct even when it runs on every path
// (staging's `run_worker_first = true` Access-gate config): non-page
// paths are returned byte-for-byte untouched.

const MD_CONTENT_TYPE = "text/markdown; charset=utf-8";

// RFC 9110 §12.5.1 Accept parsing, reduced to what negotiation here needs:
// media ranges + q weights (media-type parameters other than q are not
// used by any variant we serve, so they're ignored for matching).
function parseAccept(header) {
  const ranges = [];
  for (const part of header.split(",")) {
    const params = part.split(";");
    const range = params.shift().trim().toLowerCase();
    const slash = range.indexOf("/");
    if (slash < 1) continue;
    const type = range.slice(0, slash);
    const subtype = range.slice(slash + 1);
    if (!subtype) continue;
    let q = 1;
    for (const param of params) {
      const eq = param.indexOf("=");
      if (eq < 0) continue;
      if (param.slice(0, eq).trim().toLowerCase() !== "q") continue;
      const parsed = Number.parseFloat(param.slice(eq + 1).trim());
      q = Number.isNaN(parsed) ? 0 : Math.min(Math.max(parsed, 0), 1);
      break; // q ends the media-range params per the grammar
    }
    ranges.push({ type, subtype, q });
  }
  return ranges;
}

// Effective weight of a concrete media type under the parsed Accept
// ranges: the most specific matching range wins (exact > type/* > */*),
// per RFC 9110's precedence example; no match at all means weight 0.
function acceptWeight(ranges, type, subtype) {
  let best = -1;
  let q = 0;
  for (const r of ranges) {
    let specificity;
    if (r.type === type && r.subtype === subtype) specificity = 2;
    else if (r.type === type && r.subtype === "*") specificity = 1;
    else if (r.type === "*" && r.subtype === "*") specificity = 0;
    else continue;
    if (specificity > best) {
      best = specificity;
      q = r.q;
    }
  }
  return q;
}

// Negotiation triggers only when text/markdown STRICTLY outranks
// text/html: `Accept: text/markdown` wins (html matches nothing → 0), a
// browser's `text/html,...,*/*;q=0.8` keeps HTML, and a tie (e.g. `*/*`
// alone, or both listed at q=1) keeps HTML — the default representation.
export function acceptPrefersMarkdown(header) {
  if (!header) return false;
  const ranges = parseAccept(header);
  const md = acceptWeight(ranges, "text", "markdown");
  return md > 0 && md > acceptWeight(ranges, "text", "html");
}

// Page-like = a path with no file extension in its final segment — the
// class of URLs that serve a prerendered HTML page (or the SPA fallback)
// and are therefore negotiable in principle. Anything with an extension
// (/assets/*.css, *.wasm, *.md, robots.txt, ...) is a single fixed
// representation and must pass through untouched.
export function isPageLike(pathname) {
  const last = pathname.slice(pathname.lastIndexOf("/") + 1);
  return !last.includes(".");
}

// /features/ -> /features/index.md ; /features -> /features/index.md ;
// / -> /index.md. Mirrors wrangler's own directory-index resolution so a
// no-trailing-slash agent request negotiates directly instead of
// bouncing through the 307 redirect the HTML path would take.
export function markdownVariantPath(pathname) {
  return pathname.endsWith("/") ? `${pathname}index.md` : `${pathname}/index.md`;
}

function withVaryAccept(headers) {
  const vary = headers.get("Vary");
  if (!vary) headers.set("Vary", "Accept");
  else if (
    !vary
      .split(",")
      .some((v) => v.trim().toLowerCase() === "accept")
  ) {
    headers.set("Vary", `${vary}, Accept`);
  }
}

// Serve `request` from ASSETS with markdown negotiation applied. The
// single asset-serving path for every worker entrypoint (see
// access-gate.mjs, which delegates both its pass-through branches here).
export async function serveWithMarkdownNegotiation(request, env) {
  const url = new URL(request.url);
  const pageLike = isPageLike(url.pathname);
  const method = request.method;

  if (
    pageLike &&
    (method === "GET" || method === "HEAD") &&
    acceptPrefersMarkdown(request.headers.get("Accept"))
  ) {
    // Bare GET probe: no conditional headers, so a client's If-None-Match
    // (minted against either representation) can never turn the probe
    // into an unusable 304 — per the Cloudflare conventions, negotiated
    // markdown responses drop ETag/Last-Modified and aren't conditional.
    const mdUrl = new URL(markdownVariantPath(url.pathname), url);
    const probe = await env.ASSETS.fetch(new Request(mdUrl, { method: "GET" }));
    const contentType = (probe.headers.get("Content-Type") ?? "").toLowerCase();
    if (probe.ok && contentType.startsWith("text/markdown")) {
      const body = await probe.arrayBuffer();
      const headers = new Headers(probe.headers);
      headers.set("Content-Type", MD_CONTENT_TYPE);
      headers.set("Content-Length", String(body.byteLength));
      // Rough estimate (~4 bytes/token for English/markdown), same
      // convention as Cloudflare's x-markdown-tokens.
      headers.set("x-markdown-tokens", String(Math.ceil(body.byteLength / 4)));
      headers.delete("ETag");
      headers.delete("Last-Modified");
      withVaryAccept(headers);
      return new Response(method === "HEAD" ? null : body, {
        status: probe.status,
        headers,
      });
    }
    // No markdown variant for this path — fall through to HTML.
  }

  const response = await env.ASSETS.fetch(request);
  if (!pageLike) return response;
  const headers = new Headers(response.headers);
  withVaryAccept(headers);
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}
