// Where a user's saved graphs live: one document per scope in the
// account's own dashboard state (`api::dashboard_state`), mirrored into
// this browser's localStorage. Tying them to the account rather than the
// browser is what makes them survive a browser that clears site data on
// close.
//
// The server is authoritative, but the mirror is not merely a cache:
// Hyperdrive serves an identical SELECT from its query cache for up to a
// minute, so a reload straight after a save can still be answered with the
// document this browser has already replaced. Both copies carry the
// server's own `updated_at`, so the newer one wins and no clock but the
// server's is ever compared. The mirror also covers a failed read, which
// would otherwise render as "no graphs".
//
// One rule callers must keep: every field added to `GraphDef` from here on
// is `#[serde(default)]`. A deserialize failure collapses to no graphs, so
// a single non-defaulted new field silently deletes every graph its owner
// ever saved.
use crate::api::dashboard_state::{self, StateRead};
use crate::components::GraphDef;
use crate::local_storage;
use capsules::DashboardStateEntry;
use dioxus::logger::tracing::error;
use dioxus::prelude::spawn;
use uuid::Uuid;

const PIGEON_PREFIX: &str = "graphs.v1.pigeon.";
const FLOCK_PREFIX: &str = "graphs.v1.flock.";

/// localStorage is shared with everything else on the origin, so the
/// mirror's key needs a namespace the server's does not.
const MIRROR_PREFIX: &str = "pidgeiot.";

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
  /// The scope pair flattened into one namespaced, versioned key -- what
  /// `dashboard_state` stores this scope's document under.
  fn state_key(&self) -> String {
    match self {
      GraphScope::Pigeon(id) => {
        let mut key = String::with_capacity(PIGEON_PREFIX.len() + id.len());
        key.push_str(PIGEON_PREFIX);
        key.push_str(id);
        key
      }
      GraphScope::Flock(id) => {
        let mut buf = Uuid::encode_buffer();
        let id = id.hyphenated().encode_lower(&mut buf);
        let mut key = String::with_capacity(FLOCK_PREFIX.len() + id.len());
        key.push_str(FLOCK_PREFIX);
        key.push_str(id);
        key
      }
    }
  }

  /// The mirror's own key. The literal shape predates the server store and
  /// is kept exactly: it is also where graphs saved before the move are
  /// found, and changing it would orphan them.
  fn storage_key(&self) -> String {
    let state_key = self.state_key();
    let mut key = String::with_capacity(MIRROR_PREFIX.len() + state_key.len());
    key.push_str(MIRROR_PREFIX);
    key.push_str(&state_key);
    key
  }
}

/// The graphs inside a stored document. An unreadable document reads as no
/// graphs rather than an error -- see the field rule in this module's
/// header.
fn graphs_of(entry: DashboardStateEntry) -> Vec<GraphDef> {
  serde_json::from_str(&entry.value.into_inner()).unwrap_or_default()
}

/// Graphs saved before the store moved server-side: the same key holding a
/// bare array instead of an entry. Read so those graphs survive the move;
/// nothing writes this shape any more.
fn legacy_graphs(scope: &GraphScope) -> Vec<GraphDef> {
  local_storage::load(&scope.storage_key()).unwrap_or_default()
}

pub async fn load(scope: &GraphScope) -> Vec<GraphDef> {
  let local: Option<DashboardStateEntry> = local_storage::load(&scope.storage_key());

  match dashboard_state::get(&scope.state_key()).await {
    StateRead::Found(entry) => match local {
      Some(local) if local.updated_at > entry.updated_at => graphs_of(local),
      _ => {
        local_storage::save(&scope.storage_key(), &entry);
        graphs_of(entry)
      }
    },
    // Either the account has never saved this scope, or the cached 404
    // predates a save this browser made. Pushing what we hold covers both,
    // and is how graphs saved before this store existed move up.
    StateRead::Missing => {
      let graphs = local.map_or_else(|| legacy_graphs(scope), graphs_of);
      if !graphs.is_empty() {
        save(scope, &graphs);
      }
      graphs
    }
    // No timestamp to compare and no right to push over a document we
    // could not read.
    StateRead::Unavailable => local.map_or_else(|| legacy_graphs(scope), graphs_of),
  }
}

/// Persists a scope's graphs, replacing whatever was stored.
///
/// Fire-and-forget: the caller is an event handler and the signal it just
/// wrote is already what the user sees. The mirror is updated from the
/// response and only from the response, so a failed save leaves this
/// browser's copy where the server's is rather than inventing a timestamp.
pub fn save(scope: &GraphScope, graphs: &[GraphDef]) {
  let state_key = scope.state_key();
  let Ok(value) = serde_json::to_string(graphs) else {
    error!("Failed to serialize graphs for {state_key}");
    return;
  };
  let storage_key = scope.storage_key();

  spawn(async move {
    match dashboard_state::put(&state_key, &value).await {
      Some(entry) => {
        local_storage::save(&storage_key, &entry);
      }
      None => error!("Failed to save graphs for {state_key}"),
    }
  });
}

#[cfg(test)]
mod tests {
  use super::GraphScope;
  use uuid::Uuid;

  /// Pins both key formats against the graphs users have already saved: a
  /// change to either is a silent data loss, not a refactor.
  #[test]
  fn scope_keys_match_the_shipped_v1_format() {
    let pigeon = GraphScope::Pigeon("abc123".to_string());
    assert_eq!(pigeon.state_key(), "graphs.v1.pigeon.abc123");
    assert_eq!(pigeon.storage_key(), "pidgeiot.graphs.v1.pigeon.abc123");

    let flock = GraphScope::Flock(Uuid::parse_str("2f1a3b4c-5d6e-7f80-9012-3456789abcde").unwrap());
    assert_eq!(
      flock.state_key(),
      "graphs.v1.flock.2f1a3b4c-5d6e-7f80-9012-3456789abcde"
    );
    assert_eq!(
      flock.storage_key(),
      "pidgeiot.graphs.v1.flock.2f1a3b4c-5d6e-7f80-9012-3456789abcde"
    );
  }

  /// The server refuses a key it cannot carry in a path segment, so a
  /// scope that mints one would fail every save.
  #[test]
  fn every_minted_key_is_one_the_store_accepts() {
    assert!(capsules::valid_scope_key(
      &GraphScope::Pigeon("a".repeat(64)).state_key()
    ));
    assert!(capsules::valid_scope_key(
      &GraphScope::Flock(Uuid::now_v7()).state_key()
    ));
  }
}
