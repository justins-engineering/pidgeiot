// The worked rules' manifest, which the fixtures run, and the dashboard's starter rule. dovecote
// generates this file per bundle: each deployed rule at its version, keyed by its uuid.
import r0 from "./r6-watchdog.js";
import r1 from "./r7-reboot-loop.js";
import r2 from "./r10-swiftly-down.js";
import r3 from "./starter.js";

export default {
  r6: { version: 1, run: r0 },
  r7: { version: 1, run: r1 },
  r10: { version: 1, run: r2 },
  starter: { version: 1, run: r3 },
};
