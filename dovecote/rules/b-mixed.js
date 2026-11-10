// Made by the rule builder: edit it there, or convert it to code.
import { run } from "./builder.js";

const MODEL = JSON.parse("{\"v\":1,\"when\":{\"telemetry\":{\"key\":\"reset_cause\"}},\"match\":\"any\",\"only_if\":[{\"is\":\"bit_set\",\"key\":\"reset_cause\",\"bit\":4},{\"is\":\"changed\",\"key\":\"layout\"},{\"is\":\"changed_by\",\"key\":\"temp_c\",\"more_than\":5.0},{\"is\":\"compare\",\"key\":\"mode\",\"op\":\"not_equals\",\"to\":{\"text\":\"auto\"}},{\"is\":\"compare\",\"key\":\"temp_in\",\"op\":\"above\",\"to\":{\"key\":\"temp_out\"}}],\"then\":[{\"do\":\"mail\",\"severity\":\"warning\",\"subject\":\"Look at {pigeon}\",\"body\":\"reset_cause {reset_cause}\\nlayout {layout} {unknown}\",\"repeat\":\"once\"}],\"otherwise\":[{\"do\":\"set_value\",\"key\":\"last_cause\",\"value\":\"{reset_cause}\"}]}");

export default (event) => run(MODEL, event);
