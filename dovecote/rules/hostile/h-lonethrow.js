// Throws a message ending in half of a surrogate pair.
export default () => { throw "x\ud800"; };
