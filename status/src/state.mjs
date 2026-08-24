// The persisted status document: current per-surface state, a rolling
// sample history, and per-day rollups.
//
// All three live under ONE key, and the cron writes that key exactly once
// per cycle. That is a deliberate budget decision: a 5-minute cron is 288
// cycles a day, so one key per cycle sits inside Workers KV's free daily
// write allowance with room to spare, while splitting state and history
// into separate keys would double it for no benefit. Reads are served with
// a short cacheTtl for the same reason -- an outage is exactly when this
// page gets hammered, and the request path must stay flat.

import { SURFACES } from "./probes.mjs";

export const STATE_KEY = "status:v1";

// 288 samples at 5-minute spacing is 24 hours. Kept short on purpose: the
// long view is served by the daily rollups below, which cost a fraction of
// the bytes, so there is no reason to carry a week of raw samples around in
// every read.
const HISTORY_SAMPLES = 288;

// Roughly a quarter of a year of rollups. Small enough to be free (about a
// hundred bytes a day) and long enough to be the page's honest long-window
// number once the page has been running that long.
const ROLLUP_DAYS = 90;

export function emptyDocument() {
  return { v: 1, checked_at: null, surfaces: {}, history: [], days: [] };
}

export async function readDocument(env) {
  try {
    const raw = await env.STATUS_KV.get(STATE_KEY, { type: "json", cacheTtl: 30 });
    if (!raw || typeof raw !== "object") return emptyDocument();
    return {
      v: 1,
      checked_at: raw.checked_at ?? null,
      surfaces: raw.surfaces ?? {},
      history: Array.isArray(raw.history) ? raw.history : [],
      days: Array.isArray(raw.days) ? raw.days : [],
    };
  } catch {
    // A KV read failure must not take the page down with it. The caller
    // renders an explicit "state unavailable" instead of a 500, because a
    // status page that 500s is worse than useless.
    return null;
  }
}

function dayStamp(ms) {
  return new Date(ms).toISOString().slice(0, 10);
}

// Fold one cycle's readings into the document. Pure so the transition rules
// stay testable without a KV binding in the way.
export function applyCheck(previous, readings, now) {
  const document = previous ?? emptyDocument();
  const surfaces = {};

  for (const surface of SURFACES) {
    const reading = readings[surface.key];
    if (!reading) continue;
    const prior = document.surfaces[surface.key];
    // `since` is the age of the CURRENT state, so it only moves when the
    // state itself changes. That is the number an incident timeline wants
    // ("down for 20 minutes"), not the time of the last check.
    const changed = !prior || prior.state !== reading.state;
    surfaces[surface.key] = {
      state: reading.state,
      since: changed ? now : prior.since,
      checked_at: now,
      http_status: reading.httpStatus,
      latency_ms: reading.latencyMs,
      error: reading.error,
    };
  }

  const sample = { t: now };
  for (const surface of SURFACES) {
    if (surfaces[surface.key]) sample[surface.key] = surfaces[surface.key].state;
  }
  const history = [...document.history, sample].slice(-HISTORY_SAMPLES);

  const today = dayStamp(now);
  const days = document.days.slice();
  let bucket = days.length && days[days.length - 1].d === today ? days[days.length - 1] : null;
  if (!bucket) {
    bucket = { d: today };
    days.push(bucket);
  }
  for (const surface of SURFACES) {
    const state = surfaces[surface.key]?.state;
    if (!state || state === "unknown") continue;
    const counts = bucket[surface.key] ?? { up: 0, degraded: 0, down: 0 };
    counts[state] = (counts[state] ?? 0) + 1;
    bucket[surface.key] = counts;
  }

  return {
    v: 1,
    checked_at: now,
    surfaces,
    history,
    days: days.slice(-ROLLUP_DAYS),
  };
}

export async function writeDocument(env, document) {
  await env.STATUS_KV.put(STATE_KEY, JSON.stringify(document));
}

// Uptime over the raw sample window. Degraded deliberately counts as up:
// this is an availability number, and conflating "slow" with "unreachable"
// would overstate outages. The degraded time is still visible on the bar
// and in the per-surface detail, so nothing is hidden by the choice.
export function uptimeFromHistory(history, key) {
  const samples = history.filter((s) => s[key] && s[key] !== "unknown");
  if (!samples.length) return null;
  const good = samples.filter((s) => s[key] !== "down").length;
  return (good / samples.length) * 100;
}

export function uptimeFromDays(days, key) {
  let good = 0;
  let total = 0;
  for (const day of days) {
    const counts = day[key];
    if (!counts) continue;
    good += (counts.up ?? 0) + (counts.degraded ?? 0);
    total += (counts.up ?? 0) + (counts.degraded ?? 0) + (counts.down ?? 0);
  }
  return total ? (good / total) * 100 : null;
}
