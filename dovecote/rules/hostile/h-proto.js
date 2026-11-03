// A state key named __proto__, which a plain assignment would take as the prototype.
export default () => ({ state: { pigeon: { ["__proto__"]: "kept" } } });
