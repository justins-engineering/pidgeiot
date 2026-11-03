// `bun test.mjs`: the outbound Worker's handlers called with fakes for what the platform passes.
// Real egress fails the run, so a handler that forwarded anything could not pass here.
import { readFileSync } from "node:fs";
import { join } from "node:path";

const report = (...args) => process.stdout.write(args.join(" ") + "\n");
let failures = 0;
const check = (name, ok, why = "") => {
  report((ok ? "pass " : "FAIL ") + name + (why ? ": " + String(why).slice(0, 200) : ""));
  if (!ok) failures++;
};

const logged = [];
console.log = (...args) => logged.push(args);
globalThis.fetch = () => {
  throw new Error("the outbound Worker made a request");
};
let now = 1_000_000;
Date.now = () => now;

const { default: outbound } = await import("./index.js");

const here = import.meta.dirname;
const rulesDate = readFileSync(join(here, "..", "src", "helpers", "rules.rs"), "utf8")
  .match(/RULES_COMPATIBILITY_DATE: &str = "([0-9-]+)"/)?.[1];
const ownDate = readFileSync(join(here, "wrangler.toml"), "utf8")
  .match(/^compatibility_date = "([0-9-]+)"/m)?.[1];
check("runs on RULES_COMPATIBILITY_DATE", !!rulesDate && ownDate === rulesDate,
  ownDate + " vs " + rulesDate);

check("exports only fetch and connect", Object.keys(outbound).sort().join() === "connect,fetch",
  Object.keys(outbound).join());

// Each case its own flock, so none falls in another's quiet minute.
const requests = [
  ["GET https://example.com/", new Request("https://example.com/"), "example.com"],
  ["POST to an IP and port", new Request("http://8.8.8.8:53/q", { method: "POST", body: "x" }),
    "8.8.8.8:53"],
  ["our own API", new Request("https://api-staging.pidgeiot.com/flocks"),
    "api-staging.pidgeiot.com"],
];
for (const [name, request, host] of requests) {
  logged.length = 0;
  const flock = "f-" + host;
  let threw = null;
  try {
    await outbound.fetch(request, { flock_id: flock });
  } catch (e) {
    threw = e;
  }
  check(name + " throws, so the rule's fetch rejects", threw instanceof Error, String(threw));
  check(name + " says why and names the host",
    threw?.message === "egress is not available to rules: " + host, String(threw));
  const line = JSON.stringify(logged);
  check(name + " logs flock and host",
    line === JSON.stringify([["rule egress refused", flock, host, 0]]), line);
}

function socket(opened) {
  const s = { closed: 0, used: [], opened };
  s.close = () => {
    s.closed++;
    return Promise.resolve();
  };
  for (const side of ["readable", "writable", "startTls"]) {
    Object.defineProperty(s, side, { get: () => s.used.push(side) });
  }
  return s;
}

const sockets = [
  ["a socket", () => Promise.resolve({ localAddress: "8.8.8.8:53" }), "8.8.8.8:53"],
  ["a socket whose opened rejects", () => Promise.reject(new Error("gone")), null],
  ["a socket that names no address", () => Promise.resolve({}), null],
  ["a socket dialing a long name", () => Promise.resolve({ localAddress: "a".repeat(5000) }),
    "a".repeat(300)],
];
for (const [i, [name, opened, address]] of sockets.entries()) {
  logged.length = 0;
  const s = socket(opened());
  const flock = "s-" + i;
  try {
    await outbound.connect(s, { flock_id: flock });
  } catch (e) {
    check(name + " is refused without an exception", false, String(e));
    continue;
  }
  check(name + " is closed once", s.closed === 1, String(s.closed));
  check(name + " is never read, written or upgraded", s.used.length === 0, s.used.join());
  const line = JSON.stringify(logged);
  check(name + " logs flock and address",
    line === JSON.stringify([["rule socket refused", flock, address, 0]]), line);
}

{
  let settle;
  const s = socket(undefined);
  let closedWhenRead = null;
  const pending = new Promise((r) => (settle = r));
  Object.defineProperty(s, "opened", { get: () => ((closedWhenRead ??= s.closed), pending) });
  const handled = outbound.connect(s, { flock_id: "s-slow" });
  await Promise.resolve();
  check("a socket whose opened never settles is closed at once", s.closed === 1,
    String(s.closed));
  settle({ localAddress: "8.8.8.8:53" });
  await handled;
  check("opened is read only after the close", closedWhenRead === 1, String(closedWhenRead));
}

{
  logged.length = 0;
  const env = { flock_id: "f-loud" };
  const refuse = () => outbound.fetch(new Request("https://example.com/"), env).catch(() => {});
  for (let i = 0; i < 5; i++) await refuse();
  const s = socket(Promise.resolve({ localAddress: "8.8.8.8:53" }));
  await outbound.connect(s, env);
  await outbound.connect(socket(Promise.resolve({})), env);
  await outbound.fetch(new Request("https://example.com/"), { flock_id: "f-quiet" })
    .catch(() => {});
  now += 60_000;
  await refuse();
  check("a flock's refusals log once a minute per kind, counting the rest",
    JSON.stringify(logged) === JSON.stringify([
      ["rule egress refused", "f-loud", "example.com", 0],
      ["rule socket refused", "f-loud", "8.8.8.8:53", 0],
      ["rule egress refused", "f-quiet", "example.com", 0],
      ["rule egress refused", "f-loud", "example.com", 4],
    ]), JSON.stringify(logged));
  check("every quiet refusal still closes its socket", s.closed === 1, String(s.closed));
}

report(failures === 0 ? "all outbound checks pass" : failures + " outbound check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
