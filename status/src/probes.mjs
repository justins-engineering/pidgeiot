// The probe definitions and the up/degraded/down decision.
//
// Every surface listed here is a PUBLIC, unauthenticated URL that a real
// user or device already depends on, chosen so that a green result means
// something a customer would recognise as "it works" rather than "a health
// endpoint we wrote says yes". Each one is also cheap enough to hit every 5
// minutes forever without becoming load on the product:
//
//   api   /.well-known/api-catalog is dovecote's RFC 9727 catalog. It is
//         served straight out of the router from serde_json plus one env
//         var, so it touches neither Hyperdrive/Postgres nor the Durable
//         Objects. That makes it a clean EDGE-ROUTER liveness signal: green
//         proves the Worker script is deployed, routing and responding, and
//         deliberately does not claim the data plane behind it is healthy.
//         Probing an authenticated route instead would mean parking a
//         credential in this Worker, which is exactly the coupling a status
//         page in its own failure domain must not have.
//   auth  Kratos's own readiness endpoint, reached through the Cloudflare
//         Tunnel that fronts it, so the probe traverses the same path a
//         browser login takes rather than a side channel.
//   site  The dashboard origin's landing page: the prerendered SSG asset,
//         which is what a signed-out visitor actually loads first.
//
// URLs default to production because these are the surfaces the world sees;
// a deployment can repoint any one of them with a PROBE_<KEY>_URL var
// without this list being duplicated per environment.
export const SURFACES = [
  {
    key: "api",
    name: "API",
    detail: "Device ingestion and dashboard API (api.pidgeiot.com)",
    url: "https://api.pidgeiot.com/.well-known/api-catalog",
    expect: 200,
  },
  {
    key: "auth",
    name: "Authentication",
    detail: "Sign-in, registration and session checks (auth.pidgeiot.com)",
    url: "https://auth.pidgeiot.com/health/ready",
    expect: 200,
  },
  {
    key: "site",
    name: "Dashboard",
    detail: "The web dashboard and marketing site (pidgeiot.com)",
    url: "https://pidgeiot.com/",
    expect: 200,
  },
];

// A response slower than this is reported as degraded even though it
// succeeded. The three surfaces normally answer in 100-250ms, so seconds of
// latency is a real symptom a user would feel, not noise.
const DEGRADED_LATENCY_MS = 2000;

// Hard ceiling on a single attempt. Above this the attempt is abandoned and
// counted as a failure, which keeps one hung surface from eating the whole
// cron invocation.
const ATTEMPT_TIMEOUT_MS = 8000;

// Pause between the two attempts. Mirrors the retry-once-before-declaring-
// down rule dovecote's Kratos ops probe already uses: a lone dropped packet
// should not publish an outage to the world.
const RETRY_DELAY_MS = 2000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function probeUrl(surface, env) {
  const override = env[`PROBE_${surface.key.toUpperCase()}_URL`];
  return typeof override === "string" && override.trim() ? override.trim() : surface.url;
}

// One attempt. Never throws: a transport failure is just an unsuccessful
// attempt with a reason attached.
async function attempt(url, expect) {
  const started = Date.now();
  try {
    const response = await fetch(url, {
      method: "GET",
      redirect: "manual",
      signal: AbortSignal.timeout(ATTEMPT_TIMEOUT_MS),
      headers: { "user-agent": "pidgeiot-status-probe/1" },
      // The probe must observe the origin, not a cached copy of it.
      cf: { cacheTtl: 0, cacheEverything: false },
    });
    return {
      ok: response.status === expect,
      httpStatus: response.status,
      latencyMs: Date.now() - started,
      error: response.status === expect ? null : `HTTP ${response.status}`,
    };
  } catch (e) {
    return {
      ok: false,
      httpStatus: null,
      latencyMs: Date.now() - started,
      // The message can name the host but never a credential: nothing
      // secret is in the request in the first place.
      error: e && e.name === "TimeoutError" ? "timed out" : "unreachable",
    };
  }
}

// Check one surface and classify it.
//
// The ladder is deliberately three-valued rather than a boolean, because
// the two interesting middle cases both matter to someone reading this page
// during an incident: a surface that answers correctly but slowly, and a
// surface that failed once and then answered. Both are reported as degraded
// so that a real wobble is visible without crying outage.
export async function checkSurface(surface, env) {
  const url = probeUrl(surface, env);
  const first = await attempt(url, surface.expect);

  if (first.ok) {
    return {
      state: first.latencyMs > DEGRADED_LATENCY_MS ? "degraded" : "up",
      httpStatus: first.httpStatus,
      latencyMs: first.latencyMs,
      error: first.latencyMs > DEGRADED_LATENCY_MS ? "slow response" : null,
    };
  }

  await sleep(RETRY_DELAY_MS);
  const second = await attempt(url, surface.expect);

  if (second.ok) {
    return {
      state: "degraded",
      httpStatus: second.httpStatus,
      latencyMs: second.latencyMs,
      error: `intermittent (${first.error})`,
    };
  }

  return {
    state: "down",
    httpStatus: second.httpStatus,
    latencyMs: second.latencyMs,
    error: second.error,
  };
}

// Run every surface concurrently. One surface being down must not delay or
// suppress the reading for the others, so each is settled independently and
// a thrown probe degrades to an "unknown" reading rather than losing the
// whole cycle.
export async function checkAll(env) {
  const results = await Promise.all(
    SURFACES.map(async (surface) => {
      try {
        return [surface.key, await checkSurface(surface, env)];
      } catch {
        return [surface.key, { state: "unknown", httpStatus: null, latencyMs: null, error: "probe failed" }];
      }
    }),
  );
  return Object.fromEntries(results);
}
