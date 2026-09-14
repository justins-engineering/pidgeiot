# Published legal documents

The four documents `fancier` serves at `/terms/`, `/privacy/`, `/dpa/` and `/subprocessors/`. They
are compiled into the dashboard binary with `include_str!` and rendered through `pulldown-cmark`,
the same way `docs/api.md` backs `/api-reference/`, so the file in this directory is the page.

| File | Route | Source it was copied from |
| --- | --- | --- |
| `terms.md` | `/terms/` | `24f-terms-privacy/terms-of-service-final-2026-09-11.md` |
| `privacy.md` | `/privacy/` | `24f-terms-privacy/privacy-policy-final-2026-09-11.md` |
| `dpa.md` | `/dpa/` | `24-eu-paperwork/dpa.md`, with the edits in `docs/design/terms-assent-and-legal-pages.md` section 8 applied |
| `subprocessors.md` | `/subprocessors/` | `24-eu-paperwork/subprocessors.md`, same section |

`archive/` holds superseded published text, named for the version it was published as, so a consent
row stamped with an older version can still be resolved back to the words that were on screen.

## The copy is one way

Business folder to repository, never back. The business copies are counsel's working drafts and are
meant to move ahead of what is published; a two-way copy would drag a draft onto the live page. The
Terms and the Privacy Policy are verbatim, with only the "Last updated" line replaced. The DPA and
the sub-processor list are an interim text: counsel's substantive review is pending, and her version
replaces the published one under the DPA's own Section 12.2.

## The date on the page

Each document carries the literal token `{{LAST_UPDATED}}` on its "Last updated" line, substituted
twice over: by `fancier`'s `helpers::legal_doc::render` for the HTML page, and by
`fancier/scripts/build-release.sh` for the markdown variant. Terms, DPA and sub-processor list
render `capsules::TERMS_VERSION`; the Privacy Policy renders `capsules::PRIVACY_NOTICE_VERSION`.
Both constants are ISO dates and a test holds them to that shape.

`TERMS_VERSION` is also what every Terms assent row is stamped with, so bumping it asks every
account to accept again on its next sign-in. A wording fix that needs no fresh assent must not move
it.

## Deploy order on a version bump: fancier first, then dovecote

`fancier` carries the pages; `dovecote` decides which version is current and stamps the rows. If
`dovecote` goes first it answers the new version while the pages still show the old one, the assent
gate fires, and every row written in that window says an account accepted text it was not shown.
The other order is harmless: the pages show the new text for a few minutes while `dovecote` still
considers the old one current, so no gate fires and no row is written.
