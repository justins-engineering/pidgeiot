// Runs over each telemetry batch from each pigeon in this flock.
// Return alerts, derived values and a state patch; return {} to do nothing.
import { num } from "./pidgeiot.js";

const LIMIT = 40; // degrees C

export default function (event) {
  let hottest = null;
  for (const reading of event.readings) {
    const t = num(reading.metrics.temp_c);
    if (t !== null && (hottest === null || t > hottest)) hottest = t;
  }
  if (hottest === null) return {};
  const hot = hottest > LIMIT;
  // Restated every batch: the platform mails only when the firing flag changes.
  return {
    alerts: [{
      key: "too-hot",
      kind: hot ? "fire" : "clear",
      severity: hot ? "warning" : "info",
      scope: "pigeon",
      subject: hot ? "Running hot" : "Back to normal",
      body: "Highest reading in this batch: " + hottest + " C.",
    }],
  };
}
