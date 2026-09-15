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

## Decisions counsel owns before the first deploy

These are not agent actions and not engineering calls. Each is a place where what we deploy and
what we publish have to be made to agree.

- **The assent record is not in the Privacy Policy's retention table.** Every Terms assent row is
  version, account and time, with no address and no user agent, and erasure deletes it with the
  identity, because that is what the published policy promises. Keeping it past deletion under
  Article 17(3)(e) for the contract limitation period is the record worth having and takes one new
  retention row plus one collection sentence in the settled policy. Until those land, the code
  stays inside what the page says.

## Shipping a change to these documents

Nothing here is an agent action. The full reasoning is in
`docs/design/terms-assent-and-legal-pages.md` section 12; this is the list.

1. Copy the new text in from the business folder, keeping the `{{LAST_UPDATED}}` line, and set
   `TERMS_VERSION` (and `PRIVACY_NOTICE_VERSION` if the policy moved) to the deploy date.
   Answer the open sub-processor question first: the useSend row states the position as the
   deployed configuration stands, so removing the vendor, obtaining a DPA from it, or switching
   the fallback rail is a decision taken before the page ships, and the row is edited to match.
2. Apply `infra/migrations/2026-09-14-terms-assent.sql` to the staging database, then deploy
   fancier and dovecote to staging **in that order**.
3. Sign in on staging: the gate appears, accepting clears it, and a reload within 30 seconds
   does not bring it back. Inside that window is the only check that proves the read is
   uncacheable; dev cannot reproduce it at all.
4. `curl` each of `/terms/`, `/privacy/`, `/dpa/` and `/subprocessors/` with no JavaScript and
   confirm the substituted date and no surviving `{{LAST_UPDATED}}`.
5. Apply the migration to production, then deploy fancier and dovecote in the same order.

**Rollback.** If the gate walls everyone out of the dashboard, redeploy the previous fancier
version: the gate is client-side and dovecote needs no change, so the dashboard comes back
without touching the database or the rows already written. Worth knowing before it is needed.
