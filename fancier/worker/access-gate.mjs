// Cloudflare Access gate for the fancier staging deployment.
//
// PAUSED (2026-07-16): staging setup is on hold pending a naming decision
// (reusing "fancier" as the [env.staging] worker name would silently
// overwrite the live production Worker serving pidgeiot.com — see the team
// thread). This file is intentionally NOT referenced by wrangler.toml right
// now, so it has no effect on any build or deploy; it's parked here, already
// verified end-to-end (local mock JWKS + wrangler dev), ready to wire in via
// `[env.staging] main = "worker/access-gate.mjs"` once that's resolved.
//
// Intended wiring: wrangler.toml's [env.staging] would point `main` at this
// file instead of serving [assets] directly, so every staging request passes
// through here first. It validates the `Cf-Access-Jwt-Assertion` header that
// Cloudflare Access attaches to authenticated requests, then either hands off
// to the ASSETS binding (the Dioxus release build) or 403s. Production stays
// assets-only — this file would never be wired into the top-level (default)
// environment.
//
// The signing keys are never hardcoded: Cloudflare rotates them, so they're
// fetched from env.CF_ACCESS_CERTS_URL and cached in memory per-isolate for
// a short TTL, keyed by `kid` so a rotation is picked up within one request
// (cache miss on the new kid forces a re-fetch).

const JWKS_CACHE_TTL_MS = 5 * 60 * 1000;
const CLOCK_SKEW_LEEWAY_SECONDS = 60;

// Module-scoped: reused across requests handled by the same isolate, reset
// whenever the isolate is recycled.
let jwksCache = null; // { keys: Map<kid, JsonWebKey>, fetchedAt: number, certsUrl: string }

function base64UrlToUint8Array(b64url) {
  const b64 = b64url.replace(/-/g, "+").replace(/_/g, "/");
  const padded = b64 + "===".slice((b64.length + 3) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function base64UrlToJson(b64url) {
  return JSON.parse(new TextDecoder().decode(base64UrlToUint8Array(b64url)));
}

async function fetchJwks(certsUrl) {
  const response = await fetch(certsUrl);
  if (!response.ok) {
    throw new Error(`JWKS fetch failed: ${response.status}`);
  }
  const body = await response.json();
  const keys = new Map();
  for (const key of body.keys ?? []) {
    if (key.kid) keys.set(key.kid, key);
  }
  return keys;
}

async function getJwksKey(certsUrl, kid) {
  const now = Date.now();
  const isFresh =
    jwksCache && jwksCache.certsUrl === certsUrl && now - jwksCache.fetchedAt < JWKS_CACHE_TTL_MS;

  if (isFresh && jwksCache.keys.has(kid)) {
    return jwksCache.keys.get(kid);
  }

  // Cache miss (expired, different certs URL, or unknown kid) — refetch so
  // rotation is picked up without waiting out the full TTL.
  const keys = await fetchJwks(certsUrl);
  jwksCache = { keys, fetchedAt: now, certsUrl };
  return keys.get(kid) ?? null;
}

async function verifyAccessJwt(token, env) {
  const parts = token.split(".");
  if (parts.length !== 3) return false;
  const [headerB64, payloadB64, signatureB64] = parts;

  let header, payload;
  try {
    header = base64UrlToJson(headerB64);
    payload = base64UrlToJson(payloadB64);
  } catch {
    return false;
  }

  if (header.alg !== "RS256" || !header.kid) return false;

  const jwk = await getJwksKey(env.CF_ACCESS_CERTS_URL, header.kid);
  if (!jwk) return false;

  let cryptoKey;
  try {
    cryptoKey = await crypto.subtle.importKey(
      "jwk",
      jwk,
      { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
      false,
      ["verify"],
    );
  } catch {
    return false;
  }

  const signedData = new TextEncoder().encode(`${headerB64}.${payloadB64}`);
  const signature = base64UrlToUint8Array(signatureB64);

  const signatureValid = await crypto.subtle.verify(
    "RSASSA-PKCS1-v1_5",
    cryptoKey,
    signature,
    signedData,
  );
  if (!signatureValid) return false;

  const now = Math.floor(Date.now() / 1000);
  if (typeof payload.exp !== "number" || payload.exp + CLOCK_SKEW_LEEWAY_SECONDS < now) {
    return false;
  }

  const aud = Array.isArray(payload.aud) ? payload.aud : [payload.aud];
  if (!aud.includes(env.CF_ACCESS_AUD)) return false;

  return true;
}

export default {
  async fetch(request, env) {
    const assertion = request.headers.get("Cf-Access-Jwt-Assertion");
    if (!assertion) {
      return new Response("Forbidden", { status: 403 });
    }

    let authorized = false;
    try {
      authorized = await verifyAccessJwt(assertion, env);
    } catch {
      // Any unexpected failure (malformed token, network error fetching
      // JWKS, etc.) is a rejection, not a 500 — never fail open.
      authorized = false;
    }

    if (!authorized) {
      return new Response("Forbidden", { status: 403 });
    }

    return env.ASSETS.fetch(request);
  },
};
