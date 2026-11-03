// Ignores the answer to a request, so only a rejected fetch can fail it. Run under workerd only.
export default async () => {
  await fetch("https://example.com/");
  return {};
};
