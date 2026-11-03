// The hostile manifest: each rule tries to break its own dispatch.
import big from "./h-bigint.js";
import cyc from "./h-cyclic.js";
import get from "./h-getter.js";
import huge from "./h-huge.js";
import wide from "./h-wide.js";
import ok from "./h-ok.js";
import hang from "./h-hang.js";
import after from "./h-after.js";
import lone from "./h-lone.js";
import proto from "./h-proto.js";

export default {
  big: { version: 1, run: big }, cyc: { version: 1, run: cyc }, get: { version: 1, run: get },
  huge: { version: 1, run: huge }, wide: { version: 1, run: wide }, ok: { version: 1, run: ok },
  hang: { version: 1, run: hang }, after: { version: 1, run: after },
  lone: { version: 1, run: lone }, proto: { version: 1, run: proto },
};
