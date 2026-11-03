// The prelude's functions are called with the frozen holder as their receiver, which workerd
// refuses for a global function not bound to globalThis ("Illegal invocation") and Bun allows.
// `bun run-prelude.mjs` puts a strict copy of every global function in place first, so the
// prelude captures the strict ones, then calls each function it exposes through the holder.
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

// A call for each function entry; a constructor (capitalized) is called with `new`, which sets
// its own receiver.
const calls = {
  setTimeout: [() => {}, 0],
  clearTimeout: [undefined],
  race: [[1]],
  structuredClone: [{ a: 1 }],
  stringify: [{ a: 1 }],
  parse: ["{}"],
};

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
  try {
    P[name](...calls[name]);
    check("P." + name + " runs with the holder as receiver", true);
  } catch (e) {
    check("P." + name + " runs with the holder as receiver", false, String(e));
  }
}
console.log(failures === 0 ? "all prelude checks pass" : failures + " prelude check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
