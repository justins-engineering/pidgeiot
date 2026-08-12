#!/usr/bin/env bash
# Host launcher for the RFC 9146 CID netns rebind harness (Tier 2 of
# docs/infra/coap-cid-design.md's test plan). Builds the harness image from
# the repo root and runs it privileged -- the netns/veth/nftables/conntrack
# work needs it, and it is a throwaway container on a dev box, never the VPS.
#
#   scripts/test/cid-netns-harness.sh            # build + run all cells
#   scripts/test/cid-netns-harness.sh --no-build # reuse an existing image
#
# The container runs with no external network (--network none): every wire
# path is internal netns/veth, and denying egress is one more guarantee the
# gate can't accidentally reach anything real. No production traffic and no
# real credentials: the PSK endpoint is a stub and the PSKs are
# harness-minted (run-cid-netns.sh / cid-harness/).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE=loft-cid-harness

if [ "${1:-}" != "--no-build" ]; then
  echo "building $IMAGE ..."
  docker build -f "$REPO_ROOT/scripts/test/Dockerfile.cid-harness" -t "$IMAGE" "$REPO_ROOT"
fi

# A host dir the container copies failing-run evidence (pcaps, journals,
# client output) into before its own cleanup wipes them -- the pcap is
# exactly what you need when the gate goes red.
ARTIFACT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/loft-cid-artifacts.XXXXXX")"

echo "running netns rebind harness (privileged, no external network) ..."
set +e
docker run --rm --privileged --network none \
  -v "$ARTIFACT_DIR:/artifacts" "$IMAGE"
status=$?
set -e

if [ "$status" -eq 0 ]; then
  rm -rf "$ARTIFACT_DIR"
else
  echo "harness FAILED (exit $status); preserved evidence: $ARTIFACT_DIR"
  ls -la "$ARTIFACT_DIR" 2>/dev/null || true
fi
exit "$status"
