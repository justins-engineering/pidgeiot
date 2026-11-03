// Lone surrogates in a key and a value, beside a whole pair and an escaped backslash.
export default () => ({ values: [{ key: "k\ud800", value: "v\udfff \u{1f600} \\ud800" }] });
