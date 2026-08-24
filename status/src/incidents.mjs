// Manually published incidents, read from KV.
//
// An incident is not one record that gets edited. It is an APPEND-ONLY
// series of updates, one KV key each:
//
//   incident:<slug>:<iso8601-utc>
//
// That shape is chosen for the human posting at three in the morning. A
// single mutable record would force a read-modify-write for every update --
// fetch the JSON, splice a new entry into an array, put it back -- which is
// several careful steps under pressure and one slip away from overwriting
// the incident's own history. Appending a new key is a single command that
// cannot clobber anything, and the page reassembles the timeline on read.
//
// Keys sort lexicographically in KV list results, so the `<slug>:<iso>`
// layout groups every update of an incident together and orders each
// group's updates chronologically for free. Slugs are dated
// (`2026-08-24-api-latency`) so incidents themselves sort chronologically
// too.
//
// Update value (JSON):
//   {
//     "title":    "Elevated API latency",
//     "severity": "minor" | "major" | "critical",
//     "status":   "investigating" | "identified" | "monitoring" | "resolved",
//     "surfaces": ["api"],
//     "body":     "What we know, in plain language."
//   }
//
// Title, severity and surfaces are carried forward from earlier updates
// when a later one omits them, so a follow-up only has to state what
// changed.

export const INCIDENT_PREFIX = "incident:";

const STATUSES = ["investigating", "identified", "monitoring", "resolved"];
const SEVERITIES = ["minor", "major", "critical"];

// Split on the FIRST colon after the prefix, not the last: an ISO timestamp
// contains its own colons, so scanning from the right lands inside HH:MM:SS
// and shears the slug and the timestamp across the wrong boundary. This is
// why slugs may not contain a colon, which the README states.
function parseKey(name) {
  const rest = name.slice(INCIDENT_PREFIX.length);
  const split = rest.indexOf(":");
  if (split <= 0) return null;
  const at = rest.slice(split + 1);
  // A key that does not carry a parseable timestamp would sort and render
  // unpredictably, so it is dropped rather than guessed at.
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(at)) return null;
  return { slug: rest.slice(0, split), at };
}

// Everything an operator typed is treated as untrusted input here: a
// malformed value should degrade one update, never throw the page.
function normalizeUpdate(value, at) {
  if (!value || typeof value !== "object") return null;
  const status = STATUSES.includes(value.status) ? value.status : "investigating";
  // Left null rather than defaulted here so the incident can tell "this
  // update did not mention severity" apart from "this update said minor".
  // Defaulting at this level would let a terse follow-up ("status":
  // "monitoring") silently downgrade a critical incident, which the overall
  // banner reads from.
  const severity = SEVERITIES.includes(value.severity) ? value.severity : null;
  return {
    at,
    status,
    severity,
    title: typeof value.title === "string" ? value.title : null,
    body: typeof value.body === "string" ? value.body : "",
    surfaces: Array.isArray(value.surfaces) ? value.surfaces.filter((s) => typeof s === "string") : [],
  };
}

// Read every incident update and fold it into one entry per incident.
//
// Returns newest-first. Never throws: if KV is unavailable the page still
// renders its automated signal, which is the half that matters most during
// the kind of event that would take KV out.
export async function readIncidents(env) {
  let keys = [];
  try {
    const listed = await env.STATUS_KV.list({ prefix: INCIDENT_PREFIX, limit: 1000 });
    keys = listed.keys ?? [];
  } catch {
    return [];
  }

  const grouped = new Map();
  await Promise.all(
    keys.map(async (entry) => {
      const parsed = parseKey(entry.name);
      if (!parsed) return;
      let value = null;
      try {
        value = await env.STATUS_KV.get(entry.name, { type: "json", cacheTtl: 30 });
      } catch {
        return;
      }
      const update = normalizeUpdate(value, parsed.at);
      if (!update) return;
      if (!grouped.has(parsed.slug)) grouped.set(parsed.slug, []);
      grouped.get(parsed.slug).push(update);
    }),
  );

  const incidents = [];
  for (const [slug, updates] of grouped) {
    updates.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
    const latest = updates[updates.length - 1];
    // Carried-forward fields: the most recent update that actually stated
    // one wins, so a terse "resolved" follow-up keeps the incident's title.
    const titled = [...updates].reverse().find((u) => u.title);
    const surfaced = [...updates].reverse().find((u) => u.surfaces.length);
    const severe = [...updates].reverse().find((u) => u.severity);
    incidents.push({
      slug,
      title: titled?.title ?? slug,
      severity: severe?.severity ?? "minor",
      status: latest.status,
      resolved: latest.status === "resolved",
      surfaces: surfaced?.surfaces ?? [],
      started_at: updates[0].at,
      updated_at: latest.at,
      updates,
    });
  }

  incidents.sort((a, b) => (a.started_at < b.started_at ? 1 : a.started_at > b.started_at ? -1 : 0));
  return incidents;
}
