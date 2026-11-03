// The rules namespace's outbound Worker. Rules have no egress, so every request and every socket a
// rule opens is refused here and logged with the flock whose rules tried. The platform refuses a
// socket without a connect() handler too, but records it as this Worker's exception, naming no
// flock.

// A rule can be refused thousands of times in one dispatch and Workers Logs bill per event, so a
// flock gets one line per kind a minute in each isolate, carrying how many refusals went unlogged
// before it. Refusing never depends on it.
const QUIET_MS = 60_000;
const quiet = new Map();
// The rule chooses the host and dialed address, so they are cut before they reach the log.
const cut = (s) => String(s).slice(0, 300);

function note(kind, flock, target) {
  const key = kind + " " + flock;
  const now = Date.now();
  const last = quiet.get(key);
  if (last && now < last.until) {
    last.skipped++;
    return;
  }
  if (quiet.size >= 1024) quiet.clear();
  quiet.set(key, { until: now + QUIET_MS, skipped: 0 });
  console.log(kind, flock, target, last?.skipped ?? 0);
}

export default {
  // Thrown rather than answered, so a rule that ignores the status still fails its run.
  async fetch(request, env) {
    const host = cut(new URL(request.url).host);
    note("rule egress refused", env.flock_id, host);
    throw new Error("egress is not available to rules: " + host);
  },
  // Closed before anything else, so the refusal never waits on the socket. This end of the socket
  // reports the address the rule dialed as its `localAddress`.
  async connect(socket, env) {
    await socket.close().catch(() => {});
    const opened = await socket.opened.catch(() => null);
    const address = opened?.localAddress ? cut(opened.localAddress) : null;
    note("rule socket refused", env.flock_id, address);
  },
};
