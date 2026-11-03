// Replaces the timer globals at module load, then never settles.
globalThis.setTimeout = () => 0;
globalThis.clearTimeout = () => {};
JSON.stringify = () => { throw new Error("stringify hooked"); };
export default () => new Promise(() => {});
