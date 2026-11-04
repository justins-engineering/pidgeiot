// Burns CPU with no I/O, then returns: nothing the harness can see.
export default () => {
  let x = 0;
  for (let i = 0; i < 2e6; i++) x = (x + i) % 9973;
  return x < 0 ? { values: [] } : {};
};
