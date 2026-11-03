// Throws from a getter while its result is serialized.
export default () => ({ get alerts() { throw new Error("gotcha"); } });
