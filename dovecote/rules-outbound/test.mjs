// `bun test.mjs`: the outbound Worker's handlers called with fakes for what the platform passes.
// Real egress fails the run, so a handler that forwarded anything could not pass here.
let failures = 0;
const check = (name, ok, why = "") => {
  console.log((ok ? "pass " : "FAIL ") + name + (why ? ": " + why : ""));
  if (!ok) failures++;
};

const logged = [];
console.log = (...args) => logged.push(args);
const report = (...args) => process.stdout.write(args.join(" ") + "\n");
globalThis.fetch = () => {
  throw new Error("the outbound Worker made a request");
};

const { default: outbound } = await import("./index.js");
const say = (name, ok, why) => {
  const before = console.log;
  console.log = report;
  check(name, ok, why);
  console.log = before;
};

say("exports only fetch and connect", Object.keys(outbound).sort().join() === "connect,fetch",
  Object.keys(outbound).join());

const requests = [
  ["GET https://example.com/", new Request("https://example.com/"), "example.com"],
  ["POST to an IP and port", new Request("http://8.8.8.8:53/q", { method: "POST", body: "x" }),
    "8.8.8.8:53"],
  ["our own API", new Request("https://api-staging.pidgeiot.com/flocks"),
    "api-staging.pidgeiot.com"],
];
for (const [name, request, host] of requests) {
  logged.length = 0;
  let res;
  try {
    res = await outbound.fetch(request, { flock_id: "f-1" });
  } catch (e) {
    say(name + " answers", false, String(e));
    continue;
  }
  say(name + " is refused 403", res.status === 403, String(res.status));
  say(name + " says why", (await res.text()) === "egress is not available to rules");
  const line = JSON.stringify(logged);
  say(name + " logs flock and host",
    line === JSON.stringify([["rule egress refused", "f-1", host]]), line);
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
];
for (const [name, opened, address] of sockets) {
  logged.length = 0;
  const s = socket(opened());
  try {
    await outbound.connect(s, { flock_id: "f-2" });
  } catch (e) {
    say(name + " is refused without an exception", false, String(e));
    continue;
  }
  say(name + " is closed once", s.closed === 1, String(s.closed));
  say(name + " is never read, written or upgraded", s.used.length === 0, s.used.join());
  const line = JSON.stringify(logged);
  say(name + " logs flock and address",
    line === JSON.stringify([["rule socket refused", "f-2", address]]), line);
}

console.log = report;
console.log(failures === 0 ? "all outbound checks pass" : failures + " outbound check(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
