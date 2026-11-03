// Dials and writes to a socket. Imported at run time, so Bun can load the manifest. Run under
// workerd only.
export default async () => {
  const { connect } = await import("cloudflare:sockets");
  const writer = connect({ hostname: "8.8.8.8", port: 53 }).writable.getWriter();
  await writer.write(new Uint8Array([0]));
  return {};
};
