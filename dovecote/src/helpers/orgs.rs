//! Organizations + org RBAC -- Postgres-side helpers.
//!
//! Model recap (see `docs/api.md`'s "Organizations" section for the wire
//! surface and the full permission matrix): an `organizations` row per
//! team, `organization_members` rows carrying each member's role
//! (owner|admin|member), app-level invites in `organization_invites`
//! (token HASHES only -- self-hosted Kratos has no B2B invite flow), and a
//! nullable `flocks.org_id` making a flock EXACTLY one of user-owned or
//! org-owned. Org-management authorization (rename, invites, member
//! management) funnels through [`org_role_of`] + the route-level rules
//! documented on each function; flock-level authorization funnels through
//! [`authorize_flock`], the single gateway-side authz helper (its DO-side
//! counterpart is `objects/pigeons.rs::authorize_dashboard`).

use capsules::{
  Flock, InviteEmail, OrgRole, OrgRoleEntry, Organization, OrganizationInvite, OrganizationMember,
  OrganizationMembership, format_invite_email,
};
use time::OffsetDateTime;
use tokio_postgres::{Client, Row, types::Type};
use uuid::Uuid;
use worker::{Env, Error, Result, console_error, console_log};

use crate::helpers::firmware::FlockAccess;
use crate::helpers::sha256_hex;

/// Invite lifetime -- short by design (token-alone acceptance, see
/// `docs/api.md`): a leaked invite link goes stale on its own instead of
/// living as long as the row does.
const INVITE_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// The caller's full identity for authorization purposes: the validated
/// Kratos user id plus the org-membership set (`{user_id} ∪ {org ids with
/// roles}`), loaded ONCE per authenticated request by
/// `require_principal` (`lib.rs`) via [`load_org_roles`]. Forwarded to
/// Durable Objects as `X-User-Id` + `X-Org-Roles` (see
/// [`Principal::org_roles_header`]) so the DO-side ACL check can match
/// org-granted rows without a Postgres round trip of its own.
pub struct Principal {
  pub user_id: String,
  pub email: Option<String>,
  /// See `AuthSession::verified_emails` (`lib.rs`) -- carried through so
  /// principal-consuming routes (alerts) can validate channel overrides
  /// without a second session resolve.
  pub verified_emails: Vec<String>,
  pub org_roles: Vec<OrgRoleEntry>,
  /// Precomputed at construction so per-route proxying is a borrow, not a
  /// re-serialization.
  org_roles_json: Option<String>,
}

impl Principal {
  pub fn new(
    user_id: String,
    email: Option<String>,
    verified_emails: Vec<String>,
    org_roles: Vec<OrgRoleEntry>,
  ) -> Self {
    let org_roles_json = if org_roles.is_empty() {
      None
    } else {
      serde_json::to_string(&org_roles).ok()
    };
    Self {
      user_id,
      email,
      verified_emails,
      org_roles,
      org_roles_json,
    }
  }

  /// The caller's role in `org_id`, if a member.
  pub fn org_role(&self, org_id: &Uuid) -> Option<OrgRole> {
    self
      .org_roles
      .iter()
      .find(|e| &e.id == org_id)
      .map(|e| e.role)
  }

  /// Compact JSON for the internal `X-Org-Roles` gateway->DO header
  /// (`[{"id":"<uuid>","role":"owner"}]`) -- `None` when the caller
  /// belongs to no orgs, so the common case adds no header at all.
  pub fn org_roles_header(&self) -> Option<&str> {
    self.org_roles_json.as_deref()
  }
}

/// Idempotently ensures the org tables + `flocks.org_id` exist -- mirrors
/// `ensure_alert_tables`/`ensure_flock_firmware_table`'s rationale (no
/// separate migration runner against the shared staging/prod database).
/// The identical statements live in
/// `infra/migrations/2026-08-08-organizations.sql` for explicit
/// deploy-time application; this is the runtime belt-and-suspenders.
pub async fn ensure_org_tables(client: &Client) -> Result<()> {
  client
    .batch_execute(
      "CREATE TABLE IF NOT EXISTS organizations (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        name TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
      );
      CREATE TABLE IF NOT EXISTS organization_members (
        org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
        user_id UUID NOT NULL,
        role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
        email TEXT,
        invited_by UUID,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (org_id, user_id)
      );
      CREATE TABLE IF NOT EXISTS organization_invites (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
        email TEXT NOT NULL,
        role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
        token_hash TEXT NOT NULL UNIQUE,
        expires_at TIMESTAMPTZ NOT NULL,
        created_by UUID NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        accepted_at TIMESTAMPTZ
      );
      ALTER TABLE flocks ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES organizations(id) ON DELETE SET NULL;
      CREATE INDEX IF NOT EXISTS idx_flocks_org_id ON flocks(org_id) WHERE org_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS idx_organization_members_user_id ON organization_members(user_id);
      CREATE INDEX IF NOT EXISTS idx_organization_invites_org_id ON organization_invites(org_id);",
    )
    .await
    .map_err(|e| {
      console_error!("Org tables bootstrap error: {e}");
      Error::RustError("Internal Server Error".into())
    })
}

fn parse_uuid(value: &str, what: &str) -> Result<Uuid> {
  Uuid::parse_str(value).map_err(|e| Error::RustError(format!("Invalid {what} format: {e}")))
}

fn parse_role(raw: String) -> OrgRole {
  // The CHECK constraint guarantees one of the three values -- fall back
  // to the least-privileged role rather than erroring on a hand-edited
  // row, matching capsules' permissive-on-malformed-stored-data
  // convention.
  raw.parse().unwrap_or(OrgRole::Member)
}

fn row_to_organization(row: &Row) -> Organization {
  Organization {
    id: row.get("id"),
    name: row.get("name"),
    created_at: row.get("created_at"),
    updated_at: row.get("updated_at"),
  }
}

fn row_to_member(row: &Row) -> OrganizationMember {
  OrganizationMember {
    org_id: row.get("org_id"),
    user_id: row.get("user_id"),
    role: parse_role(row.get("role")),
    email: row.get("email"),
    invited_by: row.get("invited_by"),
    created_at: row.get("created_at"),
  }
}

fn row_to_invite(row: &Row) -> OrganizationInvite {
  OrganizationInvite {
    id: row.get("id"),
    org_id: row.get("org_id"),
    email: row.get("email"),
    role: parse_role(row.get("role")),
    expires_at: row.get("expires_at"),
    created_by: row.get("created_by"),
    created_at: row.get("created_at"),
  }
}

/// The caller's role in one org, if any -- THE org-management authorization
/// primitive every `/orgs/*` route funnels through (each route then applies
/// its own minimum-role rule, documented in `docs/api.md`'s matrix).
///
/// Carries no STABLE or VOLATILE function, so Hyperdrive caches it for
/// about a minute: a membership added, removed or re-roled reaches this
/// check within roughly 75 seconds rather than on the very next request.
/// That is the deliberate trade. Anchoring the statement on `now()` would
/// keep it out of the cache and buy exactness at the price of a database
/// round trip on every authorized organization call, on a surface where
/// the stale answer is the one the caller already had.
pub async fn org_role_of(
  client: &Client,
  org_id: &Uuid,
  user_id_str: &str,
) -> Result<Option<OrgRole>> {
  ensure_org_tables(client).await?;
  let user_uuid = parse_uuid(user_id_str, "X-User-Id")?;

  let rows = client
    .query_typed(
      "SELECT role FROM organization_members WHERE org_id = $1 AND user_id = $2;",
      &[(org_id, Type::UUID), (&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org role lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.into_iter().next().map(|r| parse_role(r.get("role"))))
}

/// The caller's full org-membership set in ONE query -- the principal-set
/// load `require_principal` (`lib.rs`) runs once per authenticated request
/// so the DO's ACL check (`X-Org-Roles`) and the gateway's flock check can
/// both consult it without further round trips. Cached by Hyperdrive on
/// the same terms as [`org_role_of`], so a membership change reaches the
/// principal set on the same ~75 s delay.
pub async fn load_org_roles(client: &Client, user_id_str: &str) -> Result<Vec<OrgRoleEntry>> {
  let user_uuid = parse_uuid(user_id_str, "X-User-Id")?;

  let rows = client
    .query_typed(
      "SELECT org_id, role FROM organization_members WHERE user_id = $1;",
      &[(&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org membership load error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .into_iter()
      .map(|r| OrgRoleEntry {
        id: r.get("org_id"),
        role: parse_role(r.get("role")),
      })
      .collect(),
  )
}

/// Creates an org and its founding `owner` membership row in one
/// transaction -- an org can never exist without at least one owner.
/// Borrows the client mutably rather than consuming it (the transaction
/// needs `&mut`) so the caller still holds it afterwards -- `POST /orgs`
/// writes the new org's business details on the same connection.
pub async fn create_organization(
  client: &mut Client,
  user_id_str: &str,
  email: Option<&str>,
  name: &str,
) -> Result<Organization> {
  ensure_org_tables(client).await?;
  let user_uuid = parse_uuid(user_id_str, "X-User-Id")?;

  let tx = client.transaction().await.map_err(|e| {
    console_error!("Org create transaction error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let row = tx
    .query_typed_one(
      "INSERT INTO organizations (name) VALUES ($1)
       RETURNING id, name, created_at, updated_at;",
      &[(&name, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Org insert error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  let org = row_to_organization(&row);

  tx.execute_typed(
    "INSERT INTO organization_members (org_id, user_id, role, email, invited_by)
     VALUES ($1, $2, 'owner', $3, NULL);",
    &[
      (&org.id, Type::UUID),
      (&user_uuid, Type::UUID),
      (&email, Type::TEXT),
    ],
  )
  .await
  .map_err(|e| {
    console_error!("Org founding-owner insert error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  tx.commit().await.map_err(|e| {
    console_error!("Org create commit error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  Ok(org)
}

/// Every org the caller belongs to, with the caller's own role.
pub async fn list_user_organizations(
  client: &Client,
  user_id_str: &str,
) -> Result<Vec<OrganizationMembership>> {
  ensure_org_tables(client).await?;
  let user_uuid = parse_uuid(user_id_str, "X-User-Id")?;

  let rows = client
    .query_typed(
      "SELECT o.id, o.name, o.created_at, o.updated_at, m.role
       FROM organizations o
       JOIN organization_members m ON m.org_id = o.id
       WHERE m.user_id = $1
       ORDER BY o.created_at ASC;",
      &[(&user_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org list error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(
    rows
      .into_iter()
      .map(|r| OrganizationMembership {
        organization: row_to_organization(&r),
        role: parse_role(r.get("role")),
      })
      .collect(),
  )
}

pub async fn get_organization(client: &Client, org_id: &Uuid) -> Result<Option<Organization>> {
  let rows = client
    .query_typed(
      "SELECT id, name, created_at, updated_at FROM organizations WHERE id = $1;",
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org get error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(rows.first().map(row_to_organization))
}

pub async fn list_org_members(client: &Client, org_id: &Uuid) -> Result<Vec<OrganizationMember>> {
  let rows = client
    .query_typed(
      "SELECT org_id, user_id, role, email, invited_by, created_at
       FROM organization_members WHERE org_id = $1 ORDER BY created_at ASC;",
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org member list error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(rows.iter().map(row_to_member).collect())
}

/// Pending (unconsumed, unexpired) invites only -- consumed/expired rows
/// are history, not actionable state.
pub async fn list_org_invites(client: &Client, org_id: &Uuid) -> Result<Vec<OrganizationInvite>> {
  let rows = client
    .query_typed(
      "SELECT id, org_id, email, role, expires_at, created_by, created_at
       FROM organization_invites
       WHERE org_id = $1 AND accepted_at IS NULL AND expires_at > now()
       ORDER BY created_at ASC;",
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org invite list error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(rows.iter().map(row_to_invite).collect())
}

pub async fn rename_organization(
  client: &Client,
  org_id: &Uuid,
  name: &str,
) -> Result<Organization> {
  let row = client
    .query_typed_one(
      "UPDATE organizations SET name = $2, updated_at = now() WHERE id = $1
       RETURNING id, name, created_at, updated_at;",
      &[(org_id, Type::UUID), (&name, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Org rename error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(row_to_organization(&row))
}

/// Deletes an org only when it owns no flocks ("delete only when empty").
/// Membership + invite rows cascade. `Err(&str)` in the inner result is the
/// user-facing 409 message.
pub async fn delete_organization_if_empty(
  mut client: Client,
  org_id: &Uuid,
) -> Result<std::result::Result<(), &'static str>> {
  let tx = client.transaction().await.map_err(|e| {
    console_error!("Org delete transaction error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let row = tx
    .query_typed_one(
      "SELECT COUNT(*)::BIGINT AS flock_count FROM flocks WHERE org_id = $1;",
      &[(org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Org emptiness check error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  if row.get::<_, i64>("flock_count") > 0 {
    return Ok(Err(
      "Conflict: organization still owns flocks -- transfer or delete them first",
    ));
  }

  tx.execute_typed(
    "DELETE FROM organizations WHERE id = $1;",
    &[(org_id, Type::UUID)],
  )
  .await
  .map_err(|e| {
    console_error!("Org delete error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  tx.commit().await.map_err(|e| {
    console_error!("Org delete commit error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  Ok(Ok(()))
}

/// Changes a member's role, enforcing last-owner protection: an org must
/// always retain at least one `owner`, so demoting the only owner is
/// refused. Returns the updated member row, or a user-facing conflict
/// message. Caller (the route) has already enforced WHO may change roles
/// (org owners only -- see `docs/api.md`).
pub async fn change_member_role(
  mut client: Client,
  org_id: &Uuid,
  target_user_id: &Uuid,
  new_role: OrgRole,
) -> Result<std::result::Result<OrganizationMember, &'static str>> {
  let tx = client.transaction().await.map_err(|e| {
    console_error!("Member role transaction error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let rows = tx
    .query_typed(
      "SELECT role FROM organization_members
       WHERE org_id = $1 AND user_id = $2 FOR UPDATE;",
      &[(org_id, Type::UUID), (target_user_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Member role read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(current) = rows.into_iter().next().map(|r| parse_role(r.get("role"))) else {
    return Ok(Err("Not Found: no such member in this organization"));
  };

  if current == OrgRole::Owner && new_role != OrgRole::Owner {
    let owners = tx
      .query_typed_one(
        "SELECT COUNT(*)::BIGINT AS owner_count FROM organization_members
         WHERE org_id = $1 AND role = 'owner';",
        &[(org_id, Type::UUID)],
      )
      .await
      .map_err(|e| {
        console_error!("Owner count error: {e}");
        Error::RustError("Internal Server Error".into())
      })?;
    if owners.get::<_, i64>("owner_count") <= 1 {
      return Ok(Err(
        "Conflict: an organization must retain at least one owner",
      ));
    }
  }

  let row = tx
    .query_typed_one(
      "UPDATE organization_members SET role = $3
       WHERE org_id = $1 AND user_id = $2
       RETURNING org_id, user_id, role, email, invited_by, created_at;",
      &[
        (org_id, Type::UUID),
        (target_user_id, Type::UUID),
        (&new_role.as_str(), Type::TEXT),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Member role update error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  let member = row_to_member(&row);

  tx.commit().await.map_err(|e| {
    console_error!("Member role commit error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  Ok(Ok(member))
}

/// Removes a membership row -- THE revocation mechanism (a removed member
/// loses every org-granted flock/pigeon right without any ACL row being
/// rewritten, since the principal set is loaded per request). It is not
/// instant: see [`org_role_of`] for the window. Last-owner protection
/// applies here too. Caller (the route) has already enforced WHO may
/// remove (owner/admin, admins never removing owners -- see
/// `docs/api.md`).
pub async fn remove_member(
  mut client: Client,
  org_id: &Uuid,
  target_user_id: &Uuid,
) -> Result<std::result::Result<(), &'static str>> {
  let tx = client.transaction().await.map_err(|e| {
    console_error!("Member remove transaction error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let rows = tx
    .query_typed(
      "SELECT role FROM organization_members
       WHERE org_id = $1 AND user_id = $2 FOR UPDATE;",
      &[(org_id, Type::UUID), (target_user_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Member remove read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(current) = rows.into_iter().next().map(|r| parse_role(r.get("role"))) else {
    return Ok(Err("Not Found: no such member in this organization"));
  };

  if current == OrgRole::Owner {
    let owners = tx
      .query_typed_one(
        "SELECT COUNT(*)::BIGINT AS owner_count FROM organization_members
         WHERE org_id = $1 AND role = 'owner';",
        &[(org_id, Type::UUID)],
      )
      .await
      .map_err(|e| {
        console_error!("Owner count error: {e}");
        Error::RustError("Internal Server Error".into())
      })?;
    if owners.get::<_, i64>("owner_count") <= 1 {
      return Ok(Err(
        "Conflict: an organization must retain at least one owner",
      ));
    }
  }

  tx.execute_typed(
    "DELETE FROM organization_members WHERE org_id = $1 AND user_id = $2;",
    &[(org_id, Type::UUID), (target_user_id, Type::UUID)],
  )
  .await
  .map_err(|e| {
    console_error!("Member remove error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  tx.commit().await.map_err(|e| {
    console_error!("Member remove commit error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  Ok(Ok(()))
}

/// Mints a fresh invite token (32 random bytes, base64url) and returns
/// `(cleartext_token, sha256_hex_hash)` -- only the hash is ever persisted.
pub fn mint_invite_token() -> Result<(String, String)> {
  use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
  let mut bytes = [0u8; 32];
  getrandom::getrandom(&mut bytes).map_err(|e| Error::RustError(format!("RNG error: {e}")))?;
  let token = URL_SAFE_NO_PAD.encode(bytes);
  let hash = sha256_hex(token.as_bytes());
  Ok((token, hash))
}

pub async fn create_invite(
  client: &Client,
  org_id: &Uuid,
  email: &str,
  role: OrgRole,
  created_by_str: &str,
  token_hash: &str,
) -> Result<OrganizationInvite> {
  let created_by = parse_uuid(created_by_str, "X-User-Id")?;
  let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(INVITE_TTL_SECS);

  let row = client
    .query_typed_one(
      "INSERT INTO organization_invites (org_id, email, role, token_hash, expires_at, created_by)
       VALUES ($1, $2, $3, $4, $5, $6)
       RETURNING id, org_id, email, role, expires_at, created_by, created_at;",
      &[
        (org_id, Type::UUID),
        (&email, Type::TEXT),
        (&role.as_str(), Type::TEXT),
        (&token_hash, Type::TEXT),
        (&expires_at, Type::TIMESTAMPTZ),
        (&created_by, Type::UUID),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Invite insert error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(row_to_invite(&row))
}

/// Revokes (deletes) a pending invite. Idempotent -- revoking an
/// already-consumed/absent invite is a no-op success.
pub async fn revoke_invite(client: &Client, org_id: &Uuid, invite_id: &Uuid) -> Result<()> {
  client
    .execute_typed(
      "DELETE FROM organization_invites WHERE id = $1 AND org_id = $2;",
      &[(invite_id, Type::UUID), (org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Invite revoke error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(())
}

/// Consumes an invite token for the calling session: token-alone (bearer)
/// matching by design -- the accept route requires an authenticated Kratos
/// session but does NOT require that session's email to equal the invited
/// address, so an invitee may accept under whichever email they actually
/// registered with (tradeoff documented in `docs/api.md`; the compensating
/// controls are the short TTL, single-use consumption, and hash-only
/// storage). Runs in one transaction with `FOR UPDATE` so a token can
/// never be consumed twice concurrently.
///
/// Hands back the caller's new membership in the shape `GET /orgs` lists
/// it, org row included, so a client can add it to its own list without
/// re-reading one that Hyperdrive may still be serving from cache.
pub async fn accept_invite(
  mut client: Client,
  token: &str,
  user_id_str: &str,
  email: Option<&str>,
) -> Result<std::result::Result<OrganizationMembership, &'static str>> {
  ensure_org_tables(&client).await?;
  let user_uuid = parse_uuid(user_id_str, "X-User-Id")?;
  let token_hash = sha256_hex(token.as_bytes());

  let tx = client.transaction().await.map_err(|e| {
    console_error!("Invite accept transaction error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let rows = tx
    .query_typed(
      "SELECT id, org_id, role FROM organization_invites
       WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > now()
       FOR UPDATE;",
      &[(&token_hash, Type::TEXT)],
    )
    .await
    .map_err(|e| {
      console_error!("Invite lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(invite_row) = rows.into_iter().next() else {
    return Ok(Err(
      "Not Found: invite is invalid, expired, or already used",
    ));
  };
  let invite_id: Uuid = invite_row.get("id");
  let org_id: Uuid = invite_row.get("org_id");
  let role = parse_role(invite_row.get("role"));
  let inviter: Uuid = {
    // created_by re-read for invited_by attribution.
    let row = tx
      .query_typed_one(
        "SELECT created_by FROM organization_invites WHERE id = $1;",
        &[(&invite_id, Type::UUID)],
      )
      .await
      .map_err(|e| {
        console_error!("Invite creator read error: {e}");
        Error::RustError("Internal Server Error".into())
      })?;
    row.get("created_by")
  };

  let inserted = tx
    .query_typed(
      "INSERT INTO organization_members (org_id, user_id, role, email, invited_by)
       VALUES ($1, $2, $3, $4, $5)
       ON CONFLICT (org_id, user_id) DO NOTHING
       RETURNING org_id, user_id, role, email, invited_by, created_at;",
      &[
        (&org_id, Type::UUID),
        (&user_uuid, Type::UUID),
        (&role.as_str(), Type::TEXT),
        (&email, Type::TEXT),
        (&inviter, Type::UUID),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Invite membership insert error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  if inserted.is_empty() {
    // Already a member: leave the invite unconsumed (it may have been
    // meant for someone else) and tell the caller.
    return Ok(Err(
      "Conflict: you are already a member of this organization",
    ));
  }

  tx.execute_typed(
    "UPDATE organization_invites SET accepted_at = now() WHERE id = $1;",
    &[(&invite_id, Type::UUID)],
  )
  .await
  .map_err(|e| {
    console_error!("Invite consume error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  let org_row = tx
    .query_typed_one(
      "SELECT id, name, created_at, updated_at FROM organizations WHERE id = $1;",
      &[(&org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Invite org read error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  let organization = row_to_organization(&org_row);

  tx.commit().await.map_err(|e| {
    console_error!("Invite accept commit error: {e}");
    Error::RustError("Internal Server Error".into())
  })?;

  Ok(Ok(OrganizationMembership { organization, role }))
}

/// Builds the fancier-side accept URL for an invite token -- `ROOT_URL`-based
/// (the frontend's own origin per environment).
pub fn build_invite_url(env: &Env, token: &str) -> String {
  format!("{}/invite?token={token}", crate::helpers::root_url(env))
}

/// Last `n` characters of `s` (or the whole string if it's shorter),
/// prefixed with an ellipsis -- enough to correlate a log line against a
/// support report or a copy of the real value without retaining the value
/// itself now that `head_sampling_rate = 1` (`wrangler.toml`) keeps every
/// log line instead of sampling almost all of them away.
fn suffix_hint(s: &str, n: usize) -> String {
  let start = s.len().saturating_sub(n);
  // `s` is ASCII (tokens/URLs), so byte and char boundaries coincide --
  // no `floor_char_boundary` needed.
  format!("...{}", &s[start..])
}

/// Sends the invite email through the EXISTING Resend transport
/// (`helpers/alerts.rs::send_email_message` -- no new provider/secret). In
/// an environment with no `RESEND_API_KEY` configured (dev), this logs a
/// (redacted) stand-in for the invite link instead, keeping the flow
/// locally testable end-to-end: `wrangler dev`'s own admin/DB access can
/// recover the real token, so nothing testable is lost. `org_id` is for
/// logging only -- the email body addresses the org by name and the
/// inviter by the name and email address on their session, whichever of
/// the two the identity carries.
#[allow(clippy::too_many_arguments)]
pub async fn send_invite_email(
  env: &Env,
  to: &str,
  org_id: &Uuid,
  org_name: &str,
  inviter_name: Option<&str>,
  inviter_email: Option<&str>,
  role: OrgRole,
  invite_url: &str,
  expires_at: OffsetDateTime,
) {
  if !crate::helpers::alerts::usesend_configured(env) {
    // Never log `to` or the full `invite_url` -- the URL's query string IS
    // the live, single-use invite token (see `build_invite_url`), a
    // credential. Only its last 4 characters are shown.
    console_log!(
      "Org invite (email transport not configured -- dev no-op): org={org_id} org_name={org_name:?} link_token={}",
      suffix_hint(invite_url, 4)
    );
    return;
  }

  let message = format_invite_email(&InviteEmail {
    inviter_name,
    inviter_email,
    org_name,
    role,
    invite_url,
    expires_at,
    sent_at: OffsetDateTime::now_utc(),
  });

  if let Err(e) = crate::helpers::alerts::send_email_message(env, to, &message).await {
    console_error!("Org invite email send failed for org {org_id}: {e}");
  }
}

// --- Flock-level authorization (the single gateway-side authz helper) ---

/// The access level a gateway flock route requires. `View` is
/// read/telemetry-level; `Manage` is mutation/administration-level. See the
/// permission matrix in `docs/api.md`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlockAction {
  View,
  Manage,
}

/// THE gateway-side flock authorization helper -- every gateway flock
/// check funnels through here, so a future central-authz swap only has to
/// replace this one function; the DO-side counterpart is
/// `objects/pigeons.rs::authorize_dashboard`.
///
/// Rules -- a flock is EXACTLY one of user-owned or org-owned:
/// - `org_id IS NULL` (personal): only `flocks.user_id == caller` grants
///   anything, and it grants `Manage` (which implies `View`).
/// - `org_id` set (org-owned): only the caller's role IN THAT ORG grants
///   anything -- `owner`/`admin` grant `Manage`, `member` grants `View`.
///   `flocks.user_id` is provenance, not an access grant, once transferred.
///
/// Returns a `FlockAccess` proof (`Some` = allowed at the requested level)
/// in the same shape the downstream proof-requiring helpers
/// (`list_flock_firmware`, telemetry history, alerts) already expect.
pub async fn authorize_flock(
  client: &Client,
  flock_id_str: &str,
  principal: &Principal,
  action: FlockAction,
) -> Result<Option<FlockAccess>> {
  ensure_org_tables(client).await?;
  let flock_uuid = parse_uuid(flock_id_str, "flock_id")?;

  let rows = client
    .query_typed(
      "SELECT user_id, org_id FROM flocks WHERE id = $1;",
      &[(&flock_uuid, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock authz lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  let Some(row) = rows.into_iter().next() else {
    // Unknown flock: no access (routes surface this as 403 -- deliberately
    // not distinguishing missing-vs-forbidden, so an unauthorized caller
    // can't probe flock ids by existence).
    return Ok(None);
  };

  let owner_id: Uuid = row.get("user_id");
  let org_id: Option<Uuid> = row.get("org_id");

  let allowed = match org_id {
    None => {
      // Personal flock: creator has full rights.
      parse_uuid(&principal.user_id, "X-User-Id")? == owner_id
    }
    Some(org) => match principal.org_role(&org) {
      Some(role) => match action {
        FlockAction::View => true,
        FlockAction::Manage => role.is_manager(),
      },
      None => false,
    },
  };

  Ok(allowed.then(|| FlockAccess::assert_checked(flock_id_str)))
}

/// One flock by id, org/owner fields included -- used by the transfer route
/// and pigeon-create seeding. Pigeon ids deliberately included so callers
/// (transfer) don't need a second query.
pub async fn get_flock_with_pigeons(
  client: &Client,
  flock_id: &Uuid,
) -> Result<Option<(Flock, Vec<String>)>> {
  let rows = client
    .query_typed(
      "SELECT
        flocks.id, flocks.user_id, flocks.org_id, flocks.name, flocks.service_plan,
        flocks.created_at, flocks.updated_at,
        COALESCE(array_agg(pigeons.id) FILTER (WHERE pigeons.id IS NOT NULL), '{}') AS pigeon_ids
       FROM flocks
       LEFT JOIN pigeons ON pigeons.flock_id = flocks.id
       WHERE flocks.id = $1
       GROUP BY flocks.id;",
      &[(flock_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock lookup error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;

  Ok(rows.into_iter().next().map(|row| {
    let pigeon_ids: Vec<String> = row.get("pigeon_ids");
    (
      Flock {
        id: row.get("id"),
        user_id: row.get("user_id"),
        org_id: row.get("org_id"),
        name: row.get("name"),
        service_plan: row.get("service_plan"),
        pigeon_ids: pigeon_ids.clone(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
      },
      pigeon_ids,
    )
  }))
}

/// Marks a flock org-owned -- called by the transfer route AFTER every
/// pigeon DO has durably received the org ACL row (the DO write is the
/// authoritative one and is NOT best-effort; this Postgres flip is the
/// final step, so a failure part-way leaves the flock still personal with
/// some harmlessly-early org ACL rows, and the transfer can simply be
/// retried -- the DO grant is an idempotent upsert).
pub async fn set_flock_org(client: &Client, flock_id: &Uuid, org_id: &Uuid) -> Result<()> {
  client
    .execute_typed(
      "UPDATE flocks SET org_id = $2 WHERE id = $1;",
      &[(flock_id, Type::UUID), (org_id, Type::UUID)],
    )
    .await
    .map_err(|e| {
      console_error!("Flock org transfer update error: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::suffix_hint;

  #[test]
  fn suffix_hint_keeps_only_the_tail() {
    assert_eq!(
      suffix_hint("https://pidgeiot.com/invite?token=abcdef1234567890", 4),
      "...7890"
    );
  }

  #[test]
  fn suffix_hint_returns_whole_string_when_shorter_than_n() {
    assert_eq!(suffix_hint("ab", 4), "...ab");
  }
}
