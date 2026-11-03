#!/bin/sh
# Every harness gate: the prelude's receivers, the fixtures and hostile cases under Bun, then both
# suites again under workerd, which refuses what Bun lets pass.
set -e
cd "$(dirname "$0")"
bun run-prelude.mjs
bun run-fixtures.mjs
bun run-hostile.mjs
bun run-workerd.mjs
