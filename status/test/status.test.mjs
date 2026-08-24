// Tests for the parts of the status page whose bugs are invisible.
//
// A wrong colour on the page gets noticed the first time someone looks. A
// wrong `since` timestamp, a rollup that quietly stops counting, or a
// follow-up update that downgrades a live incident's severity all render
// perfectly plausibly and are only wrong to someone who knows what the
// number should have been. Both of the bugs these tests were written around
// were of exactly that kind.
//
// Run: node --test status/test/status.test.mjs
//
// Uses only node:test and node:assert so there is no dependency to install
// and nothing to keep current.

import test from "node:test";
import assert from "node:assert/strict";

import { applyCheck, emptyDocument, uptimeFromDays, uptimeFromHistory } from "../src/state.mjs";
import { overallStatus } from "../src/render.mjs";

const T0 = Date.parse("2026-08-24T12:00:00Z");
const FIVE_MIN = 5 * 60 * 1000;

const reading = (state, extra = {}) => ({
  state,
  httpStatus: state === "down" ? null : 200,
  latencyMs: 120,
  error: null,
  ...extra,
});

const all = (state) => ({ api: reading(state), auth: reading(state), site: reading(state) });

test("since marks when the state changed, not when it was last checked", () => {
  let doc = applyCheck(null, all("up"), T0);
  assert.equal(doc.surfaces.api.since, T0);

  // Still up one cycle later: `since` must not move, or every duration on
  // the page would permanently read as a few seconds.
  doc = applyCheck(doc, all("up"), T0 + FIVE_MIN);
  assert.equal(doc.surfaces.api.since, T0);
  assert.equal(doc.surfaces.api.checked_at, T0 + FIVE_MIN);

  doc = applyCheck(doc, all("down"), T0 + 2 * FIVE_MIN);
  assert.equal(doc.surfaces.api.since, T0 + 2 * FIVE_MIN);
});

test("history is capped and keeps the newest samples", () => {
  let doc = null;
  for (let i = 0; i < 300; i++) doc = applyCheck(doc, all("up"), T0 + i * FIVE_MIN);
  assert.equal(doc.history.length, 288);
  assert.equal(doc.history[doc.history.length - 1].t, T0 + 299 * FIVE_MIN);
});

test("uptime counts degraded as available but down as not", () => {
  let doc = null;
  for (let i = 0; i < 10; i++) doc = applyCheck(doc, all("up"), T0 + i * FIVE_MIN);
  for (let i = 10; i < 12; i++) doc = applyCheck(doc, all("degraded"), T0 + i * FIVE_MIN);
  for (let i = 12; i < 14; i++) doc = applyCheck(doc, all("down"), T0 + i * FIVE_MIN);

  // 12 of 14 samples available.
  assert.ok(Math.abs(uptimeFromHistory(doc.history, "api") - (12 / 14) * 100) < 1e-9);
  assert.ok(Math.abs(uptimeFromDays(doc.days, "api") - (12 / 14) * 100) < 1e-9);
});

test("rollups start a new bucket when the UTC day rolls over", () => {
  const lateOnDay1 = Date.parse("2026-08-24T23:57:00Z");
  let doc = applyCheck(null, all("up"), lateOnDay1);
  doc = applyCheck(doc, all("up"), lateOnDay1 + 2 * FIVE_MIN);
  assert.deepEqual(
    doc.days.map((d) => d.d),
    ["2026-08-24", "2026-08-25"],
  );
});

test("an unknown reading is recorded but never counted as uptime", () => {
  let doc = applyCheck(null, all("up"), T0);
  doc = applyCheck(doc, { ...all("up"), api: reading("unknown") }, T0 + FIVE_MIN);
  const today = doc.days[doc.days.length - 1];
  assert.equal(today.api.up, 1);
  assert.equal(uptimeFromHistory(doc.history, "api"), 100);
});

test("empty document is safe to read from", () => {
  const doc = emptyDocument();
  assert.equal(uptimeFromHistory(doc.history, "api"), null);
  assert.equal(uptimeFromDays(doc.days, "api"), null);
  assert.equal(overallStatus(doc, []), "unknown");
});

const incident = (severity, resolved) => ({ severity, resolved, updates: [] });

test("overall status is the worst surface reading", () => {
  const up = applyCheck(null, all("up"), T0);
  assert.equal(overallStatus(up, []), "operational");

  const oneDown = applyCheck(null, { ...all("up"), api: reading("down") }, T0);
  assert.equal(overallStatus(oneDown, []), "partial_outage");

  assert.equal(overallStatus(applyCheck(null, all("down"), T0), []), "major_outage");
  assert.equal(overallStatus(applyCheck(null, { ...all("up"), api: reading("degraded") }, T0), []), "degraded");
});

test("an active incident can escalate the headline but never mask a failing probe", () => {
  const healthy = applyCheck(null, all("up"), T0);
  assert.equal(overallStatus(healthy, [incident("critical", false)]), "major_outage");
  assert.equal(overallStatus(healthy, [incident("minor", false)]), "degraded");

  // A resolved incident is history and must not colour the headline.
  assert.equal(overallStatus(healthy, [incident("critical", true)]), "operational");

  // A minor incident alongside a hard-down probe must not soften the page.
  const broken = applyCheck(null, all("down"), T0);
  assert.equal(overallStatus(broken, [incident("minor", false)]), "major_outage");
});
