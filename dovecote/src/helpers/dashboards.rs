use std::sync::atomic::{AtomicBool, Ordering};

use capsules::{
  Dashboard, DashboardLayout, DashboardRow, DashboardSummary, MAX_DASHBOARD_LAYOUT_BYTES,
  MAX_DASHBOARD_NAME_CHARS, MAX_DASHBOARDS_PER_OWNER, OrgRole, validate_layout,
};
use tokio_postgres::{Client, Row, types::Type};
use uuid::Uuid;
use worker::{Error, Result, console_error};

use crate::helpers::missing_schema_objects;
use crate::helpers::orgs::{Principal, ensure_org_tables};

/// Every column a `Dashboard` is built from; `layout` as `::text` because this workspace's
/// `tokio-postgres` has no serde_json feature.
const DASHBOARD_COLUMNS: &str =
  "id, user_id, org_id, created_by, name, layout::text AS layout, revision, created_at, updated_at";

/// Set once this isolate has found `dashboards` and its indexes.
static TABLE_READY: AtomicBool = AtomicBool::new(false);

/// Ensures `dashboards` once per isolate, logging it if missing: the migration
/// `infra/migrations/2026-11-07-dashboards.sql` should already have created it.
pub async fn ensure_dashboards_table(client: &Client) -> Result<()> {
  if TABLE_READY.load(Ordering::Relaxed) {
    return Ok(());
  }
  ensure_org_tables(client).await?;
  let missing = missing_schema_objects(
    client,
    &["dashboards", "idx_dashboards_user", "idx_dashboards_org"],
    &[],
  )
  .await?;
  if missing.is_empty() {
    TABLE_READY.store(true, Ordering::Relaxed);
    return Ok(());
  }
  console_error!(
    "ensure_dashboards_table: creating missing {}",
    missing.join(", ")
  );
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS dashboards (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        user_id UUID,
        org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
        created_by UUID,
        name TEXT NOT NULL,
        layout JSONB NOT NULL,
        revision INTEGER NOT NULL DEFAULT 1,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        CHECK ((user_id IS NULL) <> (org_id IS NULL))
      );
      CREATE INDEX IF NOT EXISTS idx_dashboards_user ON dashboards(user_id)
        WHERE user_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS idx_dashboards_org ON dashboards(org_id)
        WHERE org_id IS NOT NULL;",
    )
    .await
    .map_err(|e| {
      console_error!("ensure_dashboards_table failed: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  TABLE_READY.store(true, Ordering::Relaxed);
  Ok(())
}

/// Who a dashboard belongs to: exactly one person or one organization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DashboardOwner {
  Personal(Uuid),
  Org(Uuid),
}

impl DashboardOwner {
  fn columns(self) -> (Option<Uuid>, Option<Uuid>) {
    match self {
      DashboardOwner::Personal(user) => (Some(user), None),
      DashboardOwner::Org(org) => (None, Some(org)),
    }
  }
}

/// What the caller may do with one dashboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DashboardAccess {
  None,
  View,
  Edit,
}

/// A personal dashboard is its owner's alone. An organization's is viewed by every member and
/// edited by owners and admins, the same split as a flock's view and manage.
pub fn dashboard_access(
  user_id: Option<Uuid>,
  org_id: Option<Uuid>,
  caller: &Uuid,
  role_in_org: Option<OrgRole>,
) -> DashboardAccess {
  match (user_id, org_id) {
    (Some(owner), None) if owner == *caller => DashboardAccess::Edit,
    (None, Some(_)) => match role_in_org {
      Some(role) if role.is_manager() => DashboardAccess::Edit,
      Some(_) => DashboardAccess::View,
      None => DashboardAccess::None,
    },
    _ => DashboardAccess::None,
  }
}

/// [`dashboard_access`] for the signed-in caller, with `can_edit` filled in on the way out.
pub fn resolve_dashboard_access(
  principal: &Principal,
  caller: &Uuid,
  mut dashboard: Dashboard,
) -> (Dashboard, DashboardAccess) {
  let role = dashboard.org_id.and_then(|org| principal.org_role(&org));
  let access = dashboard_access(dashboard.user_id, dashboard.org_id, caller, role);
  dashboard.can_edit = access == DashboardAccess::Edit;
  (dashboard, access)
}

/// A validated layout serialized for storage, or the message and status that refuse it. The
/// size cap is on what is stored, not on what was sent.
pub fn stored_layout(layout: &DashboardLayout) -> std::result::Result<String, (&'static str, u16)> {
  if let Err(rejection) = validate_layout(layout) {
    return Err((rejection.message(), 400));
  }
  let Ok(json) = serde_json::to_string(layout) else {
    return Err(("Internal Server Error", 500));
  };
  if json.len() > MAX_DASHBOARD_LAYOUT_BYTES {
    return Err((
      "Payload Too Large: a dashboard layout is at most 32 KiB",
      413,
    ));
  }
  Ok(json)
}

/// The name a copy takes: the source's with " (copy)", trimmed to fit the name cap.
pub fn copy_name(source: &str) -> String {
  const SUFFIX: &str = " (copy)";
  let keep = MAX_DASHBOARD_NAME_CHARS - SUFFIX.chars().count();
  let mut name = String::with_capacity(source.len() + SUFFIX.len());
  name.extend(source.chars().take(keep));
  let trimmed = name.trim_end().len();
  name.truncate(trimmed);
  name.push_str(SUFFIX);
  name
}

fn row_to_dashboard(row: &Row) -> Dashboard {
  DashboardRow {
    id: row.get("id"),
    user_id: row.get("user_id"),
    org_id: row.get("org_id"),
    created_by: row.get("created_by"),
    name: row.get("name"),
    layout: row.get("layout"),
    revision: row.get("revision"),
    created_at: row.get("created_at"),
    updated_at: row.get("updated_at"),
  }
  .into()
}

fn db_error(what: &str, e: tokio_postgres::Error) -> Error {
  console_error!("Dashboards {what} error: {e}");
  Error::RustError("Internal Server Error".into())
}

/// The caller's personal dashboards and every dashboard of an organization in `org_ids`, newest
/// first, without layouts. `can_edit` is left for the caller to fill from the role it holds.
///
/// `read_at` keeps this out of Hyperdrive's query cache, which would otherwise hide a dashboard
/// created seconds ago from the list for up to a minute.
pub async fn list_dashboards(
  client: &Client,
  user_id: &Uuid,
  org_ids: &[Uuid],
) -> Result<Vec<DashboardSummary>> {
  ensure_dashboards_table(client).await?;
  let rows = client
    .query_typed(
      "SELECT id, user_id, org_id, name, revision, updated_at,
         CASE jsonb_typeof(layout->'widgets') WHEN 'array'
           THEN jsonb_array_length(layout->'widgets') ELSE 0 END::int4 AS widget_count,
         now() AS read_at
       FROM dashboards
       WHERE user_id = $1 OR org_id = ANY($2)
       ORDER BY updated_at DESC, id;",
      &[(user_id, Type::UUID), (&org_ids, Type::UUID_ARRAY)],
    )
    .await
    .map_err(|e| db_error("list", e))?;

  Ok(
    rows
      .iter()
      .map(|row| DashboardSummary {
        id: row.get("id"),
        user_id: row.get("user_id"),
        org_id: row.get("org_id"),
        name: row.get("name"),
        widget_count: row.get("widget_count"),
        revision: row.get("revision"),
        updated_at: row.get("updated_at"),
        can_edit: false,
      })
      .collect(),
  )
}

/// One dashboard by id, uncached for the same reason as [`list_dashboards`]: a member opening
/// an org dashboard must see the layout an admin saved seconds ago.
pub async fn load_dashboard(client: &Client, id: &Uuid) -> Result<Option<Dashboard>> {
  ensure_dashboards_table(client).await?;
  let mut sql = String::with_capacity(192);
  sql.push_str("SELECT ");
  sql.push_str(DASHBOARD_COLUMNS);
  sql.push_str(", now() AS read_at FROM dashboards WHERE id = $1;");
  let rows = client
    .query_typed(&sql, &[(id, Type::UUID)])
    .await
    .map_err(|e| db_error("read", e))?;
  Ok(rows.first().map(row_to_dashboard))
}

/// Stores a new dashboard and returns it. `Ok(None)` means the owner already holds
/// [`MAX_DASHBOARDS_PER_OWNER`]; the count is taken inside the insert so a cached count cannot
/// admit a minute's worth past the cap.
pub async fn create_dashboard(
  client: &Client,
  owner: DashboardOwner,
  created_by: &Uuid,
  name: &str,
  layout_json: &str,
) -> Result<Option<Dashboard>> {
  ensure_dashboards_table(client).await?;
  let (user_id, org_id) = owner.columns();
  let mut sql = String::with_capacity(512);
  sql.push_str(
    "INSERT INTO dashboards (user_id, org_id, created_by, name, layout)
     SELECT $1, $2, $3, $4, $5::jsonb
     WHERE (SELECT count(*) FROM dashboards
            WHERE user_id = $1 OR org_id = $2) < $6
     RETURNING ",
  );
  sql.push_str(DASHBOARD_COLUMNS);
  sql.push(';');
  let rows = client
    .query_typed(
      &sql,
      &[
        (&user_id, Type::UUID),
        (&org_id, Type::UUID),
        (created_by, Type::UUID),
        (&name, Type::TEXT),
        (&layout_json, Type::TEXT),
        (&MAX_DASHBOARDS_PER_OWNER, Type::INT8),
      ],
    )
    .await
    .map_err(|e| db_error("create", e))?;
  Ok(rows.first().map(row_to_dashboard))
}

/// Replaces name and layout if `revision` is still the stored one, bumping it. `Ok(None)` means
/// another save got there first (or the dashboard is gone); the caller re-reads to tell which.
pub async fn update_dashboard(
  client: &Client,
  id: &Uuid,
  revision: i32,
  name: &str,
  layout_json: &str,
) -> Result<Option<Dashboard>> {
  ensure_dashboards_table(client).await?;
  let mut sql = String::with_capacity(320);
  sql.push_str(
    "UPDATE dashboards SET name = $3, layout = $4::jsonb, revision = revision + 1,
       updated_at = now()
     WHERE id = $1 AND revision = $2
     RETURNING ",
  );
  sql.push_str(DASHBOARD_COLUMNS);
  sql.push(';');
  let rows = client
    .query_typed(
      &sql,
      &[
        (id, Type::UUID),
        (&revision, Type::INT4),
        (&name, Type::TEXT),
        (&layout_json, Type::TEXT),
      ],
    )
    .await
    .map_err(|e| db_error("update", e))?;
  Ok(rows.first().map(row_to_dashboard))
}

/// Deletes one dashboard. Deleting one already gone is not an error.
pub async fn delete_dashboard(client: &Client, id: &Uuid) -> Result<()> {
  ensure_dashboards_table(client).await?;
  client
    .execute_typed("DELETE FROM dashboards WHERE id = $1;", &[(id, Type::UUID)])
    .await
    .map_err(|e| db_error("delete", e))?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
  }

  #[test]
  fn a_copy_is_named_for_its_source_and_still_fits() {
    assert_eq!(copy_name("Depot 3"), "Depot 3 (copy)");
    let long = copy_name(&"x".repeat(MAX_DASHBOARD_NAME_CHARS));
    assert_eq!(long.chars().count(), MAX_DASHBOARD_NAME_CHARS);
    assert!(long.ends_with(" (copy)"));
    assert!(capsules::normalize_dashboard_name(&long).is_ok());
  }

  #[test]
  fn a_layout_is_validated_and_capped_before_storage() {
    let text = |id: String, chars: usize| capsules::Widget {
      id,
      size: capsules::WidgetSize::Full,
      title: String::new(),
      body: capsules::WidgetBody::Text {
        text: "\u{1F426}".repeat(chars),
      },
    };
    let mut layout = DashboardLayout::default();
    layout.widgets.push(text("a".into(), 10));
    assert!(stored_layout(&layout).is_ok());

    layout.widgets.push(capsules::Widget {
      body: capsules::WidgetBody::Unknown,
      ..text("b".into(), 0)
    });
    assert_eq!(stored_layout(&layout).unwrap_err().1, 400);

    // Each text widget is within its own cap; together they pass the layout's.
    let big = DashboardLayout {
      widgets: (0..5).map(|i| text(i.to_string(), 2_000)).collect(),
      ..DashboardLayout::default()
    };
    assert_eq!(stored_layout(&big).unwrap_err().1, 413);
  }

  #[test]
  fn a_personal_dashboard_is_its_owners_alone() {
    assert_eq!(
      dashboard_access(Some(id(1)), None, &id(1), None),
      DashboardAccess::Edit
    );
    assert_eq!(
      dashboard_access(Some(id(1)), None, &id(2), Some(OrgRole::Owner)),
      DashboardAccess::None
    );
  }

  #[test]
  fn org_managers_edit_and_members_view() {
    let org = Some(id(9));
    assert_eq!(
      dashboard_access(None, org, &id(2), Some(OrgRole::Owner)),
      DashboardAccess::Edit
    );
    assert_eq!(
      dashboard_access(None, org, &id(2), Some(OrgRole::Admin)),
      DashboardAccess::Edit
    );
    assert_eq!(
      dashboard_access(None, org, &id(2), Some(OrgRole::Member)),
      DashboardAccess::View
    );
    assert_eq!(
      dashboard_access(None, org, &id(2), None),
      DashboardAccess::None
    );
  }

  #[test]
  fn a_row_with_both_or_neither_owner_grants_nothing() {
    assert_eq!(
      dashboard_access(Some(id(1)), Some(id(9)), &id(1), Some(OrgRole::Owner)),
      DashboardAccess::None
    );
    assert_eq!(
      dashboard_access(None, None, &id(1), None),
      DashboardAccess::None
    );
  }
}
