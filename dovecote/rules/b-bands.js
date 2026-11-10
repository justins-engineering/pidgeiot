// Made by the rule builder: edit it there, or convert it to code.
import { run } from "./builder.js";

const MODEL = JSON.parse("{\"v\":1,\"when\":{\"telemetry\":{\"key\":\"lux\"}},\"match\":\"all\",\"then\":[{\"do\":\"set_value\",\"key\":\"brightness\",\"value\":\"60\",\"bands\":[{\"if\":{\"is\":\"compare\",\"key\":\"lux\",\"op\":\"below\",\"to\":{\"number\":50.0}},\"value\":\"30\"},{\"if\":{\"is\":\"compare\",\"key\":\"lux\",\"op\":\"above\",\"to\":{\"number\":500.0}},\"value\":\"80\"}]}]}");

export default (event) => run(MODEL, event);
