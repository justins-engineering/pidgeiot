// Server-rendered HTML for the status page.
//
// No client framework, no build step and no external requests: the page is
// a single self-contained document with its CSS inlined. That is the whole
// point of this Worker. A status page that pulls a stylesheet or a script
// from the product it reports on inherits the product's outage, and a
// status page assembled in the browser is a status page that shows nothing
// to anyone whose connection is the thing that is broken.
//
// Both colour schemes come from prefers-color-scheme over CSS custom
// properties, since there is no user, no session and nowhere to persist a
// theme choice.

import { SURFACES } from "./probes.mjs";
import { uptimeFromDays, uptimeFromHistory } from "./state.mjs";

const SUPPORT_EMAIL = "support@pidgeiot.com";
const SITE = "https://pidgeiot.com";

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

const STATE_LABEL = {
  up: "Operational",
  degraded: "Degraded",
  down: "Down",
  unknown: "Unknown",
};

const OVERALL = {
  operational: { label: "All systems operational", tone: "up" },
  degraded: { label: "Degraded performance", tone: "degraded" },
  partial_outage: { label: "Partial outage", tone: "down" },
  major_outage: { label: "Major outage", tone: "down" },
  unknown: { label: "Status unknown", tone: "unknown" },
};

// The overall headline is the worst thing currently true, with a manually
// published incident able to override the automated reading upward but
// never downward. An operator who knows the platform is broken in a way no
// probe can see must be able to say so; nobody should be able to paint over
// a probe that is actively failing.
export function overallStatus(document, incidents) {
  const active = incidents.filter((i) => !i.resolved);
  const states = SURFACES.map((s) => document?.surfaces?.[s.key]?.state).filter(Boolean);

  let level = "unknown";
  if (states.length) {
    const down = states.filter((s) => s === "down").length;
    if (down >= states.length) level = "major_outage";
    else if (down > 0) level = "partial_outage";
    else if (states.some((s) => s === "degraded")) level = "degraded";
    else if (states.every((s) => s === "up")) level = "operational";
  }

  const rank = { operational: 0, unknown: 1, degraded: 2, partial_outage: 3, major_outage: 4 };
  for (const incident of active) {
    const from = incident.severity === "critical" ? "major_outage" : incident.severity === "major" ? "partial_outage" : "degraded";
    if (rank[from] > rank[level]) level = from;
  }
  return level;
}

function ago(ms, now) {
  if (!ms) return "unknown";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 10) return "just now";
  if (seconds < 90) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}

function since(ms, now) {
  if (!ms) return "";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 10) return "just now";
  if (seconds < 90) return `for ${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `for ${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `for ${hours} h`;
  return `for ${Math.round(hours / 24)} d`;
}

function pct(value) {
  if (value === null || value === undefined) return "no data";
  // Two decimals only once the number stops being a round 100, so a clean
  // window reads "100%" rather than the falsely precise "100.00%".
  return value >= 99.995 ? "100%" : `${value.toFixed(2)}%`;
}

// Fold the raw samples into fixed time buckets so the bar has a stable
// width regardless of how much history exists yet, and so a gap in
// collection renders as a visible gap instead of silently compressing.
function buckets(history, key, now, count = 48, windowMs = 24 * 60 * 60 * 1000) {
  const width = windowMs / count;
  const start = now - windowMs;
  const slots = Array.from({ length: count }, () => null);
  for (const sample of history) {
    if (!sample[key] || sample.t < start) continue;
    const index = Math.min(count - 1, Math.floor((sample.t - start) / width));
    const current = slots[index];
    const rank = { up: 0, degraded: 1, down: 2, unknown: 0 };
    if (current === null || rank[sample[key]] > rank[current]) slots[index] = sample[key];
  }
  return slots;
}

function barHtml(slots) {
  const cells = slots
    .map((slot) => {
      const state = slot ?? "nodata";
      const label = slot ? STATE_LABEL[slot] : "No data";
      return `<span class="tick tick-${esc(state)}" title="${esc(label)}"></span>`;
    })
    .join("");
  return `<div class="bar" role="img" aria-label="Availability over the last 24 hours">${cells}</div>`;
}

function surfaceHtml(document, surface, now) {
  const entry = document?.surfaces?.[surface.key];
  const state = entry?.state ?? "unknown";
  const history = document?.history ?? [];
  const days = document?.days ?? [];
  const detail = [];
  if (entry?.latency_ms !== null && entry?.latency_ms !== undefined) detail.push(`${entry.latency_ms} ms`);
  if (entry?.http_status) detail.push(`HTTP ${entry.http_status}`);
  if (entry?.error) detail.push(esc(entry.error));

  return `
    <section class="surface" id="surface-${esc(surface.key)}">
      <div class="surface-head">
        <div>
          <h3>${esc(surface.name)}</h3>
          <p class="muted">${esc(surface.detail)}</p>
        </div>
        <span class="badge badge-${esc(state)}">${esc(STATE_LABEL[state] ?? state)}</span>
      </div>
      ${barHtml(buckets(history, surface.key, now))}
      <div class="scale">
        <span>24 h ago</span>
        <span>${esc(pct(uptimeFromHistory(history, surface.key)))} uptime (24 h)</span>
        <span>now</span>
      </div>
      <p class="muted small">
        ${entry ? `${esc(STATE_LABEL[state])} ${esc(since(entry.since, now))}` : "Not yet checked"}
        ${detail.length ? ` &middot; ${detail.join(" &middot; ")}` : ""}
        ${days.length ? ` &middot; ${esc(pct(uptimeFromDays(days, surface.key)))} over ${days.length} day${days.length === 1 ? "" : "s"}` : ""}
      </p>
    </section>`;
}

function incidentHtml(incident) {
  const updates = [...incident.updates]
    .reverse()
    .map(
      (update) => `
        <li>
          <div class="update-head">
            <span class="chip chip-${esc(update.status)}">${esc(update.status)}</span>
            <time datetime="${esc(update.at)}">${esc(update.at.replace("T", " ").replace("Z", " UTC"))}</time>
          </div>
          <p>${esc(update.body)}</p>
        </li>`,
    )
    .join("");

  const affected = incident.surfaces
    .map((key) => SURFACES.find((s) => s.key === key)?.name ?? key)
    .join(", ");

  return `
    <article class="incident incident-${esc(incident.resolved ? "resolved" : incident.severity)}">
      <header>
        <h3>${esc(incident.title)}</h3>
        <span class="chip chip-${esc(incident.status)}">${esc(incident.status)}</span>
      </header>
      <p class="muted small">
        ${esc(incident.severity)} &middot; started ${esc(incident.started_at.replace("T", " ").replace("Z", " UTC"))}
        ${affected ? ` &middot; affecting ${esc(affected)}` : ""}
      </p>
      <ol class="updates">${updates}</ol>
    </article>`;
}

const CSS = `
:root {
  color-scheme: light dark;
  --bg: #f7f8fa;
  --panel: #ffffff;
  --border: #e2e5ea;
  --text: #14171c;
  --muted: #5c6470;
  --up: #16a34a;
  --degraded: #d97706;
  --down: #dc2626;
  --unknown: #94a3b8;
  --nodata: #dfe3e8;
  --accent: #4f46e5;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0e1116;
    --panel: #161b22;
    --border: #262d36;
    --text: #e7ebf0;
    --muted: #98a2b0;
    --up: #3fb950;
    --degraded: #d29922;
    --down: #f85149;
    --unknown: #6b7684;
    --nodata: #232a33;
    --accent: #8b93f8;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0;
  padding: 0 1rem 4rem;
  background: var(--bg);
  color: var(--text);
  font: 16px/1.55 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
}
.wrap { max-width: 46rem; margin: 0 auto; }
header.top { display: flex; align-items: center; justify-content: space-between; gap: 1rem; padding: 2rem 0 1.5rem; flex-wrap: wrap; }
header.top a { color: var(--muted); text-decoration: none; font-size: .875rem; }
header.top a:hover { color: var(--accent); text-decoration: underline; }
.brand { font-weight: 700; font-size: 1.125rem; letter-spacing: -.01em; }
.overall { border-radius: .75rem; padding: 1.25rem 1.5rem; margin-bottom: 1.5rem; border: 1px solid var(--border); background: var(--panel); display: flex; align-items: center; gap: .875rem; }
.overall .dot { width: .75rem; height: .75rem; border-radius: 50%; flex: none; }
/* Its own block rather than another .overall: that layout pushes a short
   heading against an auto-margined column, which squeezes two words onto
   two lines the moment the sentence beside it is long. */
.preview { border: 1px dashed var(--border); border-radius: .75rem; padding: .875rem 1.25rem; margin-bottom: 1rem; color: var(--muted); font-size: .875rem; line-height: 1.5; }
.preview strong { color: var(--text); }
.overall h1 { font-size: 1.25rem; margin: 0; letter-spacing: -.01em; }
.overall .when { margin-left: auto; color: var(--muted); font-size: .8125rem; text-align: right; }
.tone-up .dot { background: var(--up); }
.tone-degraded .dot { background: var(--degraded); }
.tone-down .dot { background: var(--down); }
.tone-unknown .dot { background: var(--unknown); }
h2 { font-size: .8125rem; text-transform: uppercase; letter-spacing: .08em; color: var(--muted); margin: 2rem 0 .75rem; font-weight: 600; }
.surface { background: var(--panel); border: 1px solid var(--border); border-radius: .75rem; padding: 1rem 1.25rem; margin-bottom: .75rem; }
.surface-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; }
.surface h3 { margin: 0; font-size: 1rem; }
.surface p { margin: .125rem 0 0; }
.muted { color: var(--muted); }
.small { font-size: .8125rem; }
.badge { font-size: .75rem; font-weight: 600; padding: .25rem .625rem; border-radius: 999px; white-space: nowrap; }
.badge-up { background: color-mix(in srgb, var(--up) 15%, transparent); color: var(--up); }
.badge-degraded { background: color-mix(in srgb, var(--degraded) 18%, transparent); color: var(--degraded); }
.badge-down { background: color-mix(in srgb, var(--down) 15%, transparent); color: var(--down); }
.badge-unknown { background: color-mix(in srgb, var(--unknown) 18%, transparent); color: var(--muted); }
.bar { display: flex; gap: 2px; margin: .875rem 0 .375rem; height: 2rem; }
.tick { flex: 1; border-radius: 2px; min-width: 2px; }
.tick-up { background: var(--up); }
.tick-degraded { background: var(--degraded); }
.tick-down { background: var(--down); }
.tick-unknown, .tick-nodata { background: var(--nodata); }
.scale { display: flex; justify-content: space-between; font-size: .75rem; color: var(--muted); }
.incident { background: var(--panel); border: 1px solid var(--border); border-left: 3px solid var(--unknown); border-radius: .75rem; padding: 1rem 1.25rem; margin-bottom: .75rem; }
.incident-minor { border-left-color: var(--degraded); }
.incident-major { border-left-color: var(--down); }
.incident-critical { border-left-color: var(--down); }
.incident-resolved { border-left-color: var(--up); }
.incident header { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
.incident h3 { margin: 0; font-size: 1rem; }
.chip { font-size: .6875rem; font-weight: 600; text-transform: uppercase; letter-spacing: .04em; padding: .1875rem .5rem; border-radius: 999px; background: color-mix(in srgb, var(--unknown) 20%, transparent); color: var(--muted); white-space: nowrap; }
.chip-investigating { background: color-mix(in srgb, var(--down) 15%, transparent); color: var(--down); }
.chip-identified { background: color-mix(in srgb, var(--degraded) 18%, transparent); color: var(--degraded); }
.chip-monitoring { background: color-mix(in srgb, var(--accent) 18%, transparent); color: var(--accent); }
.chip-resolved { background: color-mix(in srgb, var(--up) 15%, transparent); color: var(--up); }
.updates { list-style: none; margin: .875rem 0 0; padding: 0; border-left: 1px solid var(--border); }
.updates li { padding: 0 0 .875rem 1rem; position: relative; }
.updates li:last-child { padding-bottom: 0; }
.update-head { display: flex; align-items: center; gap: .5rem; margin-bottom: .25rem; }
.update-head time { font-size: .75rem; color: var(--muted); }
.updates p { margin: 0; font-size: .9375rem; }
.note { background: var(--panel); border: 1px solid var(--border); border-radius: .75rem; padding: 1rem 1.25rem; font-size: .875rem; color: var(--muted); }
.note p { margin: 0 0 .625rem; }
.note p:last-child { margin-bottom: 0; }
.note a { color: var(--accent); }
.empty { color: var(--muted); font-size: .9375rem; padding: .25rem 0 .5rem; }
footer.bottom { margin-top: 2.5rem; padding-top: 1.25rem; border-top: 1px solid var(--border); color: var(--muted); font-size: .8125rem; display: flex; justify-content: space-between; gap: 1rem; flex-wrap: wrap; }
footer.bottom a { color: var(--accent); }
`;

export function renderPage({ document: doc, incidents, now, environment }) {
  const level = overallStatus(doc, incidents);
  const overall = OVERALL[level];
  const active = incidents.filter((i) => !i.resolved);
  const past = incidents.filter((i) => i.resolved).slice(0, 15);

  const banner =
    environment && environment !== "production"
      ? `<div class="preview"><strong>Staging preview.</strong> This deployment is a rehearsal of the status page. It probes the production surfaces, but it is not the page to trust during an incident.</div>`
      : "";

  const unavailable =
    doc === null
      ? `<p class="empty">Automated checks could not be read just now. The manual incident reports below are still current.</p>`
      : doc.checked_at === null
        ? `<p class="empty">No automated check has run yet. The first one lands within five minutes of deployment.</p>`
        : "";

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PidgeIoT Status</title>
<meta name="description" content="Live availability of the PidgeIoT API, authentication and dashboard.">
<meta name="robots" content="noindex">
<meta http-equiv="refresh" content="60">
<!-- Inlined as a data URI so the page still makes exactly zero external
     requests, and so a browser's automatic /favicon.ico probe does not
     turn into a 404 in the console of the one page people open when they
     are already suspicious that things are broken. -->
<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Ccircle cx='8' cy='8' r='6' fill='%2316a34a'/%3E%3C/svg%3E">
<style>${CSS}</style>
</head>
<body>
<div class="wrap">
<header class="top">
  <span class="brand">PidgeIoT Status</span>
  <a href="${SITE}">Back to pidgeiot.com</a>
</header>

${banner}

<div class="overall tone-${esc(overall.tone)}">
  <span class="dot"></span>
  <h1>${esc(overall.label)}</h1>
  <span class="when">${doc?.checked_at ? `checked ${esc(ago(doc.checked_at, now))}` : "awaiting first check"}</span>
</div>

${unavailable}

${
  active.length
    ? `<h2>Active incidents</h2>${active.map(incidentHtml).join("")}`
    : ""
}

<h2>Systems</h2>
${SURFACES.map((surface) => surfaceHtml(doc, surface, now)).join("")}

<h2>Past incidents</h2>
${past.length ? past.map(incidentHtml).join("") : `<p class="empty">No incidents reported.</p>`}

<h2>About this page</h2>
<div class="note">
  <p>Each system above is checked every five minutes from Cloudflare's edge by requesting a public URL that real traffic already depends on. A check that fails is retried once before anything is reported as down, so a single dropped packet does not publish an outage.</p>
  <p><strong>Operational</strong> means the check succeeded. <strong>Degraded</strong> means it succeeded but took over two seconds, or it failed once and then succeeded. <strong>Down</strong> means two consecutive checks failed. Uptime figures count degraded time as available.</p>
  <p>This page runs on its own Cloudflare Worker with its own storage, entirely separate from the platform it reports on, so it stays up when the product does not. It cannot see problems that its checks do not cover, which is what the incident reports above are for.</p>
  <p>Machine-readable version: <a href="/status.json">/status.json</a>.</p>
</div>

<h2>Need help?</h2>
<div class="note">
  <p>Email <a href="mailto:${SUPPORT_EMAIL}">${SUPPORT_EMAIL}</a>, or use the <a href="${SITE}/contact/">contact form</a> for anything with details worth structuring. PidgeIoT is run by one engineer, so expect a reply within two business days rather than in minutes.</p>
  <p>Include your account email, the flock or pigeon id involved, what you expected, what happened, and the time it happened in UTC. Never include a device token or any other credential.</p>
</div>

<footer class="bottom">
  <span>Generated ${esc(new Date(now).toISOString().replace("T", " ").slice(0, 19))} UTC</span>
  <span><a href="${SITE}/documentation/#docs-support">Support and docs</a></span>
</footer>
</div>
</body>
</html>`;
}

// The machine-readable twin of the page. Same numbers, same vocabulary, no
// presentation: intended for an uptime checker, a chat integration, or a
// customer who would rather poll than read.
export function renderJson({ document: doc, incidents, now }) {
  const history = doc?.history ?? [];
  const days = doc?.days ?? [];
  return {
    status: overallStatus(doc, incidents),
    updated_at: doc?.checked_at ? new Date(doc.checked_at).toISOString() : null,
    generated_at: new Date(now).toISOString(),
    surfaces: SURFACES.map((surface) => {
      const entry = doc?.surfaces?.[surface.key];
      return {
        key: surface.key,
        name: surface.name,
        description: surface.detail,
        state: entry?.state ?? "unknown",
        since: entry?.since ? new Date(entry.since).toISOString() : null,
        checked_at: entry?.checked_at ? new Date(entry.checked_at).toISOString() : null,
        http_status: entry?.http_status ?? null,
        latency_ms: entry?.latency_ms ?? null,
        error: entry?.error ?? null,
        uptime_24h: uptimeFromHistory(history, surface.key),
        uptime_window: uptimeFromDays(days, surface.key),
        uptime_window_days: days.length,
      };
    }),
    incidents: {
      active: incidents.filter((i) => !i.resolved),
      recent: incidents.filter((i) => i.resolved).slice(0, 15),
    },
  };
}
