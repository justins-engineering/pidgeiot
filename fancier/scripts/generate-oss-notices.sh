#!/usr/bin/env bash
# Regenerates the open-source license inventory rendered at /open-source
# (fancier/src/views/open_source.rs) from the actual, current Cargo
# dependency graph, so the page can't drift stale the way a hand-maintained
# list would. Run by fancier/scripts/build-release.sh on every release
# build. The checked-in output files (fancier/generated/oss-licenses.md,
# dovecote/generated/oss-licenses.md) are committed snapshots so plain
# `cargo check`/`dx serve` keep working without cargo-about installed --
# only a release build regenerates them.
set -euo pipefail
cd "$(dirname "$0")/../.."

if ! command -v cargo-about &>/dev/null; then
  echo "generate-oss-notices.sh: cargo-about not found on PATH." >&2
  echo "Install with: cargo install cargo-about --locked --features cli" >&2
  exit 1
fi

TEMPLATE="fancier/scripts/oss-licenses.hbs"

# fancier is the requirement (its wasm bundle is what ships to a browser);
# dovecote's server-side tree is included too since it's cheap and reads
# better than omission for an open platform. Both share one about.toml --
# the accepted-license policy is a whole-project decision, not a per-crate
# one.
cargo about generate --config about.toml -m fancier/Cargo.toml \
  -o fancier/generated/oss-licenses.md "$TEMPLATE"
cargo about generate --config about.toml -m dovecote/Cargo.toml \
  -o dovecote/generated/oss-licenses.md "$TEMPLATE"

# The /open-source page (fancier/src/views/open_source.rs) renders both
# generated files on one HTML page, so the `#license-MIT`-style anchor ids
# each file's template emits (see oss-licenses.hbs) would collide -- both
# files independently mint the same id for the same SPDX license. Namespace
# them per source crate post-generation rather than teaching the template
# about a prefix it has no way to receive (cargo-about's CLI only feeds it
# the fixed license-report JSON, no extra template variables).
sed -i -e 's/#license-/#license-fancier-/g' -e 's/id="license-/id="license-fancier-/g' \
  fancier/generated/oss-licenses.md
sed -i -e 's/#license-/#license-dovecote-/g' -e 's/id="license-/id="license-dovecote-/g' \
  dovecote/generated/oss-licenses.md

echo "generate-oss-notices.sh: wrote fancier/generated/oss-licenses.md and dovecote/generated/oss-licenses.md"
