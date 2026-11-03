// Inside every per-rule cap, but each byte of it doubles or grows six-fold once escaped.
export default () => {
  for (let n = 0; n < 25; n++) console.log("\u0001".repeat(300));
  return { state: { flock: { s: "\"".repeat(24000) } } };
};
