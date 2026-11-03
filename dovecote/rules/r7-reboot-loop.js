// More than three boots in 24 h. A boot is a reset_cause arrival, or uptime_s going
// backwards for a boot whose reset_cause report was lost. A boot is dated by its uptime where
// the reading has one, so the same boot seen twice (a redelivery, a device resend, its keys in
// separate reports) counts once, while boots a minute apart still count apart.
import { num, within } from "./pidgeiot.js";

const DAY = 86400;
const LIMIT = 3;
const KEEP = 64;
const RESEND_SECS = 10; // the same report resent is dated a few seconds apart

export default function rebootLoop(event) {
  const saved = event.state.pigeon.boots;
  // An older version of this rule may have stored another shape.
  const stored = Array.isArray(saved) ? saved : [];
  const boots = [...stored];
  for (const r of event.readings) {
    const up = num(r.metrics.uptime_s);
    const prev = num(r.previous.uptime_s?.value);
    if (r.metrics.reset_cause !== undefined) {
      const boot = up === null ? r.at : r.at - up;
      if (!boots.some((t) => Math.abs(t - boot) <= RESEND_SECS)) boots.push(boot);
    } else if (up !== null && prev !== null && up < prev) {
      const boot = r.at - up;
      // This boot's reset_cause report, if it arrived, is dated between the boot and now.
      if (!boots.some((t) => t >= boot - RESEND_SECS && t <= r.at)) boots.push(boot);
    }
  }
  const kept = within(boots, event.received_at, DAY).slice(-KEEP);

  const result = {};
  if (kept.join() !== stored.join()) result.state = { pigeon: { boots: kept } };
  if (kept.length !== stored.length) {
    result.values = [{ key: "boots_24h", value: String(kept.length) }];
  }
  // Restated every batch: the platform mails only when the firing flag changes.
  const firing = kept.length > LIMIT;
  result.alerts = [{
    key: "reboot-loop",
    kind: firing ? "fire" : "clear",
    severity: firing ? "critical" : "info",
    scope: "pigeon",
    subject: firing ? "Reboot loop" : "Reboot loop cleared",
    body: kept.length + " boots in the last 24 h.",
  }];
  return result;
}
