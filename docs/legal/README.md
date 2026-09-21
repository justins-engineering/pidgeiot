# Published legal documents

The four documents `fancier` serves at `/terms/`, `/privacy/`, `/dpa/` and `/subprocessors/`. They
are compiled into the dashboard binary with `include_str!` and rendered through `pulldown-cmark`,
the same way `docs/api.md` backs `/api-reference/`, so the file in this directory is the page.

| File | Route | Source it was copied from |
| --- | --- | --- |
| `terms.md` | `/terms/` | `24f-terms-privacy/terms-of-service-final-2026-09-15.md` |
| `privacy.md` | `/privacy/` | `24f-terms-privacy/privacy-policy-final-2026-09-15.md` |
| `dpa.md` | `/dpa/` | `24f-terms-privacy/data-processing-agreement-final-2026-09-20.md` |
| `subprocessors.md` | `/subprocessors/` | `24-eu-paperwork/subprocessors.md`, then maintained here |

`archive/` holds superseded published text, named for the version it was published as, so a consent
row stamped with an older version can still be resolved back to the words that were on screen.

The `/privacy/#privacy-*` anchors the hand-written page shipped before 2026-09-14 do not survive:
heading ids now come from the heading text, the way `/api-reference/` has always produced them.

## The copy is one way

Business folder to repository, never back. The business copies are counsel's working drafts and are
meant to move ahead of what is published; a two-way copy would drag a draft onto the live page. The
Terms, the Privacy Policy and the DPA are verbatim, with only the "Last updated" line replaced. The
sub-processor list is ours: counsel's Annex III points at it and it is maintained under the DPA's
own Section 6.

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

- **The assent record is now in the Privacy Policy's retention table, and that closes the
  Article 17(3)(e) question against us.** Every Terms assent row is version, account, time and
  source, with no address and no user agent, and the policy says it is deleted with the account.
  The DPA's Annex II G.3 states the same as a fact and keys any exception to what the policy
  permits, so keeping a row past deletion for the contract limitation period would now make a
  published clause false. Moving that line is a decision about the DPA as well as the policy.

## Shipping a change to these documents

Nothing here is an agent action. The full reasoning is in
`docs/design/terms-assent-and-legal-pages.md` section 11; this is the list.

1. Copy the new text in from the business folder, keeping the `{{LAST_UPDATED}}` line, and set
   `TERMS_VERSION` (and `PRIVACY_NOTICE_VERSION` if the policy moved) to the deploy date. The
   open sub-processor question is answered: the fallback email vendor was removed from the
   Service, so the only vendor on the list that carries our mail is the edge provider. The
   Resend row stays, and says on its face that no data reaches it.
2. Apply `infra/migrations/2026-08-27-consent-events.sql` and then
   `infra/migrations/2026-09-14-terms-assent.sql` to the staging database, in that order, then
   deploy fancier and dovecote to staging **in that order** too. The first file creates the table
   the second one alters: it was only ever needed by the Kratos consent hook, so whether any
   deployed database has it depends on whether that hook was ever wired. Both are idempotent, so
   applying the earlier one where it already ran does nothing.
3. Sign in on staging: the gate appears, accepting clears it, and a reload within 30 seconds
   does not bring it back. Inside that window is the only check that proves the read is
   uncacheable; dev cannot reproduce it at all.
4. `curl` each of `/terms/`, `/privacy/`, `/dpa/` and `/subprocessors/` with no JavaScript and
   confirm the substituted date and no surviving `{{LAST_UPDATED}}`.
5. Email every organization owner on file that the Terms, the Privacy Policy, the DPA and the
   sub-processor list are published, and keep the sent message with the signed legal records.
   The DPA names notice followed by continued use as one of its three acceptance routes, and
   for an owner who never countersigns and never reaches the dashboard gate that message is the
   only evidence the notice was given.
6. Apply both migrations to production, in the same order, then deploy fancier and dovecote.

**Rollback.** If the gate walls everyone out of the dashboard, redeploy the previous fancier
version: the gate is client-side and dovecote needs no change, so the dashboard comes back
without touching the database or the rows already written. Worth knowing before it is needed.
