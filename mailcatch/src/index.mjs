// Staging mail catcher: MailSlurper's role for the deployed environment.
//
// Staging sends real mail through Cloudflare Email Service, so anything a
// fixture or alert addresses would otherwise reach a live inbox. Pointing
// those recipients at a catch address routed here parks the message in KV
// instead, where an agent can read it back over HTTP.
//
// Deliberately not a forwarding rule: the point is that staging mail stops
// at the boundary rather than being copied onward.

// KV value cap. Test mail is small; a runaway attachment should cost one
// truncated record, not a 25 MiB write.
const MAX_RAW_BYTES = 256 * 1024;

// Messages are evidence for a test run, not an archive.
const TTL_SECONDS = 7 * 24 * 60 * 60;

// KV metadata is capped at 1 KiB, and these three fields are the only
// unbounded ones. Truncation costs nothing: the summary is a convenience
// and the untruncated headers are always in the raw MIME.
const MAX_ADDRESS_CHARS = 254; // RFC 5321 path limit
const MAX_SUBJECT_CHARS = 128;

const KEY_PREFIX = "msg:";
const DEFAULT_LIMIT = 50;
const MAX_LIMIT = 1000;

// Ids sort newest-first under KV's ascending lexicographic list, so the
// list route needs no sort and the key stays derivable from the id alone.
function newId(now) {
  const inverted = (9999999999999 - now).toString().padStart(13, "0");
  return inverted + "-" + crypto.randomUUID().slice(0, 8);
}

// Digesting first equalizes length, so a wrong-length token costs the same
// comparison as a wrong-value one.
async function secretMatches(presented, expected) {
  const encoder = new TextEncoder();
  const [a, b] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(presented)),
    crypto.subtle.digest("SHA-256", encoder.encode(expected)),
  ]);
  const left = new Uint8Array(a);
  const right = new Uint8Array(b);
  let diff = 0;
  for (let i = 0; i < left.length; i++) diff |= left[i] ^ right[i];
  return diff === 0;
}

async function authorized(request, env) {
  const expected = env.MAILCATCH_READ_TOKEN;
  if (!expected) return false; // unset secret denies rather than opens
  const header = request.headers.get("authorization") ?? "";
  const presented = header.startsWith("Bearer ") ? header.slice(7) : "";
  return secretMatches(presented, expected);
}

function json(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
}

// Reads at most MAX_RAW_BYTES, then drops the rest -- a stream that outruns
// the cap is truncated, never buffered whole to measure it.
async function readCapped(stream) {
  const reader = stream.getReader();
  const chunks = [];
  let total = 0;
  let truncated = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (total + value.length > MAX_RAW_BYTES) {
      chunks.push(value.subarray(0, MAX_RAW_BYTES - total));
      total = MAX_RAW_BYTES;
      truncated = true;
      await reader.cancel();
      break;
    }
    chunks.push(value);
    total += value.length;
  }
  const raw = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    raw.set(chunk, offset);
    offset += chunk.length;
  }
  return { raw, truncated };
}

export default {
  async email(message, env) {
    const now = Date.now();
    const { raw, truncated } = await readCapped(message.raw);
    const metadata = {
      from: String(message.from).slice(0, MAX_ADDRESS_CHARS),
      to: String(message.to).slice(0, MAX_ADDRESS_CHARS),
      subject: (message.headers.get("subject") ?? "").slice(0, MAX_SUBJECT_CHARS),
      receivedAt: new Date(now).toISOString(),
      size: message.rawSize ?? raw.length,
      truncated,
    };
    await env.MAILCATCH_KV.put(KEY_PREFIX + newId(now), raw, {
      metadata,
      expirationTtl: TTL_SECONDS,
    });
  },

  async fetch(request, env) {
    // Authenticate before routing so an unauthenticated caller learns
    // nothing about which paths exist.
    if (!(await authorized(request, env))) {
      return json({ error: "unauthorized" }, 401);
    }

    const url = new URL(request.url);
    const path = url.pathname.replace(/\/+$/, "") || "/";

    if (path === "/messages" && request.method === "GET") {
      const requested = Number.parseInt(url.searchParams.get("limit") ?? "", 10);
      const limit = Number.isNaN(requested)
        ? DEFAULT_LIMIT
        : Math.min(Math.max(requested, 1), MAX_LIMIT);
      const listed = await env.MAILCATCH_KV.list({ prefix: KEY_PREFIX, limit });
      const messages = listed.keys.map((key) => ({
        id: key.name.slice(KEY_PREFIX.length),
        ...key.metadata,
      }));
      return json({ messages, truncatedList: !listed.list_complete });
    }

    const single = path.startsWith("/messages/") ? path.slice("/messages/".length) : null;
    if (single && !single.includes("/")) {
      const key = KEY_PREFIX + single;
      if (request.method === "GET") {
        const raw = await env.MAILCATCH_KV.get(key, "text");
        if (raw === null) return json({ error: "not found" }, 404);
        return new Response(raw, {
          headers: { "content-type": "message/rfc822; charset=utf-8" },
        });
      }
      if (request.method === "DELETE") {
        await env.MAILCATCH_KV.delete(key);
        return new Response(null, { status: 204 });
      }
    }

    return json({ error: "not found" }, 404);
  },
};
