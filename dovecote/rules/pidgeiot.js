// Helpers for PidgeIoT rules. Pure: no I/O, no clock, no module state.

/** The value as a finite number, else null. Values arrive as strings, and Number("") is 0. */
export function num(v) {
  if (typeof v === "number") return Number.isFinite(v) ? v : null;
  if (typeof v !== "string" || v.trim() === "") return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
}

/** True when the value is an integer with any bit of `mask` set. */
export function bits(v, mask) {
  const n = num(v);
  return n !== null && Number.isInteger(n) && (n & mask) !== 0;
}

/** The distinct times in `[now - secs, now]`, oldest first: the windowed-count primitive. */
export function within(times, now, secs) {
  const from = now - secs;
  return [...new Set(times)]
    .filter((t) => Number.isFinite(t) && t >= from && t <= now)
    .sort((a, b) => a - b);
}

/**
 * Tracks how long `cond` has held. Pass back the returned `since` next time (store it in
 * state); `held` turns true once `cond` has been true for `secs` without a break. A stored
 * `since` that is not a number restarts the hold.
 */
export function held(since, cond, at, secs) {
  if (!cond) return { since: null, held: false };
  const start = Number.isFinite(since) ? since : at;
  return { since: start, held: at - start >= secs };
}

/** True when this reading carries `key` with a value different from the one it displaced. */
export function changed(reading, key) {
  if (!Object.hasOwn(reading.metrics, key) || !Object.hasOwn(reading.previous, key)) return false;
  return reading.metrics[key] !== reading.previous[key].value;
}
