//! Terms assent API client -- see `docs/api.md`'s "Terms assent" section.
//!
//! Nothing here is cached in `LocalSession`: the gate in `AuthGuard` owns
//! the one status this app reads, and `accept` answers with the status the
//! write produced, so a client never re-reads to confirm its own write.
//! `None` means the status could not be read, which the gate treats as
//! unknown rather than as missing assent.

use crate::api::orgs::parse;
use crate::api::{fetch_json, fetch_json_any_status};
use capsules::consent::TermsAssentStatus;
use dioxus::logger::tracing::error;

/// `GET /account/terms`. Which version the account has accepted, and which
/// one the API considers current.
pub async fn status() -> Option<TermsAssentStatus> {
  let response = fetch_json("GET", "/account/terms", None).await?;
  parse(response).await
}

/// `POST /account/terms`. Records assent to the version the API publishes
/// and answers the new status. The body is deliberately empty: the surface
/// is the server's to name.
///
/// `fetch_json_any_status` rather than `fetch_json` so a refusal is logged
/// with its status -- the panel shows an inline error and stays up, and
/// only a status this call could actually read says which failure it was.
pub async fn accept() -> Option<TermsAssentStatus> {
  let response = fetch_json_any_status("POST", "/account/terms", None).await?;
  if !response.ok() {
    error!(
      "POST /account/terms failed with status: {}",
      response.status()
    );
    return None;
  }
  parse(response).await
}
