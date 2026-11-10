// Made by the rule builder: edit it there, or convert it to code.
import { run } from "./builder.js";

const MODEL = JSON.parse("{\"v\":1,\"when\":{\"telemetry\":{\"key\":\"temp_c\"}},\"match\":\"all\",\"only_if\":[{\"is\":\"counted\",\"key\":\"reset_cause\",\"more_than\":2,\"within_secs\":86400}],\"then\":[{\"do\":\"mail\",\"severity\":\"warning\",\"subject\":\"Rebooting: {pigeon}\",\"repeat\":\"start_stop\"}]}");

export default (event) => run(MODEL, event);
