// One mail per watchdog boot, naming the stage the boot reached. The built-in Threshold
// cannot do this: it never mails a one-shot key's first event and has no bit test.
// reset_cause and reset_stage come once per boot but not always in one report, so the
// last stage seen is kept in state.
import { bits, num } from "./pidgeiot.js";

const WATCHDOG = 0x10; // RESET_WATCHDOG, bit 4 of Zephyr's hwinfo reset cause
const STAGE_FRESH_SECS = 600;
const RESEND_SECS = 10; // the same report resent is dated a few seconds apart
const STAGE_CHARS = 200; // keeps the body under its 1,024-byte cap

// Device text in a mail or in state: a control character (Unicode Cc) is refused in a subject or
// body, U+0000 anywhere in state, and length is capped.
const clean = (s, max) => String(s).replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").slice(0, max);

export default function watchdogReset(event) {
  const saved = event.state.pigeon;
  let stage = saved.stage ?? null;
  let stageAt = saved.stageAt ?? 0;
  let lastBoot = saved.lastBoot ?? null;
  for (const r of event.readings) {
    if (r.metrics.reset_stage !== undefined) {
      stage = clean(r.metrics.reset_stage, STAGE_CHARS);
      stageAt = r.at;
    }
  }
  const alerts = [];
  for (const r of event.readings) {
    if (!bits(r.metrics.reset_cause, WATCHDOG)) continue;
    // Uptime dates the boot itself; one past the reading's own time dates nothing.
    const up = num(r.metrics.uptime_s);
    const boot = r.at - (up !== null && up >= 0 && up <= r.at ? up : 0);
    if (lastBoot !== null && Math.abs(boot - lastBoot) <= RESEND_SECS) continue;
    lastBoot = boot;
    // The boot's minute: a repeat mails once. A resend racing the original on the same empty
    // state, its boot across a minute's rounding edge, mails twice; accepted as a duplicate.
    const key = "watchdog-" + Math.round(boot / 60);
    // A key may appear once per result: a second boot in the same minute rides the first mail.
    if (alerts.some((a) => a.key === key)) continue;
    const fresh = stage !== null && Math.abs(r.at - stageAt) <= STAGE_FRESH_SECS;
    alerts.push({
      key,
      kind: "notify",
      severity: "critical",
      scope: "pigeon",
      subject: "Watchdog reset",
      body: "Reset stage: " + (fresh ? clean(stage, STAGE_CHARS) : "not reported") +
        ". reset_cause " + clean(r.metrics.reset_cause, 20) + ".",
    });
  }
  const patch = {};
  if (stageAt !== (saved.stageAt ?? 0)) Object.assign(patch, { stage, stageAt });
  if (lastBoot !== (saved.lastBoot ?? null)) patch.lastBoot = lastBoot;
  const result = { alerts };
  if (Object.keys(patch).length > 0) result.state = { pigeon: patch };
  return result;
}
