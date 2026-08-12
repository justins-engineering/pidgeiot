# design-sync notes — fancier / PidgeIoT

- This is a **tokens-only** sync by deliberate owner choice: fancier's components are Rust/Dioxus (no JS/React library, no Storybook, no dist), so nothing component-shaped can ship. What syncs is the styling: full DaisyUI class set + the two custom themes + chart tokens, taught to the design agent via `conventions.md` (wired as `readmeHeader`).
- `cfg.entry` points at `.design-sync/ds-entry.mjs`, a deliberately empty module — it exists only so the converter has a JS entry to walk/bundle; zero exports is what routes the build into its tokens-only path (`[ZERO_MATCH] … treating as tokens-only DS`).
- `package.json` gained `"name": "fancier"` for the sync — the converter resolves the package dir by walking up from the entry to the nearest *named* package.json; without a name the walk would escape the crate dir.
- `cfg.cssEntry` is a **composed** file, `.design-sync/.cache/ds-css.css` = `node_modules/daisyui/daisyui.css` (the dependency's shipped full build — all component classes) + freshly compiled `assets/styling/main.css` (custom themes + chart tokens, later in the cascade so they override DaisyUI's stock light/dark). The composition exists because Tailwind 4 JIT-compiles only classes fancier's Rust source uses — shipping `main.css` alone would leave the design agent's unused-by-fancier DaisyUI classes silently unstyled. `.cache/` is gitignored; `cfg.buildCmd` regenerates both halves.
- `assets/styling/main.css` is generated and gitignored — never hand-edit or commit it; always rebuild via the Tailwind CLI (see `buildCmd`).
- Theme trap (from CLAUDE.md, encoded in conventions.md): `--color-neutral` is white-on-white (light) / black-on-black (dark). Any conventions edit must keep the "never use neutral" warning.
- Playwright: latest playwright's pinned chromium build (1234) matched the existing `~/.cache/ms-playwright/` cache, so no browser download was needed; deps live in `.ds-sync/node_modules`.

## Re-sync risks

- The DaisyUI full-CSS + main.css concatenation silently drifts if the DaisyUI major version changes class names — after a DaisyUI upgrade, re-verify a few conventions.md table entries against the fresh `_ds_bundle.css`.
- conventions.md enumerates class/token names verified against the built CSS on 2026-08-12; theme edits in `assets/tailwind.css` (new tokens, renamed chart vars) must be reflected there or the header names things that no longer exist.
- The render check is vacuous here (0 previews) — nothing visual is machine-verified. The real check is a human opening the project in claude.ai/design and building a test screen in both themes.
- No `_ds_sync.json` verification skips apply (no components), so re-syncs are always cheap full rebuilds; `resync.mjs --remote` still diffs styling/bundle hashes to decide whether an upload is needed.
