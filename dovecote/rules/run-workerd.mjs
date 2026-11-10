// Runs run-fixtures.mjs, run-builder.mjs and run-hostile.mjs against the harness under workerd, the runtime rules
// run in, which refuses code Bun passes. `bun run-workerd.mjs`: each manifest is served locally
// from a scratch copy under $TMPDIR, removed after; nothing is deployed. `--canned` runs
// make-canned.mjs against the same copies instead, so canned/ holds workerd's answers.
// workerd is wrangler's own copy, started directly because `wrangler dev` cannot forward requests
// when run by Bun. The port is probed then freed, so a race can fail a run (never pass one), and
// a gate killed hard leaves its workerd on loopback until killed by hand.
import { spawn, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync,
  writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

const here = import.meta.dirname;
const canned = process.argv.includes("--canned");
// Through wrangler, which is what pins it: workerd is not a dependency of dovecote itself.
let workerd;
try {
  const viaWrangler = createRequire(createRequire(import.meta.url)
    .resolve("wrangler/package.json"));
  workerd = viaWrangler("workerd").default;
} catch {
  console.log("FAIL workerd not installed: run bun install in dovecote");
  process.exit(1);
}
// The date rules are uploaded with, so the harness meets the same runtime behaviour here.
const declared = readFileSync(join(here, "..", "src", "helpers", "rules.rs"), "utf8")
  .match(/RULES_COMPATIBILITY_DATE: &str = "([0-9-]+)"/);
if (!declared) {
  console.log("FAIL RULES_COMPATIBILITY_DATE not found in rules.rs");
  process.exit(1);
}
const date = declared[1];

const freePort = () => new Promise((resolve) => {
  const server = createServer().listen(0, "127.0.0.1", () => {
    const { port } = server.address();
    server.close(() => resolve(port));
  });
});

// The rules namespace's outbound Worker, run as the rules' global outbound with a fixed flock, so
// the gate meets the refusals the deployed rules meet; workerd's default would give hostile code
// the open internet. Copied as .mjs to stay out of the rules' own modules.
const outbound = join(here, "..", "rules-outbound", "index.js");
const FLOCK = "f-gate";

// main.js first: workerd takes the first module as the entrypoint.
function config(dir, port) {
  const names = readdirSync(dir, { recursive: true }).filter((f) => f.endsWith(".js"))
    .sort((a, b) => (b === "main.js") - (a === "main.js"));
  const modules = names.map((f) => '(name = "' + f + '", esModule = embed "' + f + '")');
  return [
    'using Workerd = import "/workerd/workerd.capnp";',
    "const config :Workerd.Config = (",
    '  services = [(name = "rules", worker = .rules), (name = "outbound", worker = .outbound)],',
    '  sockets = [(name = "http", address = "127.0.0.1:' + port + '", http = (), ' +
      'service = "rules")],',
    ");",
    "const rules :Workerd.Worker = (",
    "  modules = [" + modules.join(", ") + "],",
    '  compatibilityDate = "' + date + '",',
    '  globalOutbound = "outbound",',
    ");",
    "const outbound :Workerd.Worker = (",
    '  modules = [(name = "outbound.mjs", esModule = embed "outbound.mjs")],',
    '  bindings = [(name = "flock_id", text = "' + FLOCK + '")],',
    '  compatibilityDate = "' + date + '",',
    ");",
    "",
  ].join("\n");
}

async function ready(url, child) {
  for (let i = 0; i < 100 && child.exitCode === null; i++) {
    try {
      const res = await fetch(url, { method: "POST", body: "not json" });
      if ((await res.json()).error === "bad_input") return true;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  return false;
}

// Each copy's rules.js is its manifest: the worked rules, or hostile/ in their place.
const suites = [
  { name: "worked", runner: "run-fixtures.mjs",
    files: readdirSync(here).filter((f) => f.endsWith(".js")) },
  { name: "builder", runner: "run-builder.mjs",
    files: readdirSync(here).filter((f) => f.endsWith(".js")) },
  { name: "hostile", runner: "run-hostile.mjs", files: ["main.js", "prelude.js"],
    extra: "hostile", logged: ["rule egress refused " + FLOCK + " example.com 0",
      "rule socket refused " + FLOCK + " 8.8.8.8:53 0"] },
];

// The outbound Worker's lines reach the pipe only once the event loop runs again.
async function logs(out, lines) {
  for (let i = 0; i < 50; i++) {
    const text = Buffer.concat(out).toString();
    if (lines.every((l) => text.includes(l))) return true;
    await new Promise((r) => setTimeout(r, 100));
  }
  return false;
}

const root = mkdtempSync(join(tmpdir(), "rules-workerd-"));
let failures = 0;
try {
  for (const suite of suites) {
    // Its answers need no canned copy: no Rust test reads them.
    if (canned && suite.name === "builder") continue;
    const dir = join(root, suite.name);
    mkdirSync(dir);
    for (const f of suite.files) cpSync(join(here, f), join(dir, f));
    if (suite.extra) cpSync(join(here, suite.extra), dir, { recursive: true });
    cpSync(outbound, join(dir, "outbound.mjs"));
    const port = await freePort();
    writeFileSync(join(dir, "gate.capnp"), config(dir, port));

    const url = "http://127.0.0.1:" + port + "/";
    const out = [];
    const child = spawn(workerd, ["serve", "gate.capnp"], { cwd: dir });
    child.stdout.on("data", (d) => out.push(d));
    child.stderr.on("data", (d) => out.push(d));
    try {
      if (!(await ready(url, child))) {
        console.log("FAIL workerd did not serve the " + suite.name + " copy");
        console.log(Buffer.concat(out).toString().slice(-2000));
        failures++;
        continue;
      }
      const args = canned ? ["make-canned.mjs", suite.name] : [suite.runner];
      console.log("== " + args.join(" ") + " under workerd " + date);
      const run = spawnSync(process.execPath, args, { cwd: here, stdio: "inherit",
        env: { ...process.env, RULES_URL: url } });
      if (run.status !== 0) failures++;
      if (!canned && suite.logged) {
        const ok = await logs(out, suite.logged);
        console.log((ok ? "pass " : "FAIL ") + "the outbound Worker logged both refusals with " +
          "the flock");
        if (!ok) failures++;
      }
    } finally {
      child.kill();
    }
  }
} finally {
  rmSync(root, { recursive: true, force: true });
}
console.log(failures === 0 ? "workerd gate passes" : failures + " workerd suite(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
