// Where a user's saved graphs live. Today that is this browser's own
// localStorage. The intended end state is a `dashboard_widgets` row holding
// the same JSON blob, which is blocked on an unresolved product question --
// whether a graph belongs to the user who made it or to the flock everyone
// in an org shares -- and that answer sets the table key, so guessing it is
// expensive to undo.
//
// This module exists so the answer, once given, changes one file. The
// shapes are already the ones a server-side version needs: `GraphScope` is
// exactly the (scope_kind, scope_id) pair such a row would be keyed by, and
// `GraphDef` serializes to the blob a JSONB column would store verbatim. No
// caller builds a storage key, names a scope with a bare string, or reaches
// for serde itself. Migrating then means reimplementing the two functions
// below against the API (and making them async) rather than rewriting every
// call site.
//
// One rule callers must keep: every field added to `GraphDef` from here on
// is `#[serde(default)]`. `local_storage::load` collapses any deserialize
// failure to `None`, and `None` reads as "this scope has no graphs", so a
// single non-defaulted new field silently deletes every graph its owner
// ever saved.
use crate::components::GraphDef;
use crate::local_storage;
use uuid::Uuid;

/// Who a set of graphs belongs to. Doubles as the fetch scope in
/// `components::graph_widget` -- a graph saved against a pigeon reads that
/// pigeon's history and a graph saved against a flock reads the flock's, so
/// the two are one fact, not two that happen to agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphScope {
  Pigeon(String),
  Flock(Uuid),
}

impl GraphScope {
  /// The scope pair flattened into one namespaced, versioned key. The
  /// literal shape predates this module and is kept exactly -- changing it
  /// would orphan every graph anyone has already saved.
  fn storage_key(&self) -> String {
    match self {
      GraphScope::Pigeon(id) => format!("pidgeiot.graphs.v1.pigeon.{id}"),
      GraphScope::Flock(id) => format!("pidgeiot.graphs.v1.flock.{id}"),
    }
  }
}

pub fn load(scope: &GraphScope) -> Vec<GraphDef> {
  local_storage::load(&scope.storage_key()).unwrap_or_default()
}

pub fn save(scope: &GraphScope, graphs: &[GraphDef]) {
  local_storage::save(&scope.storage_key(), &graphs);
}

#[cfg(test)]
mod tests {
  use super::GraphScope;
  use uuid::Uuid;

  /// Pins the key format against the graphs users have already saved: a
  /// change here is a silent data loss, not a refactor.
  #[test]
  fn storage_keys_match_the_shipped_v1_format() {
    assert_eq!(
      GraphScope::Pigeon("abc123".to_string()).storage_key(),
      "pidgeiot.graphs.v1.pigeon.abc123"
    );
    let flock = Uuid::parse_str("2f1a3b4c-5d6e-7f80-9012-3456789abcde").unwrap();
    assert_eq!(
      GraphScope::Flock(flock).storage_key(),
      "pidgeiot.graphs.v1.flock.2f1a3b4c-5d6e-7f80-9012-3456789abcde"
    );
  }
}
