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
import lonelog from "./h-lonelog.js";
import lonethrow from "./h-lonethrow.js";
import quotes from "./h-quotes.js";
import loud from "./h-loud.js";
import same from "./h-same.js";
import typo from "./h-typo.js";
import proto from "./h-proto.js";

export default {
  big: { version: 1, run: big }, cyc: { version: 1, run: cyc }, get: { version: 1, run: get },
  huge: { version: 1, run: huge }, wide: { version: 1, run: wide }, ok: { version: 1, run: ok },
  hang: { version: 1, run: hang }, after: { version: 1, run: after },
  lone: { version: 1, run: lone }, lonelog: { version: 1, run: lonelog },
  lonethrow: { version: 1, run: lonethrow }, quotes: { version: 1, run: quotes },
  loud: { version: 1, run: loud }, same: { version: 1, run: same },
  typo: { version: 1, run: typo }, proto: { version: 1, run: proto },
};
