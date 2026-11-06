// The worked rules' manifest, which the fixtures run, the dashboard's starter rule and the
// builder's fixture models (run-builder.mjs). dovecote
// generates this file per bundle: each deployed rule at its version, keyed by its uuid.
import r0 from "./r6-watchdog.js";
import r1 from "./r7-reboot-loop.js";
import r2 from "./r10-swiftly-down.js";
import r3 from "./starter.js";
import b0 from "./b-hot.js";
import b1 from "./b-reboots.js";
import b2 from "./b-bands.js";
import b3 from "./b-mixed.js";

export default {
  r6: { version: 1, run: r0 },
  r7: { version: 1, run: r1 },
  r10: { version: 1, run: r2 },
  starter: { version: 1, run: r3 },
  b_hot: { version: 1, run: b0 },
  b_reboots: { version: 1, run: b1 },
  b_bands: { version: 1, run: b2 },
  b_mixed: { version: 1, run: b3 },
};
