// Returns more than the 48 KiB result cap.
export default () => ({ state: { flock: { s: "x".repeat(50000) } } });
