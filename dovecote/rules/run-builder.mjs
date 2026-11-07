// Runs the builder's fixture models (b-*.js, written by dovecote's compile and pinned by
// `the_builder_fixtures_are_what_dovecote_writes`) through the harness and builder.js. `bun
// run-builder.mjs`, or under workerd through run-workerd.mjs. The mail model mirrors the
// platform's fire, clear and notify rule only so the checks can state what would be mailed.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { harnessIn } from "./target.mjs";

const here = import.meta.dirname;
const harness = await harnessIn(here);
const call = async (body) =>
  (await harness.fetch(new Request("https://rules/run", { method: "POST",
    body: JSON.stringify(body) }))).json();
const outOf = (r) => (r?.out_json ? JSON.parse(r.out_json) : undefined);

let failures = 0;
const check = (name, ok) => { console.log((ok ? "pass " : "FAIL ") + name); if (!ok) failures++; };

const T0 = 1793380000;
const flock = { id: "f1", name: "Bench signs" };
const ev = (pid, at, metrics, previous = {}) => ({
  contract: 1, hook: "telemetry", batch_id: pid + "-" + at, received_at: at, degraded: false,
  flock, pigeon: { id: pid, name: "Sign " + pid }, readings: [{ at, metrics, previous }],
});
const fresh = { pigeon: {}, flock: {} };
const live = async (id, common, state = fresh) =>
  (await call({ mode: "live", wall_ms: 250, common, rules: [{ id, version: 1, state }] }))
    .results[0];
const replay = async (id, events, seed) =>
  (await call({ mode: "replay", wall_ms: 250, events, rules: [{ id, version: 1, seed }] }))
    .rules[0];

function mails(runs, events) {
  const firing = new Map();
  const seen = new Set();
  const sent = [];
  for (const run of runs) {
    for (const a of outOf(run)?.alerts ?? []) {
      const k = events[run.i].pigeon.id + "/" + a.key;
      if (a.kind === "notify") {
        if (!seen.has(k)) { seen.add(k); sent.push({ i: run.i, ...a }); }
      } else {
        const now = a.kind === "fire";
        if ((firing.get(k) ?? false) !== now) {
          if (now || firing.has(k)) sent.push({ i: run.i, ...a });
          firing.set(k, now);
        }
      }
    }
  }
  return sent;
}
const values = (runs) => runs.flatMap((r) => (outOf(r)?.values ?? []).map((v) => ({ i: r.i, ...v })));

// Hot sign: held five minutes before it mails, the fan follows the branch, and a value is
// written only when it changes.
{
  const t = (at, c) => ev("a", at, { temp_c: String(c) });
  const events = [t(T0, 41), t(T0 + 120, 42), t(T0 + 300, 41), t(T0 + 600, 20), t(T0 + 900, 20)];
  const rule = await replay("b_hot", events);
  const sent = mails(rule.runs, events);
  check("a hold waits its five minutes before it mails",
    sent.length === 2 && sent[0].kind === "fire" && sent[0].i === 2);
  check("the subject fills its placeholders", sent[0].subject === "Sign running hot: Sign a at 41 C");
  check("it mails back to normal once the condition stops",
    sent[1].kind === "clear" && sent[1].i === 3 && sent[1].subject.startsWith("Back to normal: "));
  const fan = values(rule.runs).map((v) => v.i + ":" + v.value).join(",");
  check("the fan follows Then and Otherwise, written only on a change", fan === "0:auto,2:on,3:auto");
  check("a quiet repeat stores nothing new", outOf(rule.runs.find((r) => r.i === 4))?.state ===
    undefined);
  const broken = [t(T0, 41), t(T0 + 200, 39), t(T0 + 320, 41), t(T0 + 500, 41)];
  check("a break restarts the hold",
    mails((await replay("b_hot", broken)).runs, broken).every((m) => m.kind !== "fire"));
  check("a batch without the trigger key does nothing",
    (await live("b_hot", ev("a", T0, { lux: "3" }))).out_json === "{}");
  const batch = ev("a", T0, { temp_c: "41" });
  batch.readings.push({ at: T0 + 400, metrics: { temp_c: "41" }, previous: {} });
  const one = outOf(await live("b_hot", batch));
  check("readings inside one batch hold as separate batches would",
    one.alerts[0].kind === "fire" && one.values[0].value === "on" && one.values[0].at === T0 + 400);
}

// Reboot loop: more than three resets in a day fires, ageing out clears, and a batch that
// arrives late drops none of the newer times.
{
  const boot = (at) => ev("a", at, { reset_cause: "1" });
  const events = [0, 1, 2, 3].map((n) => boot(T0 + n * 9000));
  events.push(boot(T0 + 2 * 86400));
  const rule = await replay("b_reboots", events);
  const sent = mails(rule.runs, events);
  check("the fourth reset in a day fires", sent[0]?.kind === "fire" && sent[0].i === 3 &&
    sent[0].body === "Sign a booted more than three times in a day.");
  check("it clears once the resets age out", sent[1]?.kind === "clear" && sent[1].i === 4);
  const late = [boot(T0 + 3000), boot(T0 + 1000), boot(T0 + 2000), boot(T0 + 4000)];
  const kept = (await replay("b_reboots", late)).final.pigeons.a.c["0"];
  check("a late batch keeps the newer times", kept.length === 4 && kept.at(-1) === T0 + 4000);
}

// A count of another key than the trigger keeps the reports that arrive in their own batches.
{
  const boot = (at) => ev("a", at, { reset_cause: "1" });
  const events = [boot(T0), boot(T0 + 60), boot(T0 + 120), ev("a", T0 + 600, { temp_c: "20" })];
  const rule = await replay("b_counted", events);
  check("a batch without the trigger keeps its counts and mails nothing",
    rule.runs.filter((r) => r.i < 3).length === 3 &&
    rule.runs.filter((r) => r.i < 3).every((r) => outOf(r).state && !outOf(r).alerts));
  const sent = mails(rule.runs, events);
  check("the trigger then mails on the counts kept",
    sent.length === 1 && sent[0].kind === "fire" && sent[0].i === 3);
}

// A value table: rows checked top to bottom, the plain value otherwise.
{
  const lux = (at, n) => ev("a", at, { lux: String(n) });
  const events = [lux(T0, 10), lux(T0 + 60, 1000), lux(T0 + 120, 200), lux(T0 + 180, 250)];
  const got = values((await replay("b_bands", events)).runs).map((v) => v.value).join(",");
  check("a table picks its first matching row, else its value", got === "30,80,60");
}

// Any of several rows; a once mail per start, even a start inside one batch.
{
  const m = (at, metrics, previous) => ev("a", at, { reset_cause: "0", mode: "auto", ...metrics },
    previous);
  const quiet = (at) => m(at, {});
  const events = [
    m(T0, { reset_cause: "16" }),
    quiet(T0 + 60),
    m(T0 + 120, { layout: "b" }, { layout: { value: "a", at: T0 } }),
    m(T0 + 180, { temp_c: "30" }, { temp_c: { value: "20", at: T0 } }),
    quiet(T0 + 240),
    m(T0 + 300, { mode: "night" }),
    quiet(T0 + 360),
    m(T0 + 420, { temp_in: "25", temp_out: "21" }),
    m(T0 + 480, { temp_in: "20", temp_out: "21" }),
  ];
  const rule = await replay("b_mixed", events);
  const keys = mails(rule.runs, events).map((s) => s.i + ":" + s.key).join(",");
  check("each start of any row mails once under its own key, a held row only once",
    keys === "0:then-0-1,2:then-0-2,5:then-0-3,7:then-0-4");
  const first = mails(rule.runs, events)[0];
  check("a body keeps its newline and leaves unknown braces as typed",
    first.body === "reset_cause 16\nlayout {layout} {unknown}");
  const spike = ev("a", T0, { reset_cause: "16", mode: "auto" });
  spike.readings.push({ at: T0 + 5, metrics: { reset_cause: "0", mode: "auto" }, previous: {} });
  const out = outOf(await live("b_mixed", spike));
  check("a start and stop inside one batch still mails once",
    out.alerts.length === 1 && out.alerts[0].key === "then-0-1" && out.state.pigeon.e === 1 &&
    !("t" in out.state.pigeon));
  check("Otherwise sets its value from the reading",
    out.values[0].key === "last_cause" && out.values[0].value === "0");
  const control = ev("a", T0, { reset_cause: "0", mode: "auto", layout: "x\u0001y" });
  control.readings[0].metrics.mode = "night\u0007";
  const c = outOf(await live("b_mixed", control));
  check("device text loses its control characters in a mail",
    !/[\u0000-\u0009\u000b-\u001f]/.test(c.alerts[0].subject + c.alerts[0].body));
}

// Stored state of any shape reads as fresh where it does not fit, and inherited names are no
// keys.
{
  const odd = { pigeon: { t: "yes", e: -1.5, o: "2", h: [1], c: { 0: "x" }, v: 5 }, flock: {} };
  const r = await live("b_reboots", ev("a", T0, { reset_cause: "1" }), odd);
  check("state of another shape does not throw", r.status === "ok");
  const proto = await live("b_mixed", ev("a", T0, { constructor: "1", toString: "2" }));
  check("an inherited name is not a reported key", proto.out_json === "{}");
}

// The test cases dovecote makes from b-hot.js's model, expanded as the test route expands them,
// mail what they expect.
{
  const cases = JSON.parse(readFileSync(join(here, "canned", "builder-cases.json"), "utf8"));
  for (const c of cases) {
    const rule = await replay("b_hot", c.events);
    const got = mails(rule.runs, c.events).map((m) => m.key + " " + m.kind).sort().join(",");
    const want = c.expect.map((m) => m.key + " " + m.kind).sort().join(",");
    check("the made case “" + c.name + "” mails what it expects",
      got === want && rule.runs.every((r) => r.status === "ok"));
  }
}

console.log(failures === 0 ? "all builder checks pass" : failures + " builder check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
