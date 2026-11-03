// The harness a runner calls: main.js in `dir`, in this process, or with RULES_URL set the
// copy run-workerd.mjs serves under workerd (`dir` is then ignored), which enforces what Bun
// lets pass. The target is printed so a result says where it ran.
import { join } from "node:path";

export async function harnessIn(dir) {
  const url = process.env.RULES_URL;
  if (!url) return (await import(join(dir, "main.js"))).default;
  console.log("target: " + url);
  return { fetch: async (req) => fetch(url, { method: "POST", body: await req.text() }) };
}
