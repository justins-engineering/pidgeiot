// Inside every per-rule cap, but each byte of it doubles or grows six-fold once escaped: a flock
// document just under 32 KiB and eight values at the value cap, all quotes.
export default () => {
  for (let n = 0; n < 25; n++) console.log("\u0001".repeat(300));
  const values = [];
  for (let n = 0; n < 8; n++) values.push({ key: "q" + n, value: "\"".repeat(1000) });
  return { values, state: { flock: { s: "\"".repeat(16000) } } };
};
