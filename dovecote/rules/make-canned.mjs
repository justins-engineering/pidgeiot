// Writes canned/*.json: real harness responses that dovecote's tests classify or summarize, so the Rust side
// is tested against what main.js answers under workerd rather than a hand copy or Bun's text.
// `bun run-workerd.mjs --canned`, which serves each manifest and runs this once per manifest
// (`worked` or `hostile`) with RULES_URL set.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { harnessIn } from "./target.mjs";

const here = import.meta.dirname;
const manifest = process.argv[2];
if (!process.env.RULES_URL || !["worked", "hostile"].includes(manifest)) {
  console.log("FAIL run through bun run-workerd.mjs --canned");
  process.exit(1);
}
const { stringify } = JSON;
const harness = await harnessIn(here);
const send = async (body) =>
  (await harness.fetch(new Request("https://rules/run", { method: "POST",
    body: typeof body === "string" ? body : stringify(body) }))).text();
const write = (name, text) => writeFileSync(join(here, "canned", name + ".json"), text + "\n");

const T0 = 1793380000;
const common = { contract: 1, hook: "telemetry", batch_id: "b", received_at: T0, degraded: false,
  flock: { id: "f1", name: "Bench signs" }, pigeon: { id: "a", name: "Sign a" },
  readings: [{ at: T0, previous: { uptime_s: { value: "86410", at: T0 - 300 } },
    metrics: { reset_cause: "17", reset_stage: "connect", uptime_s: "12",
      swiftly_last_success_age_s: "700" } }] };
const fresh = { pigeon: {}, flock: {} };
// A second sign already down, so R10 has the two voters it needs to fire for the flock.
const states = { r10: { pigeon: {}, flock: { b: { down: true, at: T0 - 60 } } } };
const live = (ids) => ({ mode: "live", wall_ms: 250, common,
  rules: ids.map(([id, version]) => ({ id, version, state: states[id] ?? fresh })) });

if (manifest === "worked") {
  write("live-worked", await send(live([["r6", 1], ["r7", 1], ["r10", 1], ["gone", 2]])));
  write("bad-input", await send("not json"));
  write("harness-failed", await send({ mode: "live", wall_ms: 250, common, rules: 5 }));
  // A dry run's replay, as rule_dry_run.rs's worked_events() builds the same batches.
  const ev = (id, at, metrics) => ({ ...common, received_at: at, pigeon: { id, name: "Sign " + id },
    readings: [{ at, metrics, previous: {} }] });
  const events = [ev("a", T0, { temp_c: "35" }), ev("a", T0 + 60, { temp_c: "45" }),
    ev("a", T0 + 120, { temp_c: "46" }), ev("b", T0 + 130, { temp_c: "50" }),
    ev("a", T0 + 180, { temp_c: "30" }), ev("a", T0 + 240, { reset_cause: "1" })];
  write("replay-worked", await send({ mode: "replay", wall_ms: 250, events,
    rules: [{ id: "starter", version: 1 }, { id: "r7", version: 1 }] }));
} else {
  write("live-hostile",
    await send(live([["big", 1], ["ok", 1], ["lonethrow", 1], ["hang", 1], ["after", 1]])));
}
