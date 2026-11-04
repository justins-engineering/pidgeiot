// Sends the Cloudflare API requests dovecote's rule_namespace module built (requests.json, from
// the `namespace_it_requests` host test) to the staging rules namespace, and records each answer
// in answers.json for the `namespace_it_answers` host test to classify. Only URLs under the
// staging namespace are sent. The bearer is the logged-in wrangler session's, held in memory and
// never printed or written; nothing reads CF_RULES_API_TOKEN.
//
//   bun namespace-it.mjs <dir>
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const STAGING = "https://api.cloudflare.com/client/v4/accounts/7fc00cca2ed8361b4a472cbe3c24fb5f"
  + "/workers/dispatch/namespaces/dovecote-rules-staging/scripts";

const dir = process.argv[2];
if (!dir) {
  console.log("usage: bun namespace-it.mjs <dir>");
  process.exit(2);
}
const requests = JSON.parse(readFileSync(join(dir, "requests.json"), "utf8"));
for (const r of requests) {
  if (r.url !== STAGING && !r.url.startsWith(STAGING + "/") && !r.url.startsWith(STAGING + "?")) {
    console.log(`refusing ${r.label}: not the staging namespace`);
    process.exit(1);
  }
}

const out = spawnSync("bunx", ["wrangler", "auth", "token"], {
  cwd: join(import.meta.dir, ".."),
  encoding: "utf8",
});
const token = out.stdout
  .split("\n")
  .map((l) => l.trim())
  .filter((l) => /^[A-Za-z0-9._-]{20,}$/.test(l))
  .pop();
if (!token) {
  console.log("no wrangler session token; stopping");
  process.exit(1);
}

const answers = [];
for (const r of requests) {
  const headers = { authorization: "Bearer " + token };
  if (r.content_type) headers["content-type"] = r.content_type;
  const res = await fetch(r.url, { method: r.method, headers, body: r.body ?? undefined });
  const body = await res.text();
  answers.push({ label: r.label, status: res.status, retry_after: res.headers.get("retry-after"),
    body });
  let codes = [];
  try {
    codes = (JSON.parse(body).errors ?? []).map((e) => e.code);
  } catch {}
  console.log(`${r.label}: ${r.method} -> HTTP ${res.status} codes=${JSON.stringify(codes)}`);
}
writeFileSync(join(dir, "answers.json"), JSON.stringify(answers, null, 1));
