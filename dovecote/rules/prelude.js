// The harness's own copies of the globals it relies on, taken before any rule module is
// evaluated (main.js imports this first), so a rule that replaces one at its top level does
// not reach the harness. A narrowing, not a boundary: dovecote counts what gets past it.
// Each global function is bound to globalThis, since the harness calls it with this holder as
// its receiver and workerd refuses that ("Illegal invocation"); run-prelude.mjs checks it.
const bound = (name) => globalThis[name].bind(globalThis);

export default Object.freeze({
  setTimeout: bound("setTimeout"),
  clearTimeout: bound("clearTimeout"),
  Promise: globalThis.Promise,
  race: Promise.race.bind(Promise),
  structuredClone: bound("structuredClone"),
  stringify: JSON.stringify,
  parse: JSON.parse,
  Response: globalThis.Response,
  TextEncoder: globalThis.TextEncoder,
  TextDecoder: globalThis.TextDecoder,
});
