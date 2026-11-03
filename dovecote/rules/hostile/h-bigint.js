// Returns a BigInt, which JSON cannot carry.
export default () => ({ values: [{ key: "x", value: 1n }] });
