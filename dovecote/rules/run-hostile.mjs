// Hostile rules through the harness: each failure stays that rule's, and the response is JSON.
// `bun run-hostile.mjs`. The harness imports its manifest as ./rules.js, so it runs from a
// scratch copy holding hostile/ in place of the worked rules (under RULES_URL, from the copy
// run-workerd.mjs serves instead).
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { harnessIn } from "./target.mjs";

// The runner keeps its own JSON before loading the bundle, since h-hang replaces the globals.
const { stringify, parse } = JSON;
// One rule's entry on the wire: its result escaped a second time (at most twice 48 KiB), 20 log
// lines of 256 escaped bytes, and framing. dovecote reads rules x this + 1 KiB.
const RULE_WIRE_BYTES = 103 * 1024;
const LONE = /(?<!\\)(?:\\\\)*\\ud[89a-f][0-9a-f]{2}/;
const escaped = (s) => new TextEncoder().encode(stringify(s)).length - 2;

const dir = mkdtempSync(join(tmpdir(), "rules-hostile-"));
let failures = 0;
const check = (name, ok) => { console.log((ok ? "pass " : "FAIL ") + name); if (!ok) failures++; };
try {
  for (const f of ["main.js", "prelude.js"]) cpSync(join(import.meta.dirname, f), join(dir, f));
  cpSync(join(import.meta.dirname, "hostile"), dir, { recursive: true });
  const harness = await harnessIn(dir);

  const send = async (body) => {
    const res = await harness.fetch(new Request("https://rules/run", { method: "POST",
      body: typeof body === "string" ? body : stringify(body) }));
    return res.text();
  };
  const call = async (body) => parse(await send(body));

  const T0 = 1793380000;
  const common = { contract: 1, hook: "telemetry", batch_id: "b", received_at: T0, degraded: false,
    flock: { id: "f", name: "F" }, pigeon: { id: "a", name: "A" },
    readings: [{ at: T0, metrics: { k: "1" }, previous: {} }] };
  const state = { pigeon: {}, flock: {} };
  const ids = ["big", "cyc", "get", "huge", "ok", "lone", "lonelog", "lonethrow", "hang", "after"];

  const t = performance.now();
  const text = await send({ mode: "live", wall_ms: 250, common,
    rules: ids.map((id) => ({ id, version: 1, state })) });
  const ms = Math.round(performance.now() - t);
  const live = parse(text);
  const status = Object.fromEntries(live.results.map((r) => [r.id, r.status]));
  check("a BigInt in a result is that rule's bad_output", status.big === "bad_output");
  check("a cyclic result is that rule's bad_output", status.cyc === "bad_output");
  check("a throwing getter is that rule's bad_output", status.get === "bad_output");
  check("a result over 48 KiB is that rule's bad_output", status.huge === "bad_output");
  check("a healthy sibling still runs", status.ok === "ok");
  const lone = parse(live.results.find((r) => r.id === "lone").out_json ?? "{}").values?.[0];
  check("lone surrogates leave as U+FFFD, pairs and backslashes intact",
    lone?.key === "k�" && lone?.value === "v� \u{1f600} \\ud800");
  check("a lone surrogate in a log line or a thrown value leaves the response well formed",
    !LONE.test(text) && status.lonelog === "ok" && status.lonethrow === "threw" &&
    live.results.find((r) => r.id === "lonelog").logs[0] === "stage ab�");
  check("a hang with the timer globals replaced still times out", status.hang === "timeout");
  check("rules after the hang are skipped", status.after === "skipped");
  console.log(ms + " ms " + stringify(live.results.map((r) =>
    [r.id, r.status, (r.error ?? "").split("\n")[0]])));

  const dense = await send({ mode: "live", wall_ms: 250, common,
    rules: [{ id: "quotes", version: 1, state }] });
  const [quotes] = parse(dense).results;
  check("a quote-dense result inside 48 KiB stays inside the read cap",
    quotes.status === "ok" && quotes.out_json.length > 48000 &&
    new TextEncoder().encode(dense).length <= RULE_WIRE_BYTES + 1024);
  const [loud] = (await call({ mode: "live", wall_ms: 250, common,
    rules: [{ id: "loud", version: 1, state }] })).results;
  check("log lines and errors are capped on their escaped length",
    quotes.logs.length === 20 && quotes.logs.every((l) => escaped(l) <= 256) &&
    loud.status === "threw" && escaped(loud.error) <= 1024 && escaped(loud.error) > 1000);

  const wide = await call({ mode: "replay", wall_ms: 250, events: [common],
    rules: [{ id: "wide", version: 1 }] });
  check("state is capped in UTF-8 bytes, not UTF-16 units",
    wide.rules[0].runs[0].status === "bad_output");

  const over = { s: "x".repeat(5000), n: 1 };
  const seeded = await call({ mode: "replay", wall_ms: 250, events: [common], rules: [
    { id: "same", version: 1, seed: { pigeons: { a: over }, flock: over } },
    { id: "same", version: 1, seed: { pigeons: { a: { ...over, gone: "y" } }, flock: over } },
  ] });
  check("a document over its cap still takes a patch that does not grow it",
    seeded.rules.every((r) => r.runs[0].status === "ok" && r.final.pigeons.a.n === 2 &&
      r.final.flock.n === 2));

  const proto = await call({ mode: "replay", wall_ms: 250, events: [common],
    rules: [{ id: "proto", version: 1 }] });
  const kept = proto.rules[0].final.pigeons.a;
  check("a replay keeps a state key named __proto__, as live does",
    Object.hasOwn(kept, "__proto__") && kept.__proto__ === "kept");
  check("a replay returns a run whose only effect is state", proto.rules[0].runs.length === 1);

  const typo = await call({ mode: "replay", wall_ms: 250, events: [common, common],
    rules: [{ id: "typo", version: 1 }] });
  const runs = typo.rules[0].runs;
  check("a replay refuses a result with an unknown field and threads none of its state",
    runs.length === 2 && runs.every((r) => r.status === "bad_output" && !("out_json" in r)) &&
    typo.rules[0].final.pigeons.a === undefined);

  const junk = await call("not json");
  check("a body that is not JSON answers bad_input", junk.error === "bad_input");

  const broken = await call({ mode: "live", wall_ms: 250, common, rules: 5 });
  check("a harness failure still answers JSON", broken.error === "harness_failed");
} finally {
  rmSync(dir, { recursive: true, force: true });
}
console.log(failures === 0 ? "all hostile checks pass" : failures + " check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
