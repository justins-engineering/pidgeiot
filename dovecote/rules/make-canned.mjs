// Writes canned/*.json: real harness responses that dovecote's tests classify, so the Rust side
// is tested against what main.js answers rather than a hand copy. `bun make-canned.mjs`.
import { cpSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const here = import.meta.dirname;
const { stringify } = JSON;
const send = async (harness, body) =>
  (await harness.fetch(new Request("https://rules/run", { method: "POST",
    body: typeof body === "string" ? body : stringify(body) }))).text();
// Stacks name this checkout and the scratch copy; both are cut so the files are stable.
const write = (name, text) =>
  writeFileSync(join(here, "canned", name + ".json"), text.split(here).join(".") + "\n");

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

const worked = (await import(join(here, "main.js"))).default;
write("live-worked", await send(worked, live([["r6", 1], ["r7", 1], ["r10", 1], ["gone", 2]])));
write("bad-input", await send(worked, "not json"));
write("harness-failed", await send(worked, { mode: "live", wall_ms: 250, common, rules: 5 }));

// The hostile manifest replaces the worked one in a scratch copy, as run-hostile.mjs does.
const dir = mkdtempSync(join(tmpdir(), "rules-canned-"));
try {
  for (const f of ["main.js", "prelude.js"]) cpSync(join(here, f), join(dir, f));
  cpSync(join(here, "hostile"), dir, { recursive: true });
  const hostile = (await import(join(dir, "main.js"))).default;
  const text = await send(hostile,
    live([["big", 1], ["ok", 1], ["lonethrow", 1], ["hang", 1], ["after", 1]]));
  write("live-hostile", text.split(dir).join("."));
} finally {
  rmSync(dir, { recursive: true, force: true });
}
