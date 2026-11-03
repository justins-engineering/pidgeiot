// Entrypoint of every rules script. It shares an isolate with user code, so it is a
// convenience and never a boundary: dovecote re-validates everything it returns and counts
// a failed dispatch against this flock's rules.
import P from "./prelude.js";
import RULES from "./rules.js";

const CONTRACT = 1;
const LOG_LINES = 20;
const LOG_BYTES = 256;
const ERROR_BYTES = 1024;
const RESULT_BYTES = 49152;
const PIGEON_STATE_BYTES = 4096;
const FLOCK_STATE_BYTES = 32768;
const TIMEOUT = Symbol("timeout");
const encoder = new P.TextEncoder();
const decoder = new P.TextDecoder();

const reply = (body) =>
  new P.Response(P.stringify(body), { headers: { "content-type": "application/json" } });

export default {
  async fetch(request) {
    try {
      let input;
      try {
        input = await request.json();
      } catch {
        return reply({ contract: CONTRACT, error: "bad_input" });
      }
      return reply(input.mode === "replay" ? await replay(input) : await live(input));
    } catch {
      return reply({ contract: CONTRACT, error: "harness_failed" });
    }
  },
};

// Caps count UTF-8 bytes, as dovecote does, not UTF-16 units.
const bytes = (s) => encoder.encode(s).length;

// JSON.stringify escapes a lone UTF-16 surrogate, which dovecote refuses as not JSON, and a
// rule cutting text with slice() makes one easily. Pairs are written raw, so any surrogate
// escape not preceded by an odd run of backslashes is lone.
const wellFormed = (json) =>
  json.replace(/(?<!\\)((?:\\\\)*)\\ud[89a-f][0-9a-f]{2}/g, "$1\\ufffd");

// A trailing partial character decodes to one U+FFFD (3 bytes), hence the 3 held back.
function cut(s, max) {
  const b = encoder.encode(s);
  return b.length <= max ? s : decoder.decode(b.subarray(0, max - 3));
}

function why(e) {
  try {
    return String(e?.stack ?? e);
  } catch {
    return "unprintable error";
  }
}

// One budget for the whole dispatch, so twenty slow rules cannot add up past it.
function budget(ms) {
  let timer;
  const expired = new P.Promise((resolve) => {
    timer = P.setTimeout(() => resolve(TIMEOUT), ms);
  });
  return { expired, stop: () => P.clearTimeout(timer) };
}

// Serialized here, per rule, so a result that is not JSON is that rule's bad_output and never
// the whole dispatch's failure.
async function runOne(run, event, expired) {
  const logs = [];
  const original = console.log;
  console.log = (...args) => {
    if (logs.length < LOG_LINES) logs.push(cut(args.map(String).join(" "), LOG_BYTES));
  };
  try {
    const out = await P.race([new P.Promise((ok) => ok(run(event))), expired]);
    if (out === TIMEOUT) return { status: "timeout", logs };
    let json;
    try {
      json = wellFormed(P.stringify(out ?? {}));
    } catch (e) {
      const error = cut("result is not JSON: " + why(e), ERROR_BYTES);
      return { status: "bad_output", error, logs };
    }
    if (typeof json !== "string" || json[0] !== "{") {
      return { status: "bad_output", error: "result is not an object", logs };
    }
    if (bytes(json) > RESULT_BYTES) {
      return { status: "bad_output", error: "result over 48 KiB", logs };
    }
    return { status: "ok", out_json: json, logs };
  } catch (e) {
    return { status: "threw", error: cut(why(e), ERROR_BYTES), logs };
  } finally {
    console.log = original;
  }
}

async function live(input) {
  const { expired, stop } = budget(input.wall_ms);
  const results = [];
  let late = false;
  for (const want of input.rules) {
    const rule = RULES[want.id];
    if (!rule || rule.version !== want.version) {
      results.push({ id: want.id, status: "version_mismatch" });
      continue;
    }
    if (late) {
      results.push({ id: want.id, status: "skipped" });
      continue;
    }
    const event = P.structuredClone({
      ...input.common,
      rule: { id: want.id, version: want.version },
      state: want.state,
    });
    const r = await runOne(rule.run, event, expired);
    if (r.status === "timeout") late = true;
    results.push({ id: want.id, ...r });
  }
  stop();
  return { contract: CONTRACT, results };
}

// The two state caps are mirrored here only so a replay threads state as live would. A
// null-prototype document takes a key named __proto__ as a key, as dovecote's merge does.
function merge(doc, patch, cap) {
  if (patch === undefined) return { doc };
  if (patch === null || typeof patch !== "object" || Array.isArray(patch)) return { bad: true };
  const next = Object.assign(Object.create(null), doc);
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete next[k];
    else next[k] = v;
  }
  return bytes(P.stringify(next)) > cap ? { bad: true } : { doc: next };
}

async function replay(input) {
  const rules = [];
  for (const want of input.rules) {
    const rule = RULES[want.id];
    if (!rule || rule.version !== want.version) {
      rules.push({ id: want.id, status: "version_mismatch" });
      continue;
    }
    // A seed is the live state, so a new version meets what the old one stored.
    const pigeons = P.structuredClone(want.seed?.pigeons ?? {});
    let flock = P.structuredClone(want.seed?.flock ?? {});
    const runs = [];
    let stopped = null;
    for (let i = 0; i < input.events.length && stopped === null; i++) {
      const common = input.events[i];
      const pid = common.pigeon.id;
      const event = P.structuredClone({
        ...common,
        rule: { id: want.id, version: want.version },
        state: { pigeon: pigeons[pid] ?? {}, flock },
      });
      const { expired, stop } = budget(input.wall_ms);
      const r = await runOne(rule.run, event, expired);
      stop();
      if (r.status === "timeout") stopped = { at: i, status: "timeout" };
      const out = r.status === "ok" ? P.parse(r.out_json) : {};
      if (r.status === "ok") {
        const p = merge(pigeons[pid] ?? {}, out.state?.pigeon, PIGEON_STATE_BYTES);
        const f = merge(flock, out.state?.flock, FLOCK_STATE_BYTES);
        if (p.bad || f.bad) {
          r.status = "bad_output";
          r.error = "state over its cap";
        } else {
          pigeons[pid] = p.doc;
          flock = f.doc;
        }
      }
      const quiet =
        r.status === "ok" && !out.alerts?.length && !out.values?.length && !r.logs.length;
      if (!quiet) runs.push({ i, ...r });
    }
    rules.push({ id: want.id, status: "ok", runs, stopped, final: { pigeons, flock } });
  }
  return { contract: CONTRACT, rules };
}
