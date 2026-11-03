// Runs run-fixtures.mjs and run-hostile.mjs against the harness under workerd, the runtime rules
// run in, which refuses code Bun passes (an unbound global called through the prelude failed
// every dispatch). `bun run-workerd.mjs`: each manifest is served locally from a scratch copy
// under $TMPDIR, removed after; nothing is deployed. workerd is wrangler's own copy, started
// directly because `wrangler dev` cannot forward requests when run by Bun.
import { spawn, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync,
  writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

const here = import.meta.dirname;
let workerd;
try {
  workerd = (await import("workerd")).default;
} catch {
  console.log("FAIL workerd not installed: run bun install in dovecote");
  process.exit(1);
}
// The date rules are uploaded with, so the harness meets the same runtime behaviour here.
const date = readFileSync(join(here, "..", "src", "helpers", "rules.rs"), "utf8")
  .match(/RULES_COMPATIBILITY_DATE: &str = "([0-9-]+)"/)[1];

const freePort = () => new Promise((resolve) => {
  const server = createServer().listen(0, "127.0.0.1", () => {
    const { port } = server.address();
    server.close(() => resolve(port));
  });
});

// main.js first: workerd takes the first module as the entrypoint.
function config(dir, port) {
  const names = readdirSync(dir, { recursive: true }).filter((f) => f.endsWith(".js"))
    .sort((a, b) => (b === "main.js") - (a === "main.js"));
  const modules = names.map((f) => '(name = "' + f + '", esModule = embed "' + f + '")');
  return [
    'using Workerd = import "/workerd/workerd.capnp";',
    "const config :Workerd.Config = (",
    '  services = [(name = "rules", worker = .rules)],',
    '  sockets = [(name = "http", address = "127.0.0.1:' + port + '", http = (), ' +
      'service = "rules")],',
    ");",
    "const rules :Workerd.Worker = (",
    "  modules = [" + modules.join(", ") + "],",
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
  { runner: "run-fixtures.mjs", files: readdirSync(here).filter((f) => f.endsWith(".js")) },
  { runner: "run-hostile.mjs", files: ["main.js", "prelude.js"], extra: "hostile" },
];

const root = mkdtempSync(join(tmpdir(), "rules-workerd-"));
let failures = 0;
try {
  for (const suite of suites) {
    const dir = join(root, suite.runner.replace(".mjs", ""));
    mkdirSync(dir);
    for (const f of suite.files) cpSync(join(here, f), join(dir, f));
    if (suite.extra) cpSync(join(here, suite.extra), dir, { recursive: true });
    const port = await freePort();
    writeFileSync(join(dir, "gate.capnp"), config(dir, port));

    const url = "http://127.0.0.1:" + port + "/";
    const out = [];
    const child = spawn(workerd, ["serve", "gate.capnp"], { cwd: dir });
    child.stdout.on("data", (d) => out.push(d));
    child.stderr.on("data", (d) => out.push(d));
    try {
      if (!(await ready(url, child))) {
        console.log("FAIL workerd did not serve the copy for " + suite.runner);
        console.log(Buffer.concat(out).toString().slice(-2000));
        failures++;
        continue;
      }
      console.log("== " + suite.runner + " under workerd " + date);
      const run = spawnSync("bun", [suite.runner], { cwd: here, stdio: "inherit",
        env: { ...process.env, RULES_URL: url } });
      if (run.status !== 0) failures++;
    } finally {
      child.kill();
    }
  }
} finally {
  rmSync(root, { recursive: true, force: true });
}
console.log(failures === 0 ? "workerd gate passes" : failures + " workerd suite(s) failed");
process.exitCode = failures === 0 ? 0 : 1;
