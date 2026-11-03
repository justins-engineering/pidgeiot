// Fires once for the flock when at least half the reporting signs have stale departures.
// The failure counter stays 0 through a feed 5xx, so the age of the last success is the
// signal. Each sign writes only its own entry, so two signs reporting at once lose nothing.
import { num } from "./pidgeiot.js";

const STALE_SECS = 600; // the sign blanks its departures at this age
const FRESH_SECS = 1800; // a sign not heard from for this long has no vote
const REWRITE_SECS = 600; // refresh our own entry this often, so it never looks silent

export default function swiftlyDown(event) {
  // The newest reading that carries the key, wherever it sits in the batch.
  let last = null;
  for (const r of event.readings) {
    if (num(r.metrics.swiftly_last_success_age_s) !== null) last = r;
  }
  if (last === null) return {}; // no success since boot, or not a sign
  const age = num(last.metrics.swiftly_last_success_age_s);

  const id = event.pigeon.id;
  const before = event.state.flock[id];
  const me = { down: age >= STALE_SECS, at: last.at };
  const patch = {};
  if (!before || before.down !== me.down || me.at - before.at >= REWRITE_SECS) patch[id] = me;

  let voters = 0;
  let down = 0;
  for (const [key, sign] of Object.entries({ ...event.state.flock, [id]: me })) {
    if (event.received_at - sign.at > FRESH_SECS) {
      patch[key] = null;
      continue;
    }
    voters += 1;
    if (sign.down) down += 1;
  }

  const firing = voters >= 2 && down * 2 >= voters;
  const result = {
    alerts: [{
      key: "swiftly-down",
      kind: firing ? "fire" : "clear",
      severity: firing ? "critical" : "info",
      scope: "flock",
      subject: firing ? "Swiftly feed down" : "Swiftly feed back",
      body: down + " of " + voters + " reporting signs have stale departures.",
    }],
  };
  if (Object.keys(patch).length > 0) result.state = { flock: patch };
  return result;
}
