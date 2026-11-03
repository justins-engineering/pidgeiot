// The prelude's functions are called with the frozen holder as their receiver, which workerd
// refuses for a function not bound to its own receiver ("Illegal invocation") and Bun allows.
// `bun run-prelude.mjs` asserts that every function entry is bound, except the few that need no
// receiver, then calls each through the holder. A strict copy of every global function is put
// in place first, so a binding to the wrong receiver fails the call here too.
for (const name of Object.getOwnPropertyNames(globalThis)) {
  const real = Object.getOwnPropertyDescriptor(globalThis, name).value;
  if (!/^[a-z]/.test(name) || typeof real !== "function") continue;
  globalThis[name] = function (...args) {
    if (this !== undefined && this !== globalThis) {
      throw new TypeError("Illegal invocation: " + name + " called with another receiver");
    }
    return real.apply(globalThis, args);
  };
}

const P = (await import("./prelude.js")).default;

// A call for each function entry. Constructors (capitalized) are skipped: `new` sets their own
// receiver.
const calls = {
  setTimeout: [() => {}, 0],
  clearTimeout: [undefined],
  race: [[1]],
  structuredClone: [{ a: 1 }],
  stringify: [{ a: 1 }],
  parse: ["{}"],
};
// Entries that ignore their receiver, so they may be exposed unbound.
const receiverFree = new Set(["stringify", "parse"]);

let failures = 0;
const check = (name, ok, why = "") => {
  console.log((ok ? "pass " : "FAIL ") + name + (why ? ": " + why : ""));
  if (!ok) failures++;
};
for (const [name, value] of Object.entries(P)) {
  if (typeof value !== "function" || /^[A-Z]/.test(name)) continue;
  if (!Object.hasOwn(calls, name)) {
    check("P." + name + " has a call here", false, "add one to calls");
    continue;
  }
  check("P." + name + " is bound", receiverFree.has(name) || /^bound /.test(value.name));
  try {
    await P[name](...calls[name]);
    check("P." + name + " runs with the holder as receiver", true);
  } catch (e) {
    check("P." + name + " runs with the holder as receiver", false, String(e));
  }
}
console.log(failures === 0 ? "all prelude checks pass" : failures + " prelude check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
