// Logs half of a surrogate pair, as slicing device text does.
export default () => { console.log("stage " + "ab\u{1f600}".slice(0, 3)); return {}; };
