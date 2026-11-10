// Made by the rule builder: edit it there, or convert it to code.
import { run } from "./builder.js";

const MODEL = JSON.parse("{\"v\":1,\"when\":{\"telemetry\":{\"key\":\"reset_cause\"}},\"match\":\"all\",\"only_if\":[{\"is\":\"counted\",\"key\":\"reset_cause\",\"more_than\":3,\"within_secs\":86400}],\"then\":[{\"do\":\"mail\",\"severity\":\"critical\",\"subject\":\"Reboot loop on {pigeon}\",\"body\":\"{pigeon} booted more than three times in a day.\",\"repeat\":\"start_stop\"}]}");

export default (event) => run(MODEL, event);
