// A few milliseconds of CPU with no I/O, then returns.
export default () => {
  let x = 0;
  for (let i = 0; i < 2e6; i++) x = (x + i) % 9973;
  return x < 0 ? { values: [] } : {};
};
