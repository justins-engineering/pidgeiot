// Made by the rule builder: edit it there, or convert it to code.
import { run } from "./builder.js";

const MODEL = JSON.parse("{\"v\":1,\"when\":{\"telemetry\":{\"key\":\"temp_c\"}},\"match\":\"all\",\"only_if\":[{\"is\":\"compare\",\"key\":\"temp_c\",\"op\":\"above\",\"to\":{\"number\":40.0},\"for_secs\":300}],\"then\":[{\"do\":\"mail\",\"severity\":\"warning\",\"subject\":\"Sign running hot: {pigeon} at {temp_c} C\",\"repeat\":\"start_stop\"},{\"do\":\"set_value\",\"key\":\"fan\",\"value\":\"on\"}],\"otherwise\":[{\"do\":\"set_value\",\"key\":\"fan\",\"value\":\"auto\"}]}");

export default (event) => run(MODEL, event);
