//! Per-account dashboard preferences, held server-side so they survive a
//! browser that clears site data on close.
//!
//! One opaque JSON document per `scope_key`. The platform never reads
//! inside the document, which is what lets a new widget claim a key
//! without a schema change -- and what makes the value's shape entirely
//! the dashboard's business (see `fancier`'s `helpers::graph_store` for
//! the first one).
//!
//! Ownership is the Kratos identity, not the organization: a saved graph
//! is how one person chose to look at a fleet, not a fact about the fleet.

use crate::JsonString;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Size cap on one stored document, enforced by dovecote's
/// `PUT /dashboard-state/:scope_key`. A page's worth of graph configs runs
/// to a few hundred bytes, so this is abuse headroom rather than a tuned
/// limit.
pub const MAX_DASHBOARD_STATE_BYTES: usize = 16 * 1024;

/// Longest accepted `scope_key`. The longest key the dashboard mints is
/// `graphs.v1.pigeon.` plus a 64-character Durable Object id.
pub const MAX_DASHBOARD_STATE_KEY_BYTES: usize = 128;

/// How many distinct keys one account may hold. A scope exists per pigeon
/// and per flock, so reaching this needs a fleet far past any tier's
/// included device count -- or someone minting keys for their own sake.
pub const MAX_DASHBOARD_STATE_KEYS: i64 = 256;

/// Whether a `scope_key` is one the store accepts: non-empty, within
/// [`MAX_DASHBOARD_STATE_KEY_BYTES`], and drawn from `[A-Za-z0-9._-]` so
/// it rides in a URL path segment unescaped.
pub fn valid_scope_key(key: &str) -> bool {
  !key.is_empty()
    && key.len() <= MAX_DASHBOARD_STATE_KEY_BYTES
    && key
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// DB model for one `dashboard_state` row. Postgres hands back a native
/// `OffsetDateTime` for `TIMESTAMPTZ`, so only `value` needs converting:
/// it is read as `::text`, the same cast every other JSONB column in this
/// workspace uses (see [`crate::AlertDefinitionRow`]).
#[derive(Deserialize, Debug)]
pub struct DashboardStateEntryRow {
  pub scope_key: String,
  pub value: String,
  pub updated_at: OffsetDateTime,
}

impl From<DashboardStateEntryRow> for DashboardStateEntry {
  fn from(row: DashboardStateEntryRow) -> Self {
    Self {
      scope_key: row.scope_key,
      // The column is JSONB, so Postgres cannot hand back anything but
      // valid JSON; the fallback keeps a hand-edited row readable rather
      // than failing the response, matching this crate's
      // permissive-on-malformed-stored-data convention.
      value: JsonString::new(row.value).unwrap_or_else(|_| JsonString(String::from("{}"))),
      updated_at: row.updated_at,
    }
  }
}

/// One stored document, as `GET`/`PUT /dashboard-state/:scope_key` return
/// it. `updated_at` is the server's own clock, which is what lets a client
/// tell a copy it wrote from one another browser wrote later.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DashboardStateEntry {
  pub scope_key: String,
  pub value: JsonString,
  #[serde(with = "time::serde::rfc3339")]
  pub updated_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_keys_the_dashboard_mints_are_accepted() {
    assert!(valid_scope_key(
      &("graphs.v1.pigeon.".to_string() + &"a1".repeat(32))
    ));
    assert!(valid_scope_key(
      "graphs.v1.flock.2f1a3b4c-5d6e-7f80-9012-3456789abcde"
    ));
  }

  #[test]
  fn keys_that_would_escape_their_path_segment_are_refused() {
    assert!(!valid_scope_key(""));
    assert!(!valid_scope_key("graphs.v1.pigeon./../orgs"));
    assert!(!valid_scope_key("graphs.v1 pigeon"));
    assert!(!valid_scope_key("graphs%2ev1"));
    assert!(!valid_scope_key(
      &"k".repeat(MAX_DASHBOARD_STATE_KEY_BYTES + 1)
    ));
  }
}
