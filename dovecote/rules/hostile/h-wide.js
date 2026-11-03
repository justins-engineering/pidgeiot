// Under 4,096 UTF-16 units, over 4 KiB of UTF-8.
export default () => ({ state: { pigeon: { s: "é".repeat(3000) } } });
