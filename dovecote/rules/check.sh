#!/bin/sh
# Every harness gate: the prelude's receivers, the fixtures, hostile cases and builder rules under
# Bun, then those suites again under workerd, which refuses what Bun lets pass, behind the
# outbound Worker, then that Worker's handlers alone.
# Needs `bun install` in dovecote once per checkout, for wrangler's workerd. A RULES_URL from the
# shell is dropped so the Bun steps run under Bun.
set -e
cd "$(dirname "$0")"
bun run-prelude.mjs
env -u RULES_URL bun run-fixtures.mjs
env -u RULES_URL bun run-hostile.mjs
env -u RULES_URL bun run-builder.mjs
bun run-workerd.mjs
bun ../rules-outbound/test.mjs
