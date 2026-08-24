// PidgeIoT status page: entrypoint for both the request path and the cron.
//
// This Worker is a deliberately separate failure domain from the platform
// it reports on. It is its own script with its own storage, it shares no
// binding with dovecote or fancier, and at request time it reads exactly
// one thing: its own KV namespace. No Hyperdrive, no Postgres, no Kratos,
// no call into the product. Whatever takes the platform down should leave
// this page standing, because that is the moment it exists for.
//
// It is also plain ESM with no build step, matching fancier/worker/*.mjs.
// The fewer moving parts between "an outage started" and "the page renders",
// the better -- a status page that needs a toolchain to redeploy is one more
// thing to be broken at the worst possible time.

import { checkAll } from "./probes.mjs";
import { readIncidents } from "./incidents.mjs";
import { readDocument, applyCheck, writeDocument } from "./state.mjs";
import { renderPage, renderJson } from "./render.mjs";

// Short enough that the page is never meaningfully stale, long enough to
// flatten the traffic spike that an outage brings to a status page.
const PAGE_CACHE_SECONDS = 30;

function securityHeaders(extra = {}) {
  return {
    // The page is entirely self-contained, so it can afford to forbid
    // everything except its own inline stylesheet.
    "content-security-policy":
      "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    "referrer-policy": "no-referrer",
    "x-content-type-options": "nosniff",
    ...extra,
  };
}

async function load(env) {
  // Both reads are independent, and each already swallows its own failure:
  // KV being unavailable for the automated state must not also hide the
  // manual incident reports, and vice versa.
  const [document, incidents] = await Promise.all([readDocument(env), readIncidents(env)]);
  return { document, incidents };
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method Not Allowed", {
        status: 405,
        headers: securityHeaders({ allow: "GET, HEAD", "content-type": "text/plain; charset=utf-8" }),
      });
    }

    // Liveness for the watcher itself, answered without touching KV so it
    // stays true even when storage is the thing that is broken.
    if (url.pathname === "/health") {
      return new Response("ok", {
        status: 200,
        headers: securityHeaders({ "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" }),
      });
    }

    if (url.pathname === "/status.json") {
      const { document, incidents } = await load(env);
      return new Response(JSON.stringify(renderJson({ document, incidents, now: Date.now() }), null, 2), {
        status: 200,
        headers: securityHeaders({
          "content-type": "application/json; charset=utf-8",
          "cache-control": `public, max-age=${PAGE_CACHE_SECONDS}`,
          // A status feed is meant to be readable from anywhere, including
          // from the dashboard origin during an outage.
          "access-control-allow-origin": "*",
        }),
      });
    }

    if (url.pathname === "/" || url.pathname === "/index.html") {
      const { document, incidents } = await load(env);
      const html = renderPage({
        document,
        incidents,
        now: Date.now(),
        environment: env.STATUS_ENV ?? "production",
      });
      return new Response(html, {
        status: 200,
        headers: securityHeaders({
          "content-type": "text/html; charset=utf-8",
          "cache-control": `public, max-age=${PAGE_CACHE_SECONDS}`,
        }),
      });
    }

    return new Response("Not Found", {
      status: 404,
      headers: securityHeaders({ "content-type": "text/plain; charset=utf-8" }),
    });
  },

  async scheduled(event, env, ctx) {
    ctx.waitUntil(
      (async () => {
        const readings = await checkAll(env);
        const previous = await readDocument(env);
        // A failed read yields null, which applyCheck treats as an empty
        // document. That loses history rather than corrupting it, and is
        // preferable to skipping the write entirely: the current state is
        // the part people are looking at.
        const next = applyCheck(previous, readings, Date.now());
        await writeDocument(env, next);
      })(),
    );
  },
};
