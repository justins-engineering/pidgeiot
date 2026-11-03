// The harness's own copies of the globals it relies on, taken before any rule module is
// evaluated (main.js imports this first), so a rule that replaces one at its top level does
// not reach the harness. A narrowing, not a boundary: dovecote counts what gets past it.
export default Object.freeze({
  setTimeout: globalThis.setTimeout,
  clearTimeout: globalThis.clearTimeout,
  Promise: globalThis.Promise,
  race: Promise.race.bind(Promise),
  structuredClone: globalThis.structuredClone,
  stringify: JSON.stringify,
  parse: JSON.parse,
  Response: globalThis.Response,
  TextEncoder: globalThis.TextEncoder,
  TextDecoder: globalThis.TextDecoder,
});
