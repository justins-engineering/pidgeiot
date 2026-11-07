// The rule builder's interpreter. Every builder rule is a three-line module handing its model
// here, so this file is what all of them run, and a change to it changes every one at once.
// The model was validated by dovecote (capsules/src/rule_model.rs holds its shape and the
// meaning of each row); the event and the stored state were not, so both are read defensively.
import { bits, held, num, within } from "./pidgeiot.js";

const SUBJECT_BYTES = 120;
const BODY_BYTES = 1024;
const VALUE_BYTES = 1024;
const KEEP_TIMES = 32; // one more than the highest count a row may name
const encoder = new TextEncoder();
const decoder = new TextDecoder();

const own = (o, k) => o !== null && typeof o === "object" && Object.hasOwn(o, k);
const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

// A stored document of another shape, or one an older model wrote, reads as empty where it
// does not fit; null-prototype copies take any key as a key.
function table(v, ok) {
  const out = Object.create(null);
  if (!isObject(v)) return out;
  for (const [k, x] of Object.entries(v)) if (ok(x)) out[k] = x;
  return out;
}

// A trailing partial character decodes to U+FFFD, so 3 bytes are held back for it.
function cut(s, max) {
  const b = encoder.encode(s);
  return b.length <= max ? s : decoder.decode(b.subarray(0, max - 3));
}

const controls = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g;

// `{pigeon}`, `{flock}` and `{<key>}` take the names and the key's latest value; any other
// braces stay as typed. Device text can hold anything, so control characters become spaces.
function render(text, event, latest) {
  return text.replace(/\{([^{}]+)\}/g, (whole, name) => {
    if (name === "pigeon") return event.pigeon.name;
    if (name === "flock") return event.flock.name;
    return own(latest, name) ? latest[name] : whole;
  });
}

const subjectOf = (s) => cut(s.replace(/[\n]/g, " ").replace(controls, " "), SUBJECT_BYTES);
const bodyOf = (s) => cut(s.replace(controls, " "), BODY_BYTES);
const valueOf = (s) => cut(s.replace(/\u0000/g, " "), VALUE_BYTES);

// 32-bit FNV-1a, so the state remembers which value it last wrote in 8 characters.
function digest(s) {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193);
  return (h >>> 0).toString(16);
}

function compare(c, latest) {
  if (!own(latest, c.key)) return false;
  const left = latest[c.key];
  let right;
  let fromKey = false;
  if (own(c.to, "number")) right = c.to.number;
  else if (own(c.to, "text")) right = c.to.text;
  else if (own(latest, c.to.key)) {
    right = latest[c.to.key];
    fromKey = true;
  } else return false;
  if (typeof right === "string" && (c.op === "equals" || c.op === "not_equals")) {
    const a = num(left);
    const b = num(right);
    const same = fromKey && a !== null && b !== null ? a === b : left === right;
    return c.op === "equals" ? same : !same;
  }
  const a = num(left);
  const b = num(right);
  if (a === null || b === null) return false;
  switch (c.op) {
    case "above": return a > b;
    case "below": return a < b;
    case "at_least": return a >= b;
    case "at_most": return a <= b;
    case "equals": return a === b;
    case "not_equals": return a !== b;
  }
  return false;
}

// A row without its hold or count, which a value table may use too.
function plain(c, reading, latest) {
  switch (c.is) {
    case "compare": return compare(c, latest);
    case "between": {
      const n = own(latest, c.key) ? num(latest[c.key]) : null;
      return n !== null && n >= c.low && n <= c.high;
    }
    case "changed":
      return own(reading.metrics, c.key) && own(reading.previous, c.key) &&
        reading.metrics[c.key] !== reading.previous[c.key].value;
    case "changed_by": {
      if (!own(reading.metrics, c.key) || !own(reading.previous, c.key)) return false;
      const now = num(reading.metrics[c.key]);
      const before = num(reading.previous[c.key].value);
      return now !== null && before !== null && Math.abs(now - before) > c.more_than;
    }
    case "bit_set": return own(latest, c.key) && bits(latest[c.key], 2 ** c.bit);
  }
  return false;
}

/** Runs `model` over one telemetry batch and returns the contract result. */
export function run(model, event) {
  if (model?.v !== 1) throw new Error("builder model version not known");
  const trigger = model.when.telemetry.key;
  const rows = model.only_if ?? [];
  const then = model.then ?? [];
  const otherwise = model.otherwise ?? [];
  const any = model.match === "any";

  const saved = isObject(event.state.pigeon) ? event.state.pigeon : {};
  const before = {
    t: saved.t === true,
    e: Number.isInteger(saved.e) ? saved.e : 0,
    o: Number.isInteger(saved.o) ? saved.o : 0,
  };
  const holds = table(saved.h, Number.isFinite);
  const counts = table(saved.c, (x) => Array.isArray(x) && x.every(Number.isFinite));
  const written = table(saved.v, (x) => typeof x === "string");
  const valueKeys = [...new Set([...then, ...otherwise]
    .filter((a) => a.do === "set_value").map((a) => a.key))].sort();

  let truth = before.t;
  let starts = before.e;
  let stops = before.o;
  const latest = Object.create(null);
  let started = null; // the newest Then start inside this batch, by number
  let stopped = null; // the newest Otherwise start
  let decided = false;
  const values = new Map();
  for (const reading of event.readings) {
    for (const [k, v] of Object.entries(reading.metrics)) latest[k] = v;
    rows.forEach((c, i) => {
      if (c.is !== "counted" || !own(reading.metrics, c.key)) return;
      // Kept against the newest time seen, so a batch that arrives late drops nothing newer.
      const times = [...(counts[i] ?? []), reading.at];
      counts[i] = within(times, Math.max(...times), c.within_secs).slice(-KEEP_TIMES);
    });
    if (!own(reading.metrics, trigger)) continue;
    // Every row is decided, not short-circuited, so each hold keeps its own start.
    const results = rows.map((c, i) => {
      if (c.is === "counted") {
        return within(counts[i] ?? [], reading.at, c.within_secs).length > c.more_than;
      }
      const now = plain(c, reading, latest);
      if (!(c.for_secs > 0)) return now;
      const h = held(own(holds, i) ? holds[i] : null, now, reading.at, c.for_secs);
      if (h.since === null) delete holds[i];
      else holds[i] = h.since;
      return h.held;
    });
    const ok = rows.length === 0 || (any ? results.some(Boolean) : results.every(Boolean));
    if (ok && !truth) started = ++starts;
    if (!ok && truth) stopped = ++stops;
    truth = ok;
    decided = true;
    for (const a of ok ? then : otherwise) {
      if (a.do !== "set_value") continue;
      const band = (a.bands ?? []).find((b) => plain(b.if, reading, latest));
      values.set(a.key, { value: valueOf(render(band ? band.value : a.value, event, latest)),
        at: reading.at });
    }
  }
  // A batch without the trigger decides nothing, but the counts it added are kept.
  const alerts = [];
  const mail = (actions, prefix, on, start) => actions.forEach((a, i) => {
    if (a.do !== "mail") return;
    const subject = subjectOf(render(a.subject, event, latest)) || "Rule";
    const body = a.body ? bodyOf(render(a.body, event, latest)) : undefined;
    if (a.repeat === "once") {
      if (start === null) return;
      alerts.push({ key: prefix + i + "-" + start, kind: "notify", severity: a.severity,
        scope: "pigeon", subject, body });
    } else {
      alerts.push(on
        ? { key: prefix + i, kind: "fire", severity: a.severity, scope: "pigeon", subject, body }
        : { key: prefix + i, kind: "clear", severity: "info", scope: "pigeon",
          subject: subjectOf("Back to normal: " + subject), body });
    }
  });
  if (decided) {
    mail(then, "then-", truth, started);
    mail(otherwise, "else-", !truth, stopped);
  }

  const out = [];
  const digests = Object.create(null);
  for (const [k, d] of Object.entries(written)) digests[k] = d;
  for (const [key, v] of values) {
    const slot = String(valueKeys.indexOf(key));
    const d = digest(v.value);
    if (digests[slot] === d) continue;
    digests[slot] = d;
    out.push({ key, value: v.value, at: v.at });
  }

  const patch = {};
  if (truth !== before.t) patch.t = truth;
  if (starts !== before.e) patch.e = starts;
  if (stops !== before.o) patch.o = stops;
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const docOf = (t) => (Object.keys(t).length > 0 ? { ...t } : null);
  for (const [name, now, was] of [["h", holds, saved.h], ["c", counts, saved.c],
    ["v", digests, saved.v]]) {
    const doc = docOf(now);
    if (!same(doc, isObject(was) && Object.keys(was).length > 0 ? was : null)) patch[name] = doc;
  }

  const result = {};
  if (alerts.length > 0) result.alerts = alerts;
  if (out.length > 0) result.values = out;
  if (Object.keys(patch).length > 0) result.state = { pigeon: patch };
  return result;
}
