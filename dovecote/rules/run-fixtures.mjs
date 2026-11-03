// Runs the worked rules through the harness with synthetic sign events. `bun run-fixtures.mjs`
// or any runtime with fetch's Request and Response. The mail model below mirrors the platform's
// fire/clear/notify rule only so the fixtures can state what would have been mailed.
import harness from "./main.js";

const call = async (body) =>
  (await harness.fetch(new Request("https://rules/run", { method: "POST",
    body: JSON.stringify(body) }))).json();
const outOf = (r) => (r?.out_json ? JSON.parse(r.out_json) : undefined);

let failures = 0;
const check = (name, ok) => { console.log((ok ? "pass " : "FAIL ") + name); if (!ok) failures++; };

const T0 = 1793380000;
const flock = { id: "f1", name: "Bench signs" };
const sign = (id) => ({ id, name: "Sign " + id });
const ev = (pid, at, metrics, previous = {}) => ({
  contract: 1, hook: "telemetry", batch_id: pid + "-" + at, received_at: at, degraded: false,
  flock, pigeon: sign(pid), readings: [{ at, metrics, previous }],
});
const fresh = { pigeon: {}, flock: {} };

// The platform's mail decision: fire and clear mail on a change of the firing flag, notify
// once per key.
function mails(runs, events) {
  const firing = new Map();
  const seen = new Set();
  const sent = [];
  for (const run of runs) {
    for (const a of outOf(run)?.alerts ?? []) {
      const scope = a.scope === "flock" ? "flock" : "p:" + events[run.i].pigeon.id;
      const k = scope + "/" + a.key;
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

// R6, live, the bundle: a combined mask with the stage in the same report.
{
  const common = ev("a", T0, { reset_cause: "17", reset_stage: "connect", uptime_s: "12" },
    { uptime_s: { value: "86410", at: T0 - 300 } });
  const live = await call({ mode: "live", wall_ms: 250, common, rules: [
    { id: "r6", version: 1, state: fresh },
    { id: "r7", version: 1, state: fresh },
    { id: "r10", version: 1, state: fresh },
    { id: "gone", version: 2, state: fresh },
  ] });
  const r6 = outOf(live.results.find((r) => r.id === "r6"));
  check("R6 mails on the first watchdog boot, mask 17",
    r6.alerts.length === 1 && r6.alerts[0].body.includes("connect"));
  check("R6 keeps the stage in state", r6.state?.pigeon?.stage === "connect");
  const r7 = outOf(live.results.find((r) => r.id === "r7"));
  check("R7 counts one boot, writes boots_24h", r7.values?.[0]?.value === "1");
  check("R10 does nothing without the Swiftly key",
    live.results.find((r) => r.id === "r10").out_json === "{}");
  check("an undeployed rule is a version mismatch",
    live.results.find((r) => r.id === "gone").status === "version_mismatch");
}

// R6: no watchdog bit, no mail; stage arriving in a later report of the same batch still names it.
{
  const quiet = await call({ mode: "live", wall_ms: 250, common: ev("a", T0, { reset_cause: "1" }),
    rules: [{ id: "r6", version: 1, state: fresh }] });
  check("R6 ignores a reset without the watchdog bit", outOf(quiet.results[0]).alerts.length === 0);
  const split = ev("a", T0, { reset_cause: "16" });
  split.readings.push({ at: T0 + 5, metrics: { reset_stage: "fetch" }, previous: {} });
  const r = await call({ mode: "live", wall_ms: 250, common: split,
    rules: [{ id: "r6", version: 1, state: fresh }] });
  check("R6 names a stage reported separately",
    outOf(r.results[0]).alerts[0].body.includes("fetch"));
}

// R7, replay: four boots in ten hours fire once, a duplicate delivery counts nothing, and the
// loop clears once the boots age out.
{
  const events = [];
  for (let n = 0; n < 5; n++) events.push(ev("a", T0 + n * 9000, { reset_cause: "1" }));
  events.push(ev("a", T0 + 4 * 9000, { reset_cause: "1" })); // duplicate of the fifth
  const later = T0 + 2 * 86400;
  const uptime = { uptime_s: { value: "89700", at: later - 300 } };
  events.push(ev("a", later, { uptime_s: "90000" }, uptime));
  const out = await call({ mode: "replay", wall_ms: 250, events,
    rules: [{ id: "r7", version: 1 }] });
  const rule = out.rules[0];
  const sent = mails(rule.runs, events);
  check("R7 fires once, on the fourth boot", sent.filter((m) => m.kind === "fire").length === 1 &&
    sent.find((m) => m.kind === "fire").i === 3);
  check("R7 counts the duplicate delivery once", rule.final.pigeons.a.boots.length === 0 &&
    outOf(rule.runs.find((r) => r.i === 5)).values === undefined);
  check("R7 clears after the boots age out", sent.at(-1).kind === "clear" && sent.at(-1).i === 6);
}

// R10, replay over three signs: one mail when half go stale, none while it stays down, one when
// the feed is back.
{
  const age = (pid, at, s) => ev(pid, at, { swiftly_last_success_age_s: String(s) });
  const events = [
    age("a", T0, 30), age("b", T0 + 10, 30), age("c", T0 + 20, 30),
    age("a", T0 + 300, 700), age("b", T0 + 310, 700), age("c", T0 + 320, 700),
    age("a", T0 + 600, 20), age("b", T0 + 610, 20), age("c", T0 + 620, 20),
  ];
  const out = await call({ mode: "replay", wall_ms: 250, events,
    rules: [{ id: "r10", version: 1 }] });
  const sent = mails(out.rules[0].runs, events);
  check("R10 mails once on the fleet going stale",
    sent.filter((m) => m.kind === "fire").length === 1 && sent[0].i === 4);
  check("R10 mails once when the feed is back",
    sent.length === 2 && sent[1].kind === "clear" && sent[1].i === 7);
  check("R10 keeps one entry per sign", Object.keys(out.rules[0].final.flock).length === 3);
}

// A device resend: the same report again 2 s later, stamped at its own receive time, so a new
// batch_id and new reading times. R6 mails once and R7 counts one boot.
{
  const boot = { reset_cause: "16", uptime_s: "5" };
  const events = [ev("a", T0, boot), ev("a", T0 + 2, boot)];
  const out = await call({ mode: "replay", wall_ms: 250, events,
    rules: [{ id: "r6", version: 1 }, { id: "r7", version: 1 }] });
  check("R6 mails once for a resent boot report",
    mails(out.rules[0].runs, events).length === 1);
  check("R7 counts a resent boot report once", out.rules[1].final.pigeons.a.boots.length === 1);
}

// R7: two real boots whose reset_cause and uptime_s arrive in separate reports count as two;
// four boots a minute apart count as four.
{
  const events = [
    ev("a", T0 + 45, { reset_cause: "1" }),
    ev("a", T0 + 300, { uptime_s: "300" }, { uptime_s: { value: "86400", at: T0 - 300 } }),
    ev("a", T0 + 3645, { reset_cause: "1" }),
    ev("a", T0 + 3900, { uptime_s: "300" }, { uptime_s: { value: "3300", at: T0 + 3300 } }),
  ];
  const out = await call({ mode: "replay", wall_ms: 250, events,
    rules: [{ id: "r7", version: 1 }] });
  const rule = out.rules[0];
  check("R7 counts split boot reports as two boots, no fire",
    rule.final.pigeons.a.boots.length === 2 &&
    mails(rule.runs, events).every((m) => m.kind !== "fire"));
  const loop = [0, 60, 120, 180].map((s) => ev("a", T0 + s, { reset_cause: "1", uptime_s: "20" }));
  const fast = await call({ mode: "replay", wall_ms: 250, events: loop,
    rules: [{ id: "r7", version: 1 }] });
  check("R7 counts boots a minute apart", mails(fast.rules[0].runs, loop).some((m) =>
    m.kind === "fire" && m.i === 3));
}

// R10: the batch's last reading lacks the key; an earlier reading still decides.
{
  const common = ev("a", T0, { swiftly_last_success_age_s: "700" });
  common.readings.push({ at: T0 + 5, metrics: { uptime_s: "10" }, previous: {} });
  const live = await call({ mode: "live", wall_ms: 250, common,
    rules: [{ id: "r10", version: 1, state: fresh }] });
  check("R10 reads the newest reading that carries the key",
    outOf(live.results[0]).state?.flock?.a?.down === true);
}

// A replay seeded with live state: a shape an older version stored does not throw, and the
// boots already stored count.
{
  const one = [ev("a", T0, { reset_cause: "1" })];
  const odd = await call({ mode: "replay", wall_ms: 250, events: one,
    rules: [{ id: "r7", version: 1, seed: { pigeons: { a: { boots: "3 boots" } }, flock: {} } }] });
  check("R7 survives a state shape an older version wrote", odd.rules[0].runs[0].status === "ok");
  const boots = [T0 - 900, T0 - 600, T0 - 300];
  const seeded = await call({ mode: "replay", wall_ms: 250, events: one,
    rules: [{ id: "r7", version: 1, seed: { pigeons: { a: { boots } }, flock: {} } }] });
  check("a seeded replay fires on the boot that makes four",
    mails(seeded.rules[0].runs, one).some((m) => m.kind === "fire" && m.i === 0));
}

// R6 against what dovecote refuses: a C1 control in device text, and two watchdog boots in one
// batch, 20 s apart, whose times round to one minute's key.
{
  const c1 = ev("a", T0, { reset_cause: "16", reset_stage: "fetch\u0085tls", uptime_s: "5" });
  const one = await call({ mode: "live", wall_ms: 250, common: c1,
    rules: [{ id: "r6", version: 1, state: fresh }] });
  check("R6 strips C1 controls from device text",
    !/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/.test(outOf(one.results[0]).alerts[0].body));
  const two = ev("a", T0 + 5, { reset_cause: "16", uptime_s: "5" });
  two.readings.push({ at: T0 + 25, metrics: { reset_cause: "16", uptime_s: "5" }, previous: {} });
  const r = await call({ mode: "live", wall_ms: 250, common: two,
    rules: [{ id: "r6", version: 1, state: fresh }] });
  const keys = outOf(r.results[0]).alerts.map((a) => a.key);
  check("R6 gives a minute's key once in one result",
    keys.length === 1 && outOf(r.results[0]).state.pigeon.lastBoot === T0 + 20);
}

console.log(failures === 0 ? "all fixtures pass" : failures + " fixture(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
