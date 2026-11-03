// The worked rules' manifest, which the fixtures run. dovecote generates this file per bundle:
// each enabled rule at its deployed version, keyed by the rule's uuid.
import r0 from "./r6-watchdog.js";
import r1 from "./r7-reboot-loop.js";
import r2 from "./r10-swiftly-down.js";

export default {
  r6: { version: 1, run: r0 },
  r7: { version: 1, run: r1 },
  r10: { version: 1, run: r2 },
};
