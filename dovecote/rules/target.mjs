// The harness a runner calls: main.js in `dir`, in this process, or with RULES_URL set the
// same files served by workerd (run-workerd.mjs), which enforces what Bun lets pass.
import { join } from "node:path";

export async function harnessIn(dir) {
  const url = process.env.RULES_URL;
  if (!url) return (await import(join(dir, "main.js"))).default;
  return { fetch: async (req) => fetch(url, { method: "POST", body: await req.text() }) };
}
