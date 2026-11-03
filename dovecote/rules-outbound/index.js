// The rules namespace's outbound Worker. Rules have no egress, so every request and every socket a
// rule opens is refused here and logged with the flock whose rules tried. The platform refuses a
// socket without a connect() handler too, but records it as this Worker's exception, naming no
// flock.
export default {
  async fetch(request, env) {
    console.log("rule egress refused", env.flock_id, new URL(request.url).host);
    return new Response("egress is not available to rules", { status: 403 });
  },
  // Closed before anything else, so the refusal never waits on the socket. `opened` names the
  // address the rule dialed.
  async connect(socket, env) {
    await socket.close().catch(() => {});
    const opened = await socket.opened.catch(() => null);
    console.log("rule socket refused", env.flock_id, opened?.localAddress ?? null);
  },
};
