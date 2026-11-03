// Returns a cycle.
export default () => { const o = {}; o.self = o; return { state: { pigeon: o } }; };
