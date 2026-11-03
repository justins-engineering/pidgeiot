// Hostile rules through the harness: each failure stays that rule's, and the response is JSON.
// `bun run-hostile.mjs`. The harness imports its manifest as ./rules.js, so it runs from a
// scratch copy holding hostile/ in place of the worked rules.
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// The runner keeps its own JSON before loading the bundle, since h-hang replaces the globals.
const { stringify, parse } = { stringify: JSON.stringify, parse: JSON.parse };
const dir = mkdtempSync(join(tmpdir(), "rules-hostile-"));
for (const f of ["main.js", "prelude.js"]) cpSync(join(import.meta.dirname, f), join(dir, f));
cpSync(join(import.meta.dirname, "hostile"), dir, { recursive: true });
const harness = (await import(join(dir, "main.js"))).default;

let failures = 0;
const check = (name, ok) => { console.log((ok ? "pass " : "FAIL ") + name); if (!ok) failures++; };
const call = async (body) => {
  const res = await harness.fetch(new Request("https://rules/run", { method: "POST",
    body: stringify(body) }));
  return parse(await res.text());
};

const T0 = 1793380000;
const common = { contract: 1, hook: "telemetry", batch_id: "b", received_at: T0, degraded: false,
  flock: { id: "f", name: "F" }, pigeon: { id: "a", name: "A" },
  readings: [{ at: T0, metrics: { k: "1" }, previous: {} }] };
const state = { pigeon: {}, flock: {} };
const ids = ["big", "cyc", "get", "huge", "ok", "lone", "hang", "after"];

const t = performance.now();
const live = await call({ mode: "live", wall_ms: 250, common,
  rules: ids.map((id) => ({ id, version: 1, state })) });
const ms = Math.round(performance.now() - t);
const status = Object.fromEntries(live.results.map((r) => [r.id, r.status]));
check("a BigInt in a result is that rule's bad_output", status.big === "bad_output");
check("a cyclic result is that rule's bad_output", status.cyc === "bad_output");
check("a throwing getter is that rule's bad_output", status.get === "bad_output");
check("a result over 48 KiB is that rule's bad_output", status.huge === "bad_output");
check("a healthy sibling still runs", status.ok === "ok");
const lone = parse(live.results.find((r) => r.id === "lone").out_json ?? "{}").values?.[0];
check("lone surrogates leave as U+FFFD, pairs and backslashes intact",
  lone?.key === "k\ufffd" && lone?.value === "v\ufffd \u{1f600} \\ud800");
check("a hang with the timer globals replaced still times out", status.hang === "timeout");
check("rules after the hang are skipped", status.after === "skipped");
console.log(ms + " ms " + stringify(live.results.map((r) =>
  [r.id, r.status, (r.error ?? "").split("\n")[0]])));

const wide = await call({ mode: "replay", wall_ms: 250, events: [common],
  rules: [{ id: "wide", version: 1 }] });
check("state is capped in UTF-8 bytes, not UTF-16 units",
  wide.rules[0].runs[0].status === "bad_output");

const proto = await call({ mode: "replay", wall_ms: 250, events: [common],
  rules: [{ id: "proto", version: 1 }] });
const kept = proto.rules[0].final.pigeons.a;
check("a replay keeps a state key named __proto__, as live does",
  Object.hasOwn(kept, "__proto__") && kept.__proto__ === "kept");

const broken = await call({ mode: "live", wall_ms: 250, common, rules: 5 });
check("a harness failure still answers JSON", broken.error === "harness_failed");

rmSync(dir, { recursive: true, force: true });
console.log(failures === 0 ? "all hostile checks pass" : failures + " check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
