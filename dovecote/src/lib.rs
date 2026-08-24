use crate::helpers::{
  DEVICE_FIRMWARE_LIMITER, DEVICE_SHADOW_LIMITER, DeviceAuthGuard, EntitlementCap,
  INGEST_PAUSED_MESSAGE, IngestFuse, PigeonAccess, Principal, STRIPE_WEBHOOK_SECRET,
  StripeCheckoutSessionRow, StripeWebhookEvent, TelemetryHistoryPage, WebhookClaim, accept_invite,
  apply_subscription, attach_stripe_customer, authenticate_browser, backfill_owner_email,
  build_invite_url, change_member_role, check_device_cap, check_flock_alert_cap, check_ingest_fuse,
  check_org_cap, check_pigeon_alert_cap, check_pigeon_authz, check_seat_cap, claim_webhook_event,
  constant_time_eq, count_billable_messages, create_checkout_session, create_customer,
  create_flock_alert, create_invite, create_organization, create_pigeon_alert,
  create_portal_session, create_user_flock, delete_alert_definition, delete_organization_if_empty,
  delete_pigeon_pg_db, device_surface_limit, ensure_billing_tables, ensure_billing_usage_tables,
  ensure_business_details_columns, erase_user_error_reports, fetch_subscription, get_db_client,
  get_flock_with_pigeons, get_hyperdrive_conn, get_org_stripe_customer, get_organization,
  get_user_flocks, grant_org_acl_via_do, ingest_error_report, insert_pigeon_pg_db, is_alert_owner,
  is_allowed_coap_service_ip, is_demo_pigeon, is_local_dev, list_demo_pigeon_alerts,
  list_flock_alert_state, list_flock_alerts, list_flock_firmware, list_org_invites,
  list_org_members, list_pigeon_alert_state, list_pigeon_alerts, list_user_organizations,
  load_business_details, load_org_billing_overview, load_org_roles, load_org_subscription_state,
  mark_webhook_event_processed, mint_invite_token, notify_contact_submission, org_role_of,
  plan_business_details, proxy_binary_to_pigeon_do, proxy_to_pigeon_do,
  proxy_websocket_to_pigeon_do, psk_lookup_via_do, query_telemetry_history_buckets_for_flock,
  query_telemetry_history_buckets_for_pigeon, query_telemetry_history_for_flock,
  query_telemetry_history_for_pigeon, raise_message_allowance_floor, readings_from_body,
  remove_member, rename_organization, resolve_checkout_prices, revoke_invite, send_feedback_email,
  send_invite_email, sha256_hex, store_contact_submission, stripe_configured,
  update_alert_definition, update_pigeon_pg_db, update_shadow_pg_db, update_subscription_tier,
  update_telemetry_endpoint_pg_db, upsert_acl_pg_db, upsert_flock_firmware, verify_cf_access,
  verify_device_via_do, verify_webhook_signature, write_business_details,
};
use crate::queue::TelemetryMessage;
use capsules::{
  AlertDefinitionCreateRequest, AlertDefinitionUpdateRequest, BillingCheckoutRequest, BillingPlan,
  BillingPlanChangeRequest, BillingSessionUrl, FirmwareTarget, FirmwareUploadQuery,
  FlockCreateRequest, MAX_TELEMETRY_BATCH_BYTES, OrgRole, OrganizationBusinessDetailsRequest,
  OrganizationCreateRequest, OrganizationDetail, OrganizationInviteAcceptRequest,
  OrganizationInviteCreateRequest, OrganizationInviteCreated, OrganizationMemberRoleUpdateRequest,
  OrganizationRenameRequest, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow,
  TELEMETRY_HISTORY_TRUNCATED_HEADER, TelemetryEndpoint, TelemetryHistoryBucket,
  TelemetryHistoryQuery, TelemetryReportBody,
};
use futures::future::join_all;
use worker::{
  Context, Date, Env, Headers, Method, Range, Request, RequestInit, Response, ResponseBuilder,
  RouteContext, Router, console_error, console_log, event,
};

mod helpers;
mod objects;
mod queue;
mod scheduled;

/// `worker::Cors` comma-joins every configured origin into
/// `Access-Control-Allow-Origin` instead of matching per-request — invalid
/// per the CORS spec, so browsers reject it once more than one origin is
/// configured. Match `Origin` against `ROOT_URL` ourselves and hand
/// `Cors::with_origins` exactly one value; can't be computed once and
/// shared by reference since each route is a separate `async move` closure
/// and `Cors` isn't `Copy`.
fn build_cors(env: &Env, req: &Request) -> worker::Cors {
  let root_origin = env
    .var("ROOT_URL")
    .map(|v| v.to_string())
    .unwrap_or_else(|_| "https://pidgeiot.com".to_string());

  let origin = req
    .headers()
    .get("Origin")
    .ok()
    .flatten()
    .filter(|o| *o == root_origin)
    .unwrap_or(root_origin);

  worker::Cors::new()
    .with_origins(vec![origin])
    .with_methods(vec![
      Method::Get,
      Method::Post,
      Method::Put,
      Method::Delete,
      Method::Options,
    ])
    .with_allowed_headers(vec!["Content-Type", "Accept", "Authorization"])
    .with_exposed_headers(vec!["Location", TELEMETRY_HISTORY_TRUNCATED_HEADER])
    .with_credentials(true)
}

/// History responses keep a bare `TelemetryHistoryPoint` array as their
/// body -- the newest-window cap rides in a header instead, so a dashboard
/// build that doesn't know about it yet keeps parsing these unchanged.
fn telemetry_history_response(page: TelemetryHistoryPage) -> worker::Result<Response> {
  let mut response = Response::from_json(&page.points)?;
  response.headers_mut().set(
    TELEMETRY_HISTORY_TRUNCATED_HEADER,
    if page.truncated { "true" } else { "false" },
  )?;
  Ok(response)
}

/// The default (non-`raw`) history response -- a bare `TelemetryHistoryBucket`
/// array, no truncation header. Unlike `telemetry_history_response`'s page,
/// there is nothing to flag: bucketing bounds the response by construction
/// (`capsules::TELEMETRY_HISTORY_BUCKET_TARGET`'s doc comment), so
/// `truncated` would always be `false` and isn't worth a header no client
/// needs to check.
fn telemetry_history_bucket_response(
  buckets: Vec<TelemetryHistoryBucket>,
) -> worker::Result<Response> {
  Response::from_json(&buckets)
}

/// RFC 9727 API catalog handler for `/.well-known/api-catalog`, shared by
/// the GET and HEAD registrations below. API origin comes from the
/// request's own URL so prod/staging/dev each describe themselves; doc
/// links reuse `ROOT_URL` (the frontend origin) instead of a new config
/// var.
async fn api_catalog(req: Request, ctx: RouteContext<()>) -> worker::Result<Response> {
  let cors = build_cors(&ctx.env, &req);
  let Ok(url) = req.url() else {
    return Response::error("Internal Server Error", 500)
      .unwrap()
      .with_cors(&cors);
  };
  let api_origin = url.origin().ascii_serialization();
  let root_url = ctx
    .env
    .var("ROOT_URL")
    .map(|v| v.to_string())
    .unwrap_or_else(|_| "https://pidgeiot.com".to_string());

  let catalog = serde_json::json!({
    "linkset": [
      {
        "anchor": format!("{api_origin}/.well-known/api-catalog"),
        "item": [{ "href": format!("{api_origin}/") }]
      },
      {
        "anchor": format!("{api_origin}/"),
        "service-doc": [
          { "href": format!("{root_url}/api-reference/"), "type": "text/html" },
          { "href": format!("{root_url}/api-reference/index.md"), "type": "text/markdown" }
        ],
        "service-meta": [
          { "href": format!("{root_url}/auth.md"), "type": "text/markdown" },
          { "href": format!("{root_url}/llms.txt"), "type": "text/plain" }
        ]
      }
    ]
  });

  let headers = Headers::new();
  if headers
    .set(
      "Content-Type",
      "application/linkset+json; profile=\"https://www.rfc-editor.org/info/rfc9727\"",
    )
    .is_err()
  {
    console_error!("Failed to set api-catalog response headers");
    return Response::error("Internal Server Error", 500)
      .unwrap()
      .with_cors(&cors);
  }

  let Ok(response) = Response::from_json(&catalog) else {
    return Response::error("Internal Server Error", 500)
      .unwrap()
      .with_cors(&cors);
  };

  response.with_headers(headers).with_cors(&cors)
}

/// A validated Kratos session's user id plus (if resolvable) email trait.
/// Named rather than a bare tuple to match this codebase's proof-of-check
/// style (`PigeonAccess`/`FlockAccess`/`AlertAccess`), though this isn't
/// itself an authorization proof.
pub struct AuthSession {
  pub user_id: String,
  pub email: Option<String>,
  /// Identity's VERIFIED addresses (lowercased), from
  /// `identity.verifiable_addresses` -- the only addresses an alert's
  /// per-alert email override may name (open signup means an unrestricted
  /// override could turn alert delivery into a spam relay). Distinct from
  /// `email` above, which is unverified.
  pub verified_emails: Vec<String>,
}

/// Validates the Kratos cookie and returns the full session identity.
///
/// `identity.traits` is a loosely-typed `Option<Value>`, not a generated
/// struct, so email is read via `.get("email")` -- matches
/// `schemas/kratos/identity.user.schema.json`'s top-level `traits.email`
/// trait, not `traits.identity.email`. A session that can't resolve an
/// email still yields `Ok` with `email: None` rather than failing -- most
/// callers never need it, and the routes that do treat a missing email as
/// "nothing to write yet."
pub async fn require_auth_session(req: &Request, env: &Env) -> worker::Result<AuthSession> {
  let session = crate::authenticate_browser(req, env)
    .await
    .map_err(|_| worker::Error::RustError("Unauthorized".to_string()))?;

  let identity = session
    .identity
    .ok_or_else(|| worker::Error::RustError("Session missing identity".to_string()))?;

  let email = identity
    .traits
    .as_ref()
    .and_then(|traits| traits.get("email"))
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

  let verified_emails = identity
    .verifiable_addresses
    .unwrap_or_default()
    .into_iter()
    .filter(|a| a.verified)
    .map(|a| a.value.to_lowercase())
    .collect();

  Ok(AuthSession {
    user_id: identity.id,
    email,
    verified_emails,
  })
}

/// An alert's per-alert email override may only name an address the
/// caller's identity has VERIFIED -- otherwise open signup turns alert
/// delivery into a spam relay (point `channel.Email.to` at any stranger,
/// feed it telemetry). `None` (deliver to flock owner_email) is always
/// fine. Returns the user-facing rejection message so both create routes
/// and the update route emit the same 400 body. Case-insensitive;
/// `verified_emails` is already lowercased at construction.
fn validate_alert_channel(
  channel: &capsules::AlertChannel,
  verified_emails: &[String],
) -> std::result::Result<(), &'static str> {
  let capsules::AlertChannel::Email { to: Some(addr) } = channel else {
    return Ok(());
  };
  if verified_emails
    .iter()
    .any(|v| v == &addr.trim().to_lowercase())
  {
    Ok(())
  } else {
    Err("Bad Request: alert email override must match your account's verified email address")
  }
}

/// Validates the Kratos cookie and returns just the user id. Thin wrapper
/// over `require_auth_session` for the many call sites that only need the
/// id.
pub async fn require_auth(req: &Request, env: &Env) -> worker::Result<String> {
  require_auth_session(req, env).await.map(|s| s.user_id)
}

/// Validates the Kratos cookie AND loads the caller's org-membership set
/// in one Postgres query (`load_org_roles`), producing the `Principal`
/// every org-aware route forwards to Durable Objects as `X-User-Id` +
/// `X-Org-Roles` and hands to `authorize_flock`.
///
/// A failed org-membership load (PG blip, tables not migrated) degrades to
/// an empty org set -- fail-closed for org-granted access, fail-open for
/// personal access -- rather than failing the whole request. Pure-DO
/// pigeon routes have no Postgres dependency otherwise; a Hyperdrive
/// outage must not 500 a personal pigeon read that never needed Postgres.
pub async fn require_principal(req: &Request, env: &Env) -> worker::Result<Principal> {
  let auth = require_auth_session(req, env).await?;

  let org_roles = match get_db_client(env).await {
    Ok(client) => match load_org_roles(&client, &auth.user_id).await {
      Ok(roles) => roles,
      Err(e) => {
        console_error!(
          "Org membership load failed for user {} (degrading to personal-only access): {e}",
          auth.user_id
        );
        Vec::new()
      }
    },
    Err(e) => {
      console_error!("Org membership load skipped: Hyperdrive connection failed: {e}");
      Vec::new()
    }
  };

  Ok(Principal::new(
    auth.user_id,
    auth.email,
    auth.verified_emails,
    org_roles,
  ))
}

// --- MACROS & HELPERS ---

/// Declares `pigeon_id`, `namespace`, and `obj_id` in the caller's scope.
/// This prevents the borrow checker from panicking over `ObjectId`'s lifetime
/// constraint, which dictates it must not outlive the `ObjectNamespace`.
macro_rules! get_pigeon_do {
  ($ctx:expr, $pigeon_id:ident, $namespace:ident, $obj_id:ident, $cors:expr) => {
    let Some($pigeon_id) = $ctx.param("pigeon_id").cloned() else {
      return Response::error("Pigeon ID cannot be empty or invalid", 400)
        .unwrap()
        .with_cors($cors);
    };

    let Ok($namespace) = $ctx.durable_object("PIGEONS") else {
      return Response::error("Failed to bind to PIGEONS namespace", 500)
        .unwrap()
        .with_cors($cors);
    };

    let Ok($obj_id) = $namespace.id_from_string(&$pigeon_id) else {
      return Response::error("Malformed Pigeon ID string", 400)
        .unwrap()
        .with_cors($cors);
    };
  };
}

/// Helper to establish a DB client, mapping failures to HTTP 500 responses.
macro_rules! get_db {
  ($env:expr, $client:ident, $cors:expr) => {
    let Ok($client) = get_db_client(&$env).await else {
      console_error!("Failed to establish Hyperdrive connection");
      return Response::error("DB Error", 500).unwrap().with_cors($cors);
    };
  };
}

/// Safely attempts to parse JSON from a DO response payload, surfacing internal server errors.
async fn parse_do_response<T: serde::de::DeserializeOwned>(
  mut resp: Response,
) -> worker::Result<T> {
  resp.json::<T>().await.map_err(|e| {
    console_error!("Failed to parse DO response: {e}");
    worker::Error::RustError("Internal Server Error".into())
  })
}

/// Parses a standard HTTP `Range` header (`bytes=<start>-<end>`,
/// `bytes=<start>-` open-ended, or `bytes=-<suffix>` trailing slice) into
/// an R2 [`Range`]. The nRF9160 downloads firmware in small chunks straight
/// to flash rather than buffering ~300KB-1MB in its ~256KB of RAM, so
/// ranged reads are required, not an optimization. Only a single range is
/// supported (multi-range requests just use the first) since the device
/// downloads sequentially. Returns `None` on anything malformed so the
/// caller falls back to serving the whole object.
fn parse_range_header(header: &str) -> Option<Range> {
  let spec = header.strip_prefix("bytes=")?;
  let spec = spec.split(',').next()?.trim();
  let (start, end) = spec.split_once('-')?;

  if start.is_empty() {
    let suffix: u64 = end.parse().ok()?;
    return Some(Range::Suffix { suffix });
  }

  let offset: u64 = start.parse().ok()?;
  if end.is_empty() {
    return Some(Range::OffsetToEnd { offset });
  }

  let end: u64 = end.parse().ok()?;
  if end < offset {
    return None;
  }
  Some(Range::OffsetWithLength {
    offset,
    length: end - offset + 1,
  })
}

/// Computes the inclusive `(start, end)` byte range actually served for
/// `Content-Range`/`Content-Length`, using the shadow-assigned
/// `FirmwareTarget`'s size as authoritative rather than R2's own
/// `Object::size()`/`Object::range()` (ambiguous for a ranged fetch per the
/// `worker` crate's docs). Clamps an out-of-bounds request to the object's
/// end instead of a hard 416 -- a device racing a shrinking/reassigned
/// image is a rare edge case.
fn resolve_serve_range(range: &Range, total: u64) -> (u64, u64) {
  let last = total.saturating_sub(1);
  match *range {
    Range::OffsetWithLength { offset, length } => (
      offset.min(last),
      (offset + length.saturating_sub(1)).min(last),
    ),
    Range::OffsetToEnd { offset } => (offset.min(last), last),
    Range::Prefix { length } => (0, length.saturating_sub(1).min(last)),
    Range::Suffix { suffix } => (total.saturating_sub(suffix), last),
  }
}

/// Serves both names of the service-internal PSK route. NOT a device or
/// dashboard route: the only legitimate callers are the protocol
/// terminators themselves, gated by two independent layers -- a
/// source-address allowlist (COAP_SERVICE_ALLOWED_IPS, their egress
/// addresses) and the COAP_SERVICE_SECRET Worker secret (set via `wrangler
/// secret put` per env, never [vars] -- same convention as RESEND_API_KEY;
/// local dev reads it from dovecote/.dev.vars). The var and secret keep
/// their CoAP-era names: one gate, one shared value, and renaming a
/// deployed secret buys nothing. The `:pigeon_id` path param IS the PSK
/// identity -- `create`/`refresh_token` mint `tls_psk_identity` as the
/// pigeon's own DO id. An environment where either layer is unconfigured
/// refuses every call (fail closed), so this route is inert until a
/// terminator is actually provisioned.
async fn internal_psk_lookup(req: Request, ctx: RouteContext<()>) -> worker::Result<Response> {
  let cors = build_cors(&ctx.env, &req);

  // Address gate first: the secret grants unscoped PSK resolution for
  // every pigeon, so a leaked copy must not be usable from anywhere
  // but the terminator host -- and a disallowed caller gets no timing
  // signal from the secret comparison either.
  if !is_allowed_coap_service_ip(&ctx.env, &req) {
    console_error!(
      "Internal PSK lookup from disallowed address {:?}",
      req.headers().get("CF-Connecting-IP").ok().flatten()
    );
    return Response::error("Forbidden", 403).unwrap().with_cors(&cors);
  }

  let Ok(expected) = ctx.env.secret("COAP_SERVICE_SECRET") else {
    console_error!("COAP_SERVICE_SECRET not configured; refusing internal PSK lookup");
    return Response::error("Forbidden", 403).unwrap().with_cors(&cors);
  };
  // An empty or whitespace-only secret counts as unconfigured -- the
  // same definition loft's config guard applies on its side -- because
  // a bare "Bearer " header would otherwise satisfy the constant-time
  // compare below and open every pigeon's PSK to anyone.
  let expected = expected.to_string();
  if expected.trim().is_empty() {
    console_error!("COAP_SERVICE_SECRET is empty; refusing internal PSK lookup");
    return Response::error("Forbidden", 403).unwrap().with_cors(&cors);
  }

  let presented = req
    .headers()
    .get("Authorization")
    .ok()
    .flatten()
    .and_then(|h| h.strip_prefix("Bearer ").map(str::to_string));
  let Some(presented) = presented else {
    return Response::error("Unauthorized", 401)
      .unwrap()
      .with_cors(&cors);
  };
  if !constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
    console_error!("Internal PSK lookup with wrong service secret");
    return Response::error("Forbidden", 403).unwrap().with_cors(&cors);
  }

  get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

  // 200 with capsules::CoapPskLookup, or the DO's own 404 for an
  // unknown identity or a connector that mints no PSK -- passed through
  // unchanged so the terminator's negative cache keys off a real 404.
  psk_lookup_via_do(&obj_id).await?.with_cors(&cors)
}

#[event(fetch, respond_with_errors)]
async fn main(req: Request, env: Env, _ctx: Context) -> worker::Result<Response> {
  // Used only by the catch-all panic guard below, after `env` is moved
  // into `.run()` — every route closure computes its own from `ctx.env`
  // instead (see `build_cors`), since a single `Cors` can't be shared
  // by-reference across multiple `async move` closures.
  let fallback_cors = build_cors(&env, &req);

  // Only enforced when CF_ACCESS_AUD/CF_ACCESS_CERTS_URL are configured
  // (staging's uploaded-version vars) — dev and production don't set
  // these, so verify_cf_access is a no-op there and this block never
  // runs. Rejects before the router sees the request at all.
  if let Err(reason) = verify_cf_access(&req, &env).await {
    console_error!("Cloudflare Access rejected request: {reason}");
    return Response::error("Forbidden", 403)
      .unwrap()
      .with_cors(&fallback_cors);
  }

  let router = Router::new()
    .options_async("/*any", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      Response::empty()?.with_cors(&cors)
    })
    // RFC 9727 API catalog: a machine-readable linkset describing this API
    // host at the standard well-known path (Cloudflare Agent Readiness
    // checklist). Unauthenticated by design -- discovery metadata only,
    // links to the public docs; no data, no secrets. Registered for both
    // GET and HEAD (RFC 9727 §3 requires HEAD to resolve; worker::Router
    // has no automatic HEAD->GET fallback -- the runtime strips the body
    // from the HEAD variant's response itself).
    .get_async("/.well-known/api-catalog", |req, ctx| async move {
      api_catalog(req, ctx).await
    })
    .head_async("/.well-known/api-catalog", |req, ctx| async move {
      api_catalog(req, ctx).await
    })
    .post_async("/pigeons/:pigeon_id/token/refresh", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let mut do_response = proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/token/refresh").await?;

      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let pigeon = do_response.json::<Pigeon>().await.map_err(|e| {
        console_error!("Failed to parse DO response: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = update_pigeon_pg_db(client, &pigeon).await {
            console_error!("External DB Sync Error for pigeon {}: {e}", pigeon.id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&pigeon)?.with_cors(&cors)
    })
    .get_async("/device/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);

      // Ahead of every step that costs anything: an address already over
      // its failed-auth budget is refused without a Durable Object round
      // trip, which is the cost this limiter exists to bound.
      let auth_guard = DeviceAuthGuard::new(&ctx.env, &req);
      if let Some(limited) = auth_guard.blocked_response(&cors) {
        return limited;
      }

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // Poll traffic is neither billed nor counted, so nothing else bounds
      // how often one pigeon can ask. Sized in wrangler.toml above what any
      // configurable device cadence can reach.
      if let Some(limited) =
        device_surface_limit(&ctx.env, &DEVICE_SHADOW_LIMITER, &pigeon_id, &cors).await
      {
        return limited;
      }

      // No X-User-Id / Kratos session here — the DO verifies the device's
      // own Authorization header (forwarded by proxy_to_pigeon_do) against
      // this pigeon's stored public key.
      let do_response = proxy_to_pigeon_do(req, "", None, &obj_id, "/device/shadow").await?;
      if do_response.status_code() == 401 {
        auth_guard.note_failure(&ctx.env).await;
      }
      do_response.with_cors(&cors)
    })
    .post_async("/device/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);

      // Ahead of every step that costs anything: an address already over
      // its failed-auth budget is refused without a Durable Object round
      // trip, which is the cost this limiter exists to bound.
      let auth_guard = DeviceAuthGuard::new(&ctx.env, &req);
      if let Some(limited) = auth_guard.blocked_response(&cors) {
        return limited;
      }

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // Same device-auth model as the GET route above — no X-User-Id here.
      let do_response = proxy_to_pigeon_do(req, "", None, &obj_id, "/device/shadow/report").await?;
      if do_response.status_code() >= 400 {
        if do_response.status_code() == 401 {
          auth_guard.note_failure(&ctx.env).await;
        }
        return do_response.with_cors(&cors);
      }

      // An accepted report-back is one billable device message, same as a
      // telemetry report. Telemetry defers its tally to the queue
      // consumer, but this path has no queue leg, so it's tallied here --
      // after the DO has verified the token and stored the report, so a
      // rejected request never counts. Best-effort inside: a failed tally
      // undercounts in the customer's favour, never fails the device's
      // confirmation.
      count_billable_messages(&ctx.env, &pigeon_id, 1).await;

      let shadow = parse_do_response::<PigeonShadow>(do_response).await?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = update_shadow_pg_db(client, &pigeon_id, &shadow).await {
            console_error!("External DB Sync Error for shadow {}: {e}", pigeon_id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&shadow)?.with_cors(&cors)
    })
    // Real-time channel for non-cellular (WiFi/mains-powered) devices -- a
    // persistent, hibernation-backed WebSocket in place of the poll/report
    // pattern above. `GET` matches the standard WebSocket-upgrade
    // convention (the upgrade is a GET with an `Upgrade: websocket`
    // header, not a distinct HTTP method).
    .get_async("/device/pigeons/:pigeon_id/ws", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);

      // Ahead of every step that costs anything: an address already over
      // its failed-auth budget is refused without a Durable Object round
      // trip, which is the cost this limiter exists to bound.
      let auth_guard = DeviceAuthGuard::new(&ctx.env, &req);
      if let Some(limited) = auth_guard.blocked_response(&cors) {
        return limited;
      }

      let is_upgrade = req
        .headers()
        .get("Upgrade")
        .ok()
        .flatten()
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
      if !is_upgrade {
        return Response::error("Bad Request: expected a WebSocket Upgrade request", 400)
          .unwrap()
          .with_cors(&cors);
      }

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // Same device-auth model as the other /device/pigeons/:id/* routes —
      // no X-User-Id here. The DO verifies the bearer token itself, BEFORE
      // accepting the socket, against this pigeon's own device_public_key
      // (see is_authorized_device/accept_websocket_device,
      // objects/pigeons.rs) — a rejected token comes back as a normal 401
      // response instead of a 101 upgrade.
      let do_response = proxy_websocket_to_pigeon_do(req, &obj_id, "/device/ws").await?;
      if do_response.status_code() == 401 {
        auth_guard.note_failure(&ctx.env).await;
      }
      do_response.with_cors(&cors)
    })
    .post_async(
      "/device/pigeons/:pigeon_id/telemetry",
      |mut req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);

        // Ahead of every step that costs anything: an address already over
        // its failed-auth budget is refused without a Durable Object round
        // trip, which is the cost this limiter exists to bound.
        let auth_guard = DeviceAuthGuard::new(&ctx.env, &req);
        if let Some(limited) = auth_guard.blocked_response(&cors) {
          return limited;
        }

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // Telemetry queue -- bound in both env.staging.queues and the
        // default/production [[queues.*]] blocks of wrangler.toml. Only
        // dev has no TELEMETRY_QUEUE binding and falls through to the
        // synchronous direct-DO-write path below.
        //
        // Anywhere else that fallback would be a silent regression rather
        // than a local convenience: it moves every report's history write
        // back onto the request's own critical path, so ingestion quietly
        // becomes slower and more expensive per message while still
        // answering 200. A deployed environment that has lost this
        // binding is a deployment fault, and answering 500 is what makes
        // it visible -- the device keeps its unsent readings queued and
        // retries, so the reports are delayed rather than lost.
        let telemetry_queue = match ctx.env.queue("TELEMETRY_QUEUE") {
          Ok(queue) => queue,
          Err(e) => {
            if !is_local_dev(&ctx.env) {
              console_error!(
                "TELEMETRY_QUEUE binding unavailable in a deployed environment: {e}"
              );
              return Response::error("Internal Server Error", 500)
                .unwrap()
                .with_cors(&cors);
            }
            // Same device-auth model as the shadow device routes above.
            return proxy_to_pigeon_do(req, "", None, &obj_id, "/device/telemetry")
              .await?
              .with_cors(&cors);
          }
        };

        // The queue has no authentication of its own, so the device's
        // bearer token must be verified against the DO *before* anything
        // is enqueued — an unauthenticated/forged report must never reach
        // the queue. This costs one extra DO round trip (verify) on top of
        // the eventual consumer write, versus one combined round trip in
        // the non-queue path above.
        let auth_header = req.headers().get("Authorization").ok().flatten();

        // Read as text first: the batch form has a raw-body cap, and the
        // only way to enforce a byte cap is to measure the bytes before
        // deciding what they are.
        let Ok(raw) = req.text().await else {
          return Response::error("Bad Request: Invalid JSON", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(body) = serde_json::from_str::<TelemetryReportBody>(&raw) else {
          return Response::error("Bad Request: Invalid JSON", 400)
            .unwrap()
            .with_cors(&cors);
        };

        // The cap applies to the batch form only. A flat report has always
        // been bounded by the key and value caps alone, and narrowing that
        // retroactively would refuse bodies devices in the field already
        // send.
        if matches!(body, TelemetryReportBody::Batch(_)) && raw.len() > MAX_TELEMETRY_BATCH_BYTES {
          return Response::error(
            format!(
              "Payload Too Large: telemetry batch over {MAX_TELEMETRY_BATCH_BYTES} bytes"
            ),
            413,
          )
          .unwrap()
          .with_cors(&cors);
        }

        // Resolved here, at the edge, because this is where the receive
        // time is known -- a device's own timestamps are clamped against
        // it (see helpers::resolve_batch) and nothing downstream re-derives
        // one. Refused here rather than at the consumer so the device
        // learns its report was rejected: past this point the route answers
        // 202 and the write happens off the queue, where nothing can reach
        // the device. A report is applied whole or not at all.
        let now_secs = (Date::now().as_millis() / 1000) as i64;
        let readings = match readings_from_body(body, now_secs) {
          Ok(readings) => readings,
          Err(message) => {
            return Response::error(message, 400).unwrap().with_cors(&cors);
          }
        };

        let verify_resp =
          verify_device_via_do(auth_header, &obj_id, "/device/telemetry/verify").await?;
        if verify_resp.status_code() >= 400 {
          if verify_resp.status_code() == 401 {
            auth_guard.note_failure(&ctx.env).await;
          }
          return verify_resp.with_cors(&cors);
        }

        // Free-tier fuse, checked after auth so only a real device ever
        // sees it: a free account past its monthly message allowance gets
        // a 429 -- the pigeon library backs off and keeps unsent readings
        // queued, so data is delayed rather than lost. Deliberately never
        // 401 (that status is reserved for "session gone") and fail-open
        // inside the check, so a lookup failure can't brick ingestion.
        if matches!(
          check_ingest_fuse(&ctx.env, &pigeon_id).await,
          IngestFuse::Pause
        ) {
          return Response::error(INGEST_PAUSED_MESSAGE, 429)
            .unwrap()
            .with_cors(&cors);
        }

        // Pre-serialize the readings here: a Vec round-tripped through
        // Queue::send arrives at the consumer as an empty object
        // (serde-wasm-bindgen map -> JS Map -> JSON.stringify == "{}"),
        // so the queue message carries a JSON string instead -- see
        // TelemetryMessage in queue.rs.
        let Ok(readings_json) = serde_json::to_string(&readings) else {
          console_error!("Failed to serialize telemetry for pigeon {pigeon_id}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let message = TelemetryMessage {
          pigeon_id: pigeon_id.clone(),
          reported_at_ms: Date::now().as_millis(),
          readings_json: Some(readings_json),
          // This route enqueues right after a bare auth check -- no DO
          // round trip has happened yet that could merge the readings or
          // capture a previous value, so the consumer's own write hop
          // (write_telemetry_device, via queue.rs::dispatch_write) does
          // both.
          pre_merged: false,
          metrics_json: String::new(),
          previous_values_json: None,
        };

        if telemetry_queue.send(message).await.is_err() {
          console_error!("Failed to enqueue telemetry for pigeon {pigeon_id}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        Response::ok("{}")
          .unwrap()
          .with_status(202)
          .with_cors(&cors)
      },
    )
    .post_async("/device/pigeons/:pigeon_id/logs", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);

      // Ahead of every step that costs anything: an address already over
      // its failed-auth budget is refused without a Durable Object round
      // trip, which is the cost this limiter exists to bound.
      let auth_guard = DeviceAuthGuard::new(&ctx.env, &req);
      if let Some(limited) = auth_guard.blocked_response(&cors) {
        return limited;
      }

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // Same device-auth model as the other /device/pigeons/:id/* routes —
      // no X-User-Id here, the DO verifies the bearer token itself. Body is
      // a raw binary dictionary-log chunk, not JSON — proxy_binary_to_pigeon_do
      // forwards it byte-for-byte instead of through proxy_to_pigeon_do's
      // text()-based forwarding, which would corrupt non-UTF-8 bytes.
      let do_response = proxy_binary_to_pigeon_do(req, &obj_id, "/device/logs").await?;
      if do_response.status_code() >= 400 {
        if do_response.status_code() == 401 {
          auth_guard.note_failure(&ctx.env).await;
        }
        return do_response.with_cors(&cors);
      }

      // An accepted log chunk is one billable device message, same as a
      // telemetry report. No queue leg on this path either, so it's
      // tallied here, only after the DO has verified the token and stored
      // the chunk; best-effort inside, so a failed tally undercounts
      // rather than failing the upload.
      count_billable_messages(&ctx.env, &pigeon_id, 1).await;

      do_response.with_cors(&cors)
    })
    .get_async(
      "/device/pigeons/:pigeon_id/firmware",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);

        // Ahead of every step that costs anything: an address already over
        // its failed-auth budget is refused without a Durable Object round
        // trip, which is the cost this limiter exists to bound.
        let auth_guard = DeviceAuthGuard::new(&ctx.env, &req);
        if let Some(limited) = auth_guard.blocked_response(&cors) {
          return limited;
        }

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // One Range request per chunk, and each one costs a DO round trip
        // to resolve the target plus an R2 read. Sized in wrangler.toml
        // above the fastest cadence the device library's own chunking can
        // produce, because a false 429 here is expensive: the device
        // aborts the whole transfer on its first chunk error, and gives up
        // on a version entirely after three aborts.
        if let Some(limited) =
          device_surface_limit(&ctx.env, &DEVICE_FIRMWARE_LIMITER, &pigeon_id, &cors).await
        {
          return limited;
        }

        // Extracted before proxy_to_pigeon_do consumes `req` below.
        let range = req
          .headers()
          .get("Range")
          .ok()
          .flatten()
          .and_then(|h| parse_range_header(&h));

        // Same device-auth model as the other /device/pigeons/:id/* routes —
        // no X-User-Id here. The DO verifies the bearer token itself and, on
        // success, hands back this pigeon's currently-assigned firmware
        // target (from its own shadow's target_config.firmware) in this one
        // round trip, so a second DO call isn't needed just to resolve which
        // R2 object to stream.
        let do_response = proxy_to_pigeon_do(req, "", None, &obj_id, "/device/firmware/target").await?;
        if do_response.status_code() >= 400 {
          if do_response.status_code() == 401 {
            auth_guard.note_failure(&ctx.env).await;
          }
          return do_response.with_cors(&cors);
        }

        let target = parse_do_response::<FirmwareTarget>(do_response).await?;

        let Ok(bucket) = ctx.env.bucket("FIRMWARE_BUCKET") else {
          console_error!("Failed to bind FIRMWARE_BUCKET");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let object_key = format!("firmware/{}.bin", target.sha256);
        let mut get_builder = bucket.get(&object_key);
        if let Some(r) = range.clone() {
          get_builder = get_builder.range(r);
        }

        let Ok(Some(object)) = get_builder.execute().await else {
          console_error!("Firmware object missing from R2: {object_key}");
          return Response::error("Not Found: Firmware object missing from storage", 404)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(body) = object.body() else {
          console_error!("R2 object body unexpectedly absent for {object_key}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(response_body) = body.response_body() else {
          console_error!("Failed to build streamed response body for {object_key}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        // target.size is the total from the pigeon's own shadow — treated as
        // authoritative for Content-Range/Content-Length rather than R2's own
        // Object::size()/Object::range() (ambiguous for a ranged fetch; see
        // resolve_serve_range's doc comment).
        let total = target.size.max(0) as u64;
        let headers = Headers::new();
        let mut ok = headers.set("Accept-Ranges", "bytes").is_ok();
        ok &= headers
          .set("Content-Type", "application/octet-stream")
          .is_ok();
        ok &= headers.set("ETag", &object.http_etag()).is_ok();
        ok &= headers.set("X-Firmware-Sha256", &target.sha256).is_ok();
        ok &= headers.set("X-Firmware-Version", &target.version).is_ok();
        ok &= headers.set("X-Firmware-Size", &total.to_string()).is_ok();

        let status = match range {
          Some(r) => {
            let (start, end) = resolve_serve_range(&r, total);
            ok &= headers
              .set("Content-Length", &(end + 1 - start).to_string())
              .is_ok();
            ok &= headers
              .set("Content-Range", &format!("bytes {start}-{end}/{total}"))
              .is_ok();
            206
          }
          None => {
            ok &= headers.set("Content-Length", &total.to_string()).is_ok();
            200
          }
        };

        if !ok {
          console_error!("Failed to set one or more firmware response headers");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(builder) = ResponseBuilder::new()
          .with_status(status)
          .with_headers(headers)
          .with_cors(&cors)
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Ok(builder.body(response_body))
      },
    )
    .get_async("/pigeons/:pigeon_id/logs", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/logs/get")
        .await?
        .with_cors(&cors)
    })
    // Service-internal PSK resolution for the protocol terminators: the
    // CoAP one (`loft`) and the MQTT broker (`pigeonhole`), each in its
    // own repo. Two names, one handler -- `device-psk` is what a
    // terminator that is not CoAP asks for, and the older `coap-psk` name
    // stays because loft is deployed against it and moves over on its own
    // schedule.
    .get_async("/internal/device-psk/:pigeon_id", |req, ctx| async move {
      internal_psk_lookup(req, ctx).await
    })
    .get_async("/internal/coap-psk/:pigeon_id", |req, ctx| async move {
      internal_psk_lookup(req, ctx).await
    })
    .get_async("/flocks", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      let user_flocks = get_user_flocks(&client, &auth.user_id).await?;

      // Best-effort backfill of `owner_email` (alerts recipient) for flocks
      // created before that column was populated on create -- never fails
      // this request, same fire-and-log convention as every PG-sync side
      // effect in this codebase.
      if let Some(email) = &auth.email
        && let Err(e) = backfill_owner_email(&client, &auth.user_id, email).await
      {
        console_error!("owner_email backfill failed for user {}: {e}", auth.user_id);
      }

      Response::from_json(&user_flocks)?.with_cors(&cors)
    })
    .post_async("/flocks", |mut req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(payload) = req.json::<FlockCreateRequest>().await else {
        return Response::error("Invalid JSON payload", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if payload.name.trim().is_empty() {
        return Response::error("Flock name cannot be empty", 400)
          .unwrap()
          .with_cors(&cors);
      }

      get_db!(ctx.env, client, &cors);

      let flock = create_user_flock(
        &client,
        &auth.user_id,
        &payload.name,
        auth.email.as_deref(),
      )
      .await?;

      let headers = Headers::new();
      if headers
        .set("Location", &format!("/flocks/{}", flock.id))
        .is_err()
      {
        console_error!("Failed to set Location header for flock {}", flock.id);
      }

      Response::from_json(&flock)?
        .with_status(201)
        .with_headers(headers)
        .with_cors(&cors)
    })
    .post_async("/pigeons/batch", |mut req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(pigeon_ids) = req.json::<Vec<String>>().await else {
        return Response::error("Pigeon IDs cannot be empty or invalid", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if pigeon_ids.len() > 48 {
        return Response::error("Batch size exceeds subrequest limits", 400)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(pigeon_namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500)
          .unwrap()
          .with_cors(&cors);
      };

      let org_roles_json = principal.org_roles_header().map(|s| s.to_string());
      let fetch_tasks = pigeon_ids.into_iter().map(|id| {
        let namespace_clone = pigeon_namespace.clone();
        let u_id = principal.user_id.clone();
        let org_json = org_roles_json.clone();

        async move {
          let stub = namespace_clone.id_from_string(&id).ok()?.get_stub().ok()?;

          let headers = worker::Headers::new();
          headers.append("X-User-Id", &u_id).ok()?;
          if let Some(org_json) = &org_json {
            headers.append("X-Org-Roles", org_json).ok()?;
          }

          let mut do_req_init = RequestInit::default();
          do_req_init.with_headers(headers);

          let do_req = Request::new_with_init("https://internal/pigeon/get", &do_req_init).ok()?;
          stub.fetch_with_request(do_req).await.ok()
        }
      });

      let responses = join_all(fetch_tasks).await;
      let mut pigeons: Vec<Pigeon> = Vec::with_capacity(responses.len());

      for mut resp in responses.into_iter().flatten() {
        if let Ok(pigeon) = resp.json::<Pigeon>().await {
          pigeons.push(pigeon);
        }
      }

      Response::from_json(&pigeons)?.with_cors(&cors)
    })
    .post_async("/flock/pigeons", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      // Peek the payload for flock authorization without consuming the
      // original request's body (same clone pattern as POST
      // /pigeons/:id/alerts) -- the DO's own /create handler still parses
      // the original below.
      let Ok(mut peek_req) = req.clone() else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };
      let Ok(payload) = peek_req.json::<capsules::PigeonCreateRequest>().await else {
        return Response::error("Bad Request: Invalid JSON", 400)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      // Pigeon creation is gated on the TARGET FLOCK -- personal flock: its
      // owner only; org-owned flock: org owner/admin (plain members are
      // read/telemetry-level, see docs/api.md's matrix).
      let Ok(flock_row) = get_flock_with_pigeons(&client, &payload.flock_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };
      let Some((flock, _)) = flock_row else {
        return Response::error("Forbidden: You do not have access to this flock", 403)
          .unwrap()
          .with_cors(&cors);
      };
      let allowed = match flock.org_id {
        None => flock.user_id.to_string() == principal.user_id,
        Some(org) => principal.org_role(&org).is_some_and(|r| r.is_manager()),
      };
      if !allowed {
        return Response::error(
          "Forbidden: Only the flock owner (or an org owner/admin) can create pigeons here",
          403,
        )
        .unwrap()
        .with_cors(&cors);
      }

      // Device-count entitlement, status-gated before plan inside
      // check_device_cap. A refusal blocks growth only -- existing devices
      // keep ingesting -- and the check fails open on lookup errors, so a
      // Postgres blip can't block provisioning.
      if let EntitlementCap::Refuse(message) = check_device_cap(&client, &payload.flock_id).await {
        return Response::error(message, 403).unwrap().with_cors(&cors);
      }

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500)
          .unwrap()
          .with_cors(&cors);
      };

      let obj_id = namespace.unique_id().map_err(|e| {
        console_error!("Failed to create unique DO ID: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      let do_response =
        proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/create").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let pcr = parse_do_response::<PigeonDetail>(do_response).await?;

      // Org-owned flock: seed the org's own ACL row alongside the
      // creator's. The DO write is authoritative, not best-effort -- a
      // failed grant fails the request loudly (the pigeon exists with the
      // creator as owner; retry via POST /pigeons/:id/acl).
      let org_acl = match flock.org_id {
        Some(org) => {
          let org_id_str = org.to_string();
          let Ok(grant_resp) = grant_org_acl_via_do(&obj_id, &org_id_str).await else {
            console_error!("Org ACL grant dispatch failed for pigeon {}", pcr.pigeon.id);
            return Response::error(
              "Internal Server Error: pigeon created but org access grant failed -- retry via POST /pigeons/:id/acl",
              500,
            )
            .unwrap()
            .with_cors(&cors);
          };
          if grant_resp.status_code() >= 400 {
            console_error!(
              "Org ACL grant failed ({}) for pigeon {}",
              grant_resp.status_code(),
              pcr.pigeon.id
            );
            return Response::error(
              "Internal Server Error: pigeon created but org access grant failed -- retry via POST /pigeons/:id/acl",
              500,
            )
            .unwrap()
            .with_cors(&cors);
          }
          Some(PigeonAcl {
            entity_id: org,
            role: "owner".to_string(),
          })
        }
        None => None,
      };

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = insert_pigeon_pg_db(client, &pcr).await {
            console_error!("External DB Sync Error for pigeon {}: {e}", pcr.pigeon.id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      // Best-effort PG mirror of the org ACL row (the client from the
      // authz check above is still usable here) -- same fire-and-log
      // convention as every other PG sync.
      if let Some(acl) = &org_acl
        && let Err(e) = upsert_acl_pg_db(&client, &pcr.pigeon.id, acl).await
      {
        console_error!("External DB Sync Error for org ACL {}: {e}", pcr.pigeon.id);
      }

      let headers = Headers::new();
      if headers
        .set("Location", &format!("/pigeons/{}", pcr.pigeon.id))
        .is_err()
      {
        console_error!(
          "Failed to set Location header for pigeon {}",
          pcr.pigeon.id
        );
      }

      Response::from_json(&pcr)?
        .with_status(201)
        .with_headers(headers)
        .with_cors(&cors)
    })
    // --- Flock transfer ---
    // Moves a PERSONAL flock into an org the caller manages. The org ACL
    // row is propagated into every member pigeon's Durable Object FIRST
    // (authoritative, not best-effort -- any failure aborts before the
    // flock is marked org-owned; the DO grant is an idempotent upsert, so
    // a retry after a partial failure is safe), then flocks.org_id flips,
    // then the Postgres pigeon_acl mirror is synced best-effort per the
    // usual convention.
    .post_async(
      "/flocks/:flock_id/transfer",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx
          .param("flock_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<capsules::FlockTransferRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(flock_row) = get_flock_with_pigeons(&client, &flock_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        let Some((flock, pigeon_ids)) = flock_row else {
          return Response::error("Forbidden: You do not have access to this flock", 403)
            .unwrap()
            .with_cors(&cors);
        };

        if flock.org_id.is_some() {
          return Response::error("Conflict: flock is already owned by an organization", 409)
            .unwrap()
            .with_cors(&cors);
        }
        if flock.user_id.to_string() != principal.user_id {
          return Response::error("Forbidden: only the flock owner can transfer it", 403)
            .unwrap()
            .with_cors(&cors);
        }
        if !principal
          .org_role(&payload.org_id)
          .is_some_and(|r| r.is_manager())
        {
          return Response::error(
            "Forbidden: you must be an owner/admin of the target organization",
            403,
          )
          .unwrap()
          .with_cors(&cors);
        }

        let Ok(namespace) = ctx.durable_object("PIGEONS") else {
          return Response::error("Failed to bind to PIGEONS namespace", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let org_id_str = payload.org_id.to_string();
        for pid in &pigeon_ids {
          let Ok(obj_id) = namespace.id_from_string(pid) else {
            console_error!("Transfer: malformed pigeon id {pid} in flock {flock_id}");
            return Response::error(
              "Internal Server Error: transfer aborted before completion -- retry is safe",
              500,
            )
            .unwrap()
            .with_cors(&cors);
          };
          let Ok(grant_resp) = grant_org_acl_via_do(&obj_id, &org_id_str).await else {
            console_error!("Transfer: org ACL grant dispatch failed for pigeon {pid}");
            return Response::error(
              "Internal Server Error: transfer aborted before completion -- retry is safe",
              500,
            )
            .unwrap()
            .with_cors(&cors);
          };
          if grant_resp.status_code() >= 400 {
            console_error!(
              "Transfer: org ACL grant failed ({}) for pigeon {pid}",
              grant_resp.status_code()
            );
            return Response::error(
              "Internal Server Error: transfer aborted before completion -- retry is safe",
              500,
            )
            .unwrap()
            .with_cors(&cors);
          }
        }

        let Ok(()) = crate::helpers::set_flock_org(&client, &flock_id, &payload.org_id).await
        else {
          return Response::error(
            "Internal Server Error: org access granted but flock not yet marked -- retry is safe",
            500,
          )
          .unwrap()
          .with_cors(&cors);
        };

        // Best-effort PG mirror of the new org ACL rows.
        let org_acl = PigeonAcl {
          entity_id: payload.org_id,
          role: "owner".to_string(),
        };
        for pid in &pigeon_ids {
          if let Err(e) = upsert_acl_pg_db(&client, pid, &org_acl).await {
            console_error!("External DB Sync Error for org ACL {pid}: {e}");
          }
        }

        let Ok(Some((transferred, _))) = get_flock_with_pigeons(&client, &flock_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&transferred)?.with_cors(&cors)
      },
    )
    .get_async(
      "/pigeons/:pigeon_id",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
        proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/get")
          .await?
          .with_cors(&cors)
      },
    )
    .get_async(
      "/pigeons/:pigeon_id/detail",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
        proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/detail")
          .await?
          .with_cors(&cors)
      },
    )
    .put_async("/pigeons/:pigeon_id", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let do_response = proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let pigeon = parse_do_response::<Pigeon>(do_response).await?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = update_pigeon_pg_db(client, &pigeon).await {
            console_error!("External DB Sync Error for pigeon {}: {e}", pigeon.id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&pigeon)?.with_cors(&cors)
    })
    .delete_async(
      "/pigeons/:pigeon_id",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        let do_response = proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/delete").await?;
        if do_response.status_code() >= 400 {
          return do_response.with_cors(&cors);
        }

        match get_db_client(&ctx.env).await {
          Ok(client) => {
            if let Err(e) = delete_pigeon_pg_db(client, &pigeon_id).await {
              console_error!("External DB Sync Error for pigeon {}: {e}", pigeon_id);
            }
          }
          Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
        }

        // Best-effort cleanup of this pigeon's stored log dictionary (task
        // #5) -- same fire-and-log convention as the PG sync above; a
        // leftover R2 object is unreachable anyway once the ACL rows are
        // gone (every log-dictionary route re-checks the ACL first).
        match ctx.env.bucket("FIRMWARE_BUCKET") {
          Ok(bucket) => {
            let object_key = format!("log-dictionaries/{pigeon_id}.json");
            if bucket.delete(&object_key).await.is_err() {
              console_error!("R2 log dictionary cleanup failed for {object_key}");
            }
          }
          Err(e) => console_error!("Cleanup skipped: FIRMWARE_BUCKET bind failed: {e}"),
        }

        Response::empty()?.with_cors(&cors)
      },
    )
    // --- Shadow Routes ---
    .get_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/shadow/get")
        .await?
        .with_cors(&cors)
    })
    .put_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let do_response = proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/shadow/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let shadow = parse_do_response::<PigeonShadow>(do_response).await?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = update_shadow_pg_db(client, &pigeon_id, &shadow).await {
            console_error!("External DB Sync Error for shadow {}: {e}", pigeon_id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&shadow)?.with_cors(&cors)
    })
    // --- ACL Routes ---
    .get_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/acl/list")
        .await?
        .with_cors(&cors)
    })
    .post_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let do_response = proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/acl/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let acl = parse_do_response::<PigeonAcl>(do_response).await?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = upsert_acl_pg_db(&client, &pigeon_id, &acl).await {
            console_error!("External DB Sync Error for ACL {}: {e}", pigeon_id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&acl)?.with_cors(&cors)
    })
    // --- Telemetry Routes ---
    .get_async("/pigeons/:pigeon_id/telemetry", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/telemetry/get")
        .await?
        .with_cors(&cors)
    })
    .put_async(
      "/pigeons/:pigeon_id/telemetry-endpoint",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        let do_response =
          proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/telemetry-endpoint/update").await?;
        if do_response.status_code() >= 400 {
          return do_response.with_cors(&cors);
        }

        let endpoint = parse_do_response::<Option<TelemetryEndpoint>>(do_response).await?;

        match get_db_client(&ctx.env).await {
          Ok(client) => {
            if let Err(e) =
              update_telemetry_endpoint_pg_db(client, &pigeon_id, endpoint.as_ref()).await
            {
              console_error!(
                "External DB Sync Error for telemetry endpoint {}: {e}",
                pigeon_id
              );
            }
          }
          Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
        }

        Response::from_json(&endpoint)?.with_cors(&cors)
      },
    )
    // --- Shell Route ---
    // v1: request/response diagnostic shell over the device WebSocket
    // channel, not a new persistent connection of its own. See
    // objects/pigeons.rs::execute_shell_command for relay/timeout/owner-gate
    // details.
    .post_async("/pigeons/:pigeon_id/shell", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(principal) = require_principal(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // No Postgres sync afterward -- unlike shadow/ACL/telemetry-endpoint
      // writes, a shell command's result isn't persisted state, just a
      // one-shot response the DO relays back from the device. The DO's own
      // handler already enforces owner-only auth, the "device not
      // connected"/"already in flight" 409s, and the timeout -> 504; this
      // route just forwards its response (success or error) unchanged.
      proxy_to_pigeon_do(req, &principal.user_id, principal.org_roles_header(), &obj_id, "/shell/execute")
        .await?
        .with_cors(&cors)
    })
    .get_async(
      "/pigeons/:pigeon_id/telemetry/history",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        // Extracted before the ACL-probe proxy call below consumes `req`.
        let Ok(query) = req.query::<TelemetryHistoryQuery>() else {
          return Response::error("Bad Request: Invalid query parameters", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // Authorization lives in the DO's pigeon_acl table, but the data
        // itself is in Postgres/Greptime -- check the DO first via the ACL
        // probe. `check_pigeon_authz` returns a `PigeonAccess` proof on
        // success, which is what makes `query_telemetry_history_for_pigeon`
        // below callable at all (its signature requires the proof, not a
        // bare pigeon_id).
        let authz_result =
          check_pigeon_authz(req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        let keys = query.key_list();

        // `raw=true` keeps the old flat/truncating shape, Greptime-first
        // fallback and all -- see TelemetryHistoryQuery::raw's doc comment
        // for who still needs it. The default bucketed path below skips
        // Greptime entirely: it's unconfigured everywhere but dev (see
        // CLAUDE.md's Postgres-consolidation note), and bucketing it too
        // would mean a second, parallel implementation over Greptime's own
        // SQL dialect for a store this platform no longer writes to outside
        // local dev.
        if query.raw {
          // Greptime-first, PG-fallback-on-error -- see helpers/greptime.rs
          // for the full reasoning. `greptime_origin` returning `None`
          // (unconfigured for this env) skips straight to the PG path
          // below.
          if crate::helpers::greptime_origin(&ctx.env).is_some() {
            match crate::helpers::query_greptime_history_for_pigeon(
              &ctx.env,
              &pigeon_id,
              keys.as_deref(),
              query.since,
              query.until,
            )
            .await
            {
              Ok(page) => return telemetry_history_response(page)?.with_cors(&cors),
              Err(e) => console_error!(
                "Greptime history query failed for pigeon {pigeon_id}, falling back to PG: {e}"
              ),
            }
          }

          get_db!(ctx.env, client, &cors);

          let Ok(page) = query_telemetry_history_for_pigeon(
            &client,
            &access,
            keys.as_deref(),
            query.since,
            query.until,
          )
          .await
          else {
            return Response::error("Internal Server Error", 500)
              .unwrap()
              .with_cors(&cors);
          };

          return telemetry_history_response(page)?.with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(buckets) = query_telemetry_history_buckets_for_pigeon(
          &client,
          &access,
          keys.as_deref(),
          query.since,
          query.until,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        telemetry_history_bucket_response(buckets)?.with_cors(&cors)
      },
    )
    .get_async(
      "/flocks/:flock_id/telemetry/history",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(query) = req.query::<TelemetryHistoryQuery>() else {
          return Response::error("Bad Request: Invalid query parameters", 400)
            .unwrap()
            .with_cors(&cors);
        };

        // Flocks have no per-entity ACL table (unlike pigeons) --
        // authorization goes through `authorize_flock` (helpers/orgs.rs),
        // which returns the FlockAccess proof the query helpers below
        // require. This PG round-trip is needed regardless of whether
        // Greptime is configured -- Greptime has no pigeons/flocks tables
        // to resolve membership/ownership from.
        get_db!(ctx.env, client, &cors);

        let Ok(access) = crate::helpers::authorize_flock(
          &client,
          &flock_id,
          &principal,
          crate::helpers::FlockAction::View,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        let Some(flock_access) = access else {
          return Response::error("Forbidden: You do not have access to this flock", 403)
            .unwrap()
            .with_cors(&cors);
        };

        let keys = query.key_list();

        // Same raw/bucketed split as the pigeon-scoped route above -- see
        // its comment for why the bucketed default skips Greptime.
        if query.raw {
          if crate::helpers::greptime_origin(&ctx.env).is_some() {
            match crate::helpers::get_flock_pigeon_ids(&client, &flock_access).await {
              Ok(pigeon_ids) => {
                match crate::helpers::query_greptime_history_for_pigeons(
                  &ctx.env,
                  &pigeon_ids,
                  keys.as_deref(),
                  query.since,
                  query.until,
                )
                .await
                {
                  Ok(page) => return telemetry_history_response(page)?.with_cors(&cors),
                  Err(e) => console_error!(
                    "Greptime flock history query failed for flock {flock_id}, falling back to PG: {e}"
                  ),
                }
              }
              Err(e) => console_error!(
                "Flock pigeon-id lookup failed for {flock_id}, falling back to PG: {e}"
              ),
            }
          }

          let Ok(page) = query_telemetry_history_for_flock(
            &client,
            &flock_access,
            keys.as_deref(),
            query.since,
            query.until,
          )
          .await
          else {
            return Response::error("Internal Server Error", 500)
              .unwrap()
              .with_cors(&cors);
          };

          return telemetry_history_response(page)?.with_cors(&cors);
        }

        let Ok(buckets) = query_telemetry_history_buckets_for_flock(
          &client,
          &flock_access,
          keys.as_deref(),
          query.since,
          query.until,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        telemetry_history_bucket_response(buckets)?.with_cors(&cors)
      },
    )
    // --- Public Demo Routes ---
    // Deliberately unauthenticated -- read-only, gated by exact membership
    // in DEMO_PIGEON_IDS (helpers::is_demo_pigeon) rather than any
    // session/ACL/device-token check. An unallowlisted pigeon_id 404s,
    // matching how authenticated pigeon routes 404 rather than 403 on an
    // unknown id -- this surface must not leak which ids exist.
    .get_async("/demo/pigeons/:pigeon_id/telemetry", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);

      let Some(pigeon_id) = ctx.param("pigeon_id").cloned() else {
        return Response::error("Not Found", 404).unwrap().with_cors(&cors);
      };
      if !is_demo_pigeon(&ctx.env, &pigeon_id) {
        return Response::error("Not Found", 404).unwrap().with_cors(&cors);
      }

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // No X-User-Id here -- the allowlist check above is the
      // authorization; the DO's /pigeon/demo/telemetry handler skips its
      // own ACL check accordingly (objects/pigeons.rs::
      // get_telemetry_latest_demo).
      proxy_to_pigeon_do(req, "", None, &obj_id, "/demo/telemetry")
        .await?
        .with_cors(&cors)
    })
    .get_async(
      "/demo/pigeons/:pigeon_id/telemetry/history",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);

        let Some(pigeon_id) = ctx.param("pigeon_id").cloned() else {
          return Response::error("Not Found", 404).unwrap().with_cors(&cors);
        };
        if !is_demo_pigeon(&ctx.env, &pigeon_id) {
          return Response::error("Not Found", 404).unwrap().with_cors(&cors);
        }

        let Ok(query) = req.query::<TelemetryHistoryQuery>() else {
          return Response::error("Bad Request: Invalid query parameters", 400)
            .unwrap()
            .with_cors(&cors);
        };

        // No DO round-trip needed for authorization here (contrast the
        // authenticated route's check_pigeon_authz probe) -- the allowlist
        // check above already is the proof. from_demo_allowlist exists
        // specifically so query_telemetry_history_for_pigeon still can't
        // be called from a call site that skipped some form of check.
        let access = PigeonAccess::from_demo_allowlist(&pigeon_id);
        let keys = query.key_list();

        // Same raw/bucketed split as the authenticated pigeon-scoped route
        // -- the demo pigeon is the exact case that motivated bucketing
        // (5 keys at 30s, ~3.5h drawable under the old truncate-at-5000
        // shape against a page that asks for 6h -- see capsules'
        // TELEMETRY_HISTORY_BUCKET_TARGET doc comment).
        if query.raw {
          if crate::helpers::greptime_origin(&ctx.env).is_some() {
            match crate::helpers::query_greptime_history_for_pigeon(
              &ctx.env,
              &pigeon_id,
              keys.as_deref(),
              query.since,
              query.until,
            )
            .await
            {
              Ok(page) => return telemetry_history_response(page)?.with_cors(&cors),
              Err(e) => console_error!(
                "Greptime demo history query failed for pigeon {pigeon_id}, falling back to PG: {e}"
              ),
            }
          }

          get_db!(ctx.env, client, &cors);

          let Ok(page) = query_telemetry_history_for_pigeon(
            &client,
            &access,
            keys.as_deref(),
            query.since,
            query.until,
          )
          .await
          else {
            return Response::error("Internal Server Error", 500)
              .unwrap()
              .with_cors(&cors);
          };

          return telemetry_history_response(page)?.with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(buckets) = query_telemetry_history_buckets_for_pigeon(
          &client,
          &access,
          keys.as_deref(),
          query.since,
          query.until,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        telemetry_history_bucket_response(buckets)?.with_cors(&cors)
      },
    )
    // Lets the demo page draw its threshold line from the alert that is
    // really enforcing it rather than from a number typed into the page.
    //
    // Answers with `DemoAlert`, never `AlertDefinition` -- see that type's
    // doc comment and `list_demo_pigeon_alerts`. The definition carries a
    // recipient email address and the owner's account UUID, and this route
    // has no session to withhold them from.
    .get_async("/demo/pigeons/:pigeon_id/alerts", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);

      let Some(pigeon_id) = ctx.param("pigeon_id").cloned() else {
        return Response::error("Not Found", 404).unwrap().with_cors(&cors);
      };
      if !is_demo_pigeon(&ctx.env, &pigeon_id) {
        return Response::error("Not Found", 404).unwrap().with_cors(&cors);
      }

      // Alerts live only in Postgres, so unlike the demo telemetry route
      // beside this one there is no DO to proxy to. Same allowlist-as-proof
      // construction the demo history route uses.
      let access = PigeonAccess::from_demo_allowlist(&pigeon_id);

      get_db!(ctx.env, client, &cors);

      let Ok(alerts) = list_demo_pigeon_alerts(&client, &access).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      Response::from_json(&alerts)?.with_cors(&cors)
    })
    // --- Firmware Routes ---
    .post_async(
      "/flocks/:flock_id/firmware",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        // `board` is required, same as `version` -- every new upload must
        // declare the Zephyr `CONFIG_BOARD_TARGET` it was built for, so
        // `objects/pigeons.rs::check_firmware_board_compat` has something
        // to enforce against.
        let Ok(query) = req.query::<FirmwareUploadQuery>() else {
          return Response::error(
            "Bad Request: Missing 'version' or 'board' query parameter",
            400,
          )
          .unwrap()
          .with_cors(&cors);
        };

        if query.version.trim().is_empty() {
          return Response::error("Bad Request: 'version' cannot be empty", 400)
            .unwrap()
            .with_cors(&cors);
        }

        if query.board.trim().is_empty() {
          return Response::error("Bad Request: 'board' cannot be empty", 400)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(bytes) = req.bytes().await else {
          return Response::error("Bad Request: Failed to read body", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if bytes.is_empty() {
          return Response::error("Bad Request: Empty firmware image", 400)
            .unwrap()
            .with_cors(&cors);
        }

        if bytes.len() > capsules::MAX_FIRMWARE_BYTES {
          return Response::error("Payload Too Large: Firmware image exceeds size cap", 413)
            .unwrap()
            .with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        // authorize_flock (helpers/orgs.rs) is the single flock-authz
        // helper -- Manage = personal-flock owner, or org owner/admin on
        // an org-owned flock. `upsert_flock_firmware` doesn't need the
        // returned proof itself, so it's discarded here.
        let Ok(owner) = crate::helpers::authorize_flock(
          &client,
          &flock_id,
          &principal,
          crate::helpers::FlockAction::Manage,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        if owner.is_none() {
          return Response::error("Forbidden: Only flock managers can upload firmware", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let sha256 = sha256_hex(&bytes);

        let Ok(bucket) = ctx.env.bucket("FIRMWARE_BUCKET") else {
          console_error!("Failed to bind FIRMWARE_BUCKET");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        // Content-addressed: identical bytes always land at the same R2
        // key regardless of flock or version label, so re-uploading the
        // same binary is a cheap no-op write, not a duplicate.
        if bucket
          .put(format!("firmware/{sha256}.bin"), bytes.clone())
          .execute()
          .await
          .is_err()
        {
          console_error!("R2 firmware upload failed for sha256 {sha256}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(image) = upsert_flock_firmware(
          &client,
          &flock_id,
          &query.version,
          bytes.len() as i64,
          &sha256,
          &query.board,
        )
        .await
        else {
          console_error!("Firmware catalog insert failed for flock {flock_id}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&image)?.with_cors(&cors)
      },
    )
    .get_async(
      "/flocks/:flock_id/firmware",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        // View-level: any org member may browse an org flock's firmware
        // catalog (metadata only); a personal flock stays owner-only.
        let Ok(owner) = crate::helpers::authorize_flock(
          &client,
          &flock_id,
          &principal,
          crate::helpers::FlockAction::View,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        // `list_flock_firmware` requires a `FlockAccess` proof rather than
        // a bare `flock_id`, so the check's `Some` case is threaded
        // straight through instead of being collapsed back into a `bool`
        // first.
        let Some(flock_access) = owner else {
          return Response::error("Forbidden: You do not have access to this flock", 403)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(images) = list_flock_firmware(&client, &flock_access).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&images)?.with_cors(&cors)
      },
    )
    // --- Log Dictionary Routes ---
    // Per-pigeon storage of a firmware build's log_dictionary.json so the
    // dashboard's log viewer can decode this pigeon's dictionary-encoded
    // ring-buffer chunks (GET /pigeons/:id/logs) client-side. Stored in R2
    // under the existing FIRMWARE_BUCKET binding at
    // `log-dictionaries/<pigeon_id>.json` -- per-pigeon, not per-flock,
    // because a dictionary only decodes the exact build that produced it,
    // and pigeons in one flock can run different builds. Member-gated (any
    // ACL row); the check runs via the DO's /pigeon/authz/check probe
    // (check_pigeon_authz) since the data itself lives in R2, not the DO.
    // The R2 key is derived from the PigeonAccess proof, so an unchecked
    // pigeon_id can never name the object.
    .put_async(
      "/pigeons/:pigeon_id/log-dictionary",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // Cloned before the body is read below -- same rationale as
        // POST /pigeons/:pigeon_id/alerts: the ACL probe consumes the
        // clone's body (which it never inspects), leaving the original
        // intact for the dictionary payload read.
        let Ok(authz_req) = req.clone() else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let authz_result =
          check_pigeon_authz(authz_req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        let Ok(bytes) = req.bytes().await else {
          return Response::error("Bad Request: Failed to read body", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if bytes.is_empty() {
          return Response::error("Bad Request: Empty log dictionary", 400)
            .unwrap()
            .with_cors(&cors);
        }

        if bytes.len() > capsules::MAX_LOG_DICTIONARY_BYTES {
          return Response::error("Payload Too Large: Log dictionary exceeds size cap", 413)
            .unwrap()
            .with_cors(&cors);
        }

        // Must be a real JSON document (Zephyr's database_gen.py output) --
        // the viewer parses it client-side, but rejecting garbage here
        // keeps the stored object always-parseable and lets this route
        // report the build_id/version it found.
        let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
          return Response::error("Bad Request: Body is not valid JSON", 400)
            .unwrap()
            .with_cors(&cors);
        };

        // `build_id` is a string in current Zephyr databases but has been
        // an integer in older tooling -- stringify whatever's there.
        let build_id = doc.get("build_id").and_then(|v| match v {
          serde_json::Value::String(s) => Some(s.clone()),
          serde_json::Value::Number(n) => Some(n.to_string()),
          _ => None,
        });
        let version = doc.get("version").and_then(|v| v.as_i64());

        let Ok(bucket) = ctx.env.bucket("FIRMWARE_BUCKET") else {
          console_error!("Failed to bind FIRMWARE_BUCKET");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let object_key = format!("log-dictionaries/{}.json", access.pigeon_id());
        if bucket.put(&object_key, bytes.clone()).execute().await.is_err() {
          console_error!("R2 log dictionary upload failed for {object_key}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let info = capsules::LogDictionaryInfo {
          size: bytes.len() as i64,
          build_id,
          version,
        };

        Response::from_json(&info)?.with_cors(&cors)
      },
    )
    .get_async(
      "/pigeons/:pigeon_id/log-dictionary",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // GET carries no body, so the original request feeds the ACL probe
        // directly (same as GET /pigeons/:pigeon_id/alerts).
        let authz_result =
          check_pigeon_authz(req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        let Ok(bucket) = ctx.env.bucket("FIRMWARE_BUCKET") else {
          console_error!("Failed to bind FIRMWARE_BUCKET");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let object_key = format!("log-dictionaries/{}.json", access.pigeon_id());
        let Ok(Some(object)) = bucket.get(&object_key).execute().await else {
          return Response::error("Not Found: No log dictionary uploaded for this pigeon", 404)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(body) = object.body() else {
          console_error!("R2 object body unexpectedly absent for {object_key}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(response_body) = body.response_body() else {
          console_error!("Failed to build streamed response body for {object_key}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let headers = Headers::new();
        let mut ok = headers.set("Content-Type", "application/json").is_ok();
        ok &= headers
          .set("Content-Length", &object.size().to_string())
          .is_ok();
        if !ok {
          console_error!("Failed to set log dictionary response headers");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(builder) = ResponseBuilder::new().with_headers(headers).with_cors(&cors) else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Ok(builder.body(response_body))
      },
    )
    .delete_async(
      "/pigeons/:pigeon_id/log-dictionary",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        let authz_result =
          check_pigeon_authz(req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        let Ok(bucket) = ctx.env.bucket("FIRMWARE_BUCKET") else {
          console_error!("Failed to bind FIRMWARE_BUCKET");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        // R2 delete is idempotent -- deleting an absent key succeeds, so a
        // double-delete (or a delete before any upload) is a clean 200, not
        // an error worth distinguishing.
        let object_key = format!("log-dictionaries/{}.json", access.pigeon_id());
        if bucket.delete(&object_key).await.is_err() {
          console_error!("R2 log dictionary delete failed for {object_key}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        Response::empty()?.with_cors(&cors)
      },
    )
    // --- Alert Routes ---
    // Owner-gated CRUD for user-defined alert definitions. Scope (pigeon
    // vs. flock) is implied by which of the two create/list route pairs
    // was hit, never trusted from the request body -- see
    // capsules::AlertDefinitionCreateRequest's doc comment. Update/delete
    // are flat `/alerts/:alert_id` routes gated by `is_alert_owner` (a
    // direct `alert_definitions.user_id` check), since an alert's owner is
    // unambiguous regardless of its scope.
    .post_async(
      "/pigeons/:pigeon_id/alerts",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        // Full principal: the ACL probe needs the org-membership set, and
        // channel validation below needs the identity's verified
        // addresses.
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        let user_id = principal.user_id.clone();

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // Cloned before the body is read below -- `check_pigeon_authz`
        // proxies to the DO's bare ACL probe route, which forwards
        // whatever body the request carries even though it never inspects
        // it. Cloning lets the probe consume that copy's body without
        // disturbing the original `req`, still needed for the create
        // payload below.
        let Ok(authz_req) = req.clone() else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let authz_result =
          check_pigeon_authz(authz_req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        let Ok(payload) = req.json::<AlertDefinitionCreateRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if payload.name.trim().is_empty() {
          return Response::error("Bad Request: 'name' cannot be empty", 400)
            .unwrap()
            .with_cors(&cors);
        }

        if let Err(msg) = validate_alert_channel(&payload.channel, &principal.verified_emails) {
          return Response::error(msg, 400).unwrap().with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        // Alert-count entitlement. The limit is per account across every
        // flock and pigeon it owns, not per pigeon.
        if let EntitlementCap::Refuse(message) = check_pigeon_alert_cap(&client, &access).await {
          return Response::error(message, 403).unwrap().with_cors(&cors);
        }

        let Ok(alert) = create_pigeon_alert(&client, &access, &user_id, &payload).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&alert)?
          .with_status(201)
          .with_cors(&cors)
      },
    )
    .get_async(
      "/pigeons/:pigeon_id/alerts",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // GET carries no body, so the original `req` can be reused directly
        // for the ACL probe (unlike the POST route above).
        let authz_result =
          check_pigeon_authz(req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        get_db!(ctx.env, client, &cors);

        let Ok(alerts) = list_pigeon_alerts(&client, &access).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&alerts)?.with_cors(&cors)
      },
    )
    // Current fired/ok status per alert (gap G3) -- deliberately a
    // separate route rather than folded into the definitions list above:
    // a flock-scoped alert can carry several `AlertState` rows (one per
    // pigeon it currently fires/clears for, see the type's own doc
    // comment), so there is no single state value to attach to one
    // `AlertDefinition`. Same auth as the list route it sits beside.
    .get_async(
      "/pigeons/:pigeon_id/alerts/state",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        let authz_result =
          check_pigeon_authz(req, &principal.user_id, principal.org_roles_header(), &obj_id, &pigeon_id).await?;
        let access = match authz_result {
          Ok(access) => access,
          Err(resp) => return resp.with_cors(&cors),
        };

        get_db!(ctx.env, client, &cors);

        let Ok(states) = list_pigeon_alert_state(&client, &access).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&states)?.with_cors(&cors)
      },
    )
    .post_async(
      "/flocks/:flock_id/alerts",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        // Full principal for org-aware flock authz + verified-address
        // channel validation.
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        let user_id = principal.user_id.clone();

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<AlertDefinitionCreateRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if payload.name.trim().is_empty() {
          return Response::error("Bad Request: 'name' cannot be empty", 400)
            .unwrap()
            .with_cors(&cors);
        }

        if let Err(msg) = validate_alert_channel(&payload.channel, &principal.verified_emails) {
          return Response::error(msg, 400).unwrap().with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(owner) = crate::helpers::authorize_flock(
          &client,
          &flock_id,
          &principal,
          crate::helpers::FlockAction::Manage,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_access) = owner else {
          return Response::error("Forbidden: Only flock managers can create alerts", 403)
            .unwrap()
            .with_cors(&cors);
        };

        // Alert-count entitlement, same per-account limit as the
        // pigeon-scoped route.
        if let EntitlementCap::Refuse(message) = check_flock_alert_cap(&client, &flock_access).await
        {
          return Response::error(message, 403).unwrap().with_cors(&cors);
        }

        let Ok(alert) = create_flock_alert(&client, &flock_access, &user_id, &payload).await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&alert)?
          .with_status(201)
          .with_cors(&cors)
      },
    )
    .get_async(
      "/flocks/:flock_id/alerts",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(owner) = crate::helpers::authorize_flock(
          &client,
          &flock_id,
          &principal,
          crate::helpers::FlockAction::View,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_access) = owner else {
          return Response::error("Forbidden: You do not have access to this flock", 403)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(alerts) = list_flock_alerts(&client, &flock_access).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&alerts)?.with_cors(&cors)
      },
    )
    // Flock counterpart of the pigeon `/alerts/state` route above -- same
    // reasoning (a flock-scoped alert's state is per-pigeon, not
    // per-definition) and the same `FlockAction::View` auth as the
    // definitions list it sits beside.
    .get_async(
      "/flocks/:flock_id/alerts/state",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(principal) = require_principal(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(owner) = crate::helpers::authorize_flock(
          &client,
          &flock_id,
          &principal,
          crate::helpers::FlockAction::View,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_access) = owner else {
          return Response::error("Forbidden: You do not have access to this flock", 403)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(states) = list_flock_alert_state(&client, &flock_access).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&states)?.with_cors(&cors)
      },
    )
    .put_async(
      "/alerts/:alert_id",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        // Full session for verified-address channel validation.
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        let user_id = auth.user_id.clone();

        let Some(alert_id) = ctx.param("alert_id").cloned() else {
          return Response::error("Alert ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<AlertDefinitionUpdateRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if let Some(channel) = &payload.channel
          && let Err(msg) = validate_alert_channel(channel, &auth.verified_emails)
        {
          return Response::error(msg, 400).unwrap().with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(owner) = is_alert_owner(&client, &alert_id, &user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(alert_access) = owner else {
          return Response::error("Forbidden: Only the alert owner can update it", 403)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(alert) = update_alert_definition(&client, &alert_access, &payload).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&alert)?.with_cors(&cors)
      },
    )
    .delete_async(
      "/alerts/:alert_id",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        // Alert ownership is a direct user-id check (is_alert_owner) --
        // no org involvement, so the plain session id suffices here.
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(alert_id) = ctx.param("alert_id").cloned() else {
          return Response::error("Alert ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(owner) = is_alert_owner(&client, &alert_id, &user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(alert_access) = owner else {
          return Response::error("Forbidden: Only the alert owner can delete it", 403)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(()) = delete_alert_definition(&client, &alert_access).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::empty()?.with_cors(&cors)
      },
    )
    // --- Feedback Route ---
    // Public, optionally-authenticated: the feedback form is linked from
    // marketing pages too, so no session is required, but a present Kratos
    // session is resolved server-side and included in the notification
    // email (never trusted from the body). Abuse protection is
    // deliberately existing-pattern-only: Content-Type must be JSON, body
    // and each field are size-capped (capsules::MAX_FEEDBACK_*), and
    // delivery reuses the prod-only OPS_ALERT_EMAIL + RESEND_API_KEY pair,
    // so staging/dev degrade to a logged no-op. No per-IP rate limiter
    // here -- that's platform-level (a Cloudflare WAF rule or Turnstile),
    // not something to hand-roll in-route.
    .post_async("/feedback", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);

      let is_json = req
        .headers()
        .get("Content-Type")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
      if !is_json {
        return Response::error("Bad Request: Content-Type must be application/json", 400)
          .unwrap()
          .with_cors(&cors);
      }

      // Optional session -- anonymous feedback is allowed, so a failed
      // session check just means "no submitter context", never a 401.
      let submitter = require_auth_session(&req, &ctx.env)
        .await
        .ok()
        .map(|auth| capsules::FeedbackSubmitter {
          user_id: auth.user_id,
          email: auth.email,
        });

      // Raw-body cap checked before JSON parsing even starts, so oversized
      // garbage is rejected without being deserialized.
      let Ok(body_text) = req.text().await else {
        return Response::error("Bad Request: Failed to read body", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if body_text.len() > capsules::MAX_FEEDBACK_BODY_BYTES {
        return Response::error("Payload Too Large: Feedback body exceeds size cap", 413)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(payload) = serde_json::from_str::<capsules::FeedbackRequest>(&body_text) else {
        return Response::error("Bad Request: Invalid JSON payload", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if payload.message.trim().is_empty() {
        return Response::error("Bad Request: 'message' cannot be empty", 400)
          .unwrap()
          .with_cors(&cors);
      }

      if payload.message.len() > capsules::MAX_FEEDBACK_MESSAGE_BYTES {
        return Response::error("Payload Too Large: 'message' exceeds size cap", 413)
          .unwrap()
          .with_cors(&cors);
      }

      if payload
        .contact_email
        .as_ref()
        .is_some_and(|e| e.len() > capsules::MAX_FEEDBACK_CONTACT_EMAIL_BYTES)
      {
        return Response::error("Bad Request: 'contact_email' is too long", 400)
          .unwrap()
          .with_cors(&cors);
      }

      if payload
        .page_context
        .as_ref()
        .is_some_and(|p| p.len() > capsules::MAX_FEEDBACK_PAGE_CONTEXT_BYTES)
      {
        return Response::error("Bad Request: 'page_context' is too long", 400)
          .unwrap()
          .with_cors(&cors);
      }

      if payload
        .diagnostics
        .as_ref()
        .is_some_and(|d| d.len() > capsules::MAX_FEEDBACK_DIAGNOSTICS_BYTES)
      {
        return Response::error("Bad Request: 'diagnostics' is too long", 400)
          .unwrap()
          .with_cors(&cors);
      }

      let (subject, text) = capsules::format_feedback_email(
        &payload,
        submitter.as_ref(),
        time::OffsetDateTime::now_utc(),
      );

      // Fire-and-log, same convention as every other notification path --
      // the submitter's 202 never depends on delivery succeeding (and in
      // staging/dev, where OPS_ALERT_EMAIL is unset by design, delivery is
      // a logged no-op).
      send_feedback_email(&ctx.env, &subject, &text).await;

      Response::ok("{}").unwrap().with_status(202).with_cors(&cors)
    })
    // --- Contact Route ---
    // The public "talk to us" form (`/contact/` in fancier). Public and
    // unauthenticated by definition -- the people it exists for do not
    // have accounts yet -- so it carries real abuse controls rather than
    // the size caps alone that `POST /feedback` gets away with: a per-IP
    // limiter, a honeypot field, and a minimum fill time
    // (`capsules::contact::validate`). A session is resolved if one
    // happens to be present, purely so an enquiry from an existing user
    // is recognisable, and never trusted from the body.
    //
    // Unlike /feedback this route PERSISTS before it notifies. A contact
    // form that answers 202 and then loses the message to a mail-transport
    // outage drops business the sender believes reached us.
    .post_async("/contact", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);

      // Cloudflare's rate-limiter binding, keyed on the connecting
      // address; the key is checked and discarded, never stored. Over the
      // limit answers 429 -- never 401, which the dashboard treats as
      // "session gone" and would sign a signed-in visitor out for using a
      // marketing page. A limiter fault fails open for the same reason
      // `POST /errors` does, weighed the same way: one enquiry lost costs
      // more than one window of junk that the honeypot and fill-time
      // floor still have to get past.
      match ctx.env.rate_limiter("CONTACT_LIMITER") {
        Ok(limiter) => {
          let key = req
            .headers()
            .get("CF-Connecting-IP")
            .ok()
            .flatten()
            .unwrap_or_default();
          match limiter.limit(key).await {
            Ok(outcome) if !outcome.success => {
              return Response::error("Too Many Requests", 429)
                .unwrap()
                .with_cors(&cors);
            }
            Ok(_) => {}
            Err(e) => console_error!("contact: rate limiter check failed (failing open): {e}"),
          }
        }
        Err(e) => console_error!("contact: rate limiter binding unavailable (failing open): {e}"),
      }

      // TURNSTILE SEAM. Cloudflare Turnstile is the right long-term
      // control here and needs a site key that only the account owner can
      // mint, so it is deliberately not wired up. Slotting it in is three
      // edits and belongs exactly here, after the limiter and before the
      // body is read: add `turnstile_token: Option<String>` to
      // `capsules::ContactRequest`, render the widget in fancier's
      // `views/contact.rs` and put its token in that field, then POST the
      // token plus `CF-Connecting-IP` to
      // https://challenges.cloudflare.com/turnstile/v0/siteverify with a
      // `TURNSTILE_SECRET` Worker secret and return 403 unless the
      // response's `success` is true. Nothing below needs to change.

      let is_json = req
        .headers()
        .get("Content-Type")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
      if !is_json {
        return Response::error("Bad Request: Content-Type must be application/json", 400)
          .unwrap()
          .with_cors(&cors);
      }

      // Raw-body cap checked before JSON parsing starts, so oversized
      // garbage is rejected without being deserialized.
      let Ok(body_text) = req.text().await else {
        return Response::error("Bad Request: Failed to read body", 400)
          .unwrap()
          .with_cors(&cors);
      };
      if body_text.len() > capsules::MAX_CONTACT_BODY_BYTES {
        return Response::error("Payload Too Large: Contact body exceeds size cap", 413)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(payload) = serde_json::from_str::<capsules::ContactRequest>(&body_text) else {
        return Response::error("Bad Request: Invalid JSON payload", 400)
          .unwrap()
          .with_cors(&cors);
      };

      // One definition of valid, shared with the form that produced this
      // (capsules::contact::validate) -- including the honeypot and the
      // fill-time floor, so no abuse control here can be one the client
      // knows nothing about or vice versa.
      if let Err(rejection) = capsules::contact::validate(&payload) {
        let status = rejection.status();
        if status < 400 {
          // The honeypot. Answering exactly like a success is the point:
          // telling a script which control caught it tells it what to
          // change. Nothing is stored and nothing is emailed.
          console_log!("contact: honeypot tripped, submission dropped");
          return Response::ok("{}")
            .unwrap()
            .with_status(status)
            .with_cors(&cors);
        }
        return Response::error(rejection.message(), status)
          .unwrap()
          .with_cors(&cors);
      }

      // Optional session, same as /feedback: a signed-out visitor's
      // enquiry is stored without an identity, and a failed session check
      // is never a 401.
      let user_id = require_auth_session(&req, &ctx.env)
        .await
        .ok()
        .and_then(|auth| uuid::Uuid::parse_str(&auth.user_id).ok());

      get_db!(ctx.env, client, &cors);
      let Ok(submission_id) = store_contact_submission(&client, &payload, user_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      let (subject, text) =
        capsules::format_contact_email(&payload, time::OffsetDateTime::now_utc());

      // Fire-and-log, same convention as every other notification path.
      // Safe to be best-effort only because the row above is not: the
      // enquiry is already durable, and `notified_at` stays NULL until a
      // send actually succeeds.
      notify_contact_submission(&ctx.env, &client, submission_id, &subject, &text).await;

      Response::ok("{}").unwrap().with_status(202).with_cors(&cors)
    })
    // --- Error Reporting Routes ---
    // Ingest for the dashboard's crash/error reports (docs/api.md "Error
    // reporting"). Two content types, and the split carries the identity
    // policy: `text/plain` is what `sendBeacon` can send from a dying wasm
    // module without a CORS preflight -- but text/plain is CORS-safelisted,
    // so ANY page on the internet can POST it here cross-origin with
    // cookies attached; that branch therefore never resolves a session and
    // its envelope rejects unknown fields, making automatic reports
    // anonymous by construction. `application/json` is not safelisted --
    // the preflight gates it to ROOT_URL -- so it alone may carry the
    // identified manual note, with identity resolved from the session and
    // never from the body.
    .post_async("/errors", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);

      // Cloudflare's rate-limiter binding, keyed on the connecting
      // address; the key is checked and discarded, never stored. Over the
      // limit answers 429 -- and never 401, because the dashboard treats
      // 401 as "session gone" and signs the tab out, which must not happen
      // to someone mid-bug-report. A limiter fault fails open: dropping
      // real crash reports costs more than a window of unthrottled junk.
      match ctx.env.rate_limiter("ERROR_INGEST_LIMITER") {
        Ok(limiter) => {
          let key = req
            .headers()
            .get("CF-Connecting-IP")
            .ok()
            .flatten()
            .unwrap_or_default();
          match limiter.limit(key).await {
            Ok(outcome) if !outcome.success => {
              return Response::error("Too Many Requests", 429)
                .unwrap()
                .with_cors(&cors);
            }
            Ok(_) => {}
            Err(e) => console_error!("error ingest: rate limiter check failed (failing open): {e}"),
          }
        }
        Err(e) => {
          console_error!("error ingest: rate limiter binding unavailable (failing open): {e}")
        }
      }

      let content_type = req
        .headers()
        .get("Content-Type")
        .ok()
        .flatten()
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_default();
      let is_json = content_type.starts_with("application/json");
      let is_text = content_type.starts_with("text/plain");
      if !is_json && !is_text {
        return Response::error(
          "Bad Request: Content-Type must be text/plain or application/json",
          400,
        )
        .unwrap()
        .with_cors(&cors);
      }

      let Ok(body_text) = req.text().await else {
        return Response::error("Bad Request: Failed to read body", 400)
          .unwrap()
          .with_cors(&cors);
      };
      if body_text.len() > capsules::MAX_ERROR_REPORT_BYTES {
        return Response::error("Payload Too Large: report exceeds size cap", 413)
          .unwrap()
          .with_cors(&cors);
      }

      let (report, user_id, note) = if is_json {
        let Ok(payload) = serde_json::from_str::<capsules::ErrorNoteRequest>(&body_text) else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };
        let note = payload.note.trim().to_string();
        if note.is_empty() {
          return Response::error("Bad Request: 'note' cannot be empty", 400)
            .unwrap()
            .with_cors(&cors);
        }
        if note.len() > capsules::MAX_FEEDBACK_MESSAGE_BYTES {
          return Response::error("Payload Too Large: 'note' exceeds size cap", 413)
            .unwrap()
            .with_cors(&cors);
        }
        // Optional session, same as /feedback: a signed-out visitor's note
        // is stored anonymously; a failed session check is never a 401.
        let user_id = require_auth_session(&req, &ctx.env)
          .await
          .ok()
          .and_then(|auth| uuid::Uuid::parse_str(&auth.user_id).ok());
        (payload.report, user_id, Some(note))
      } else {
        // Anonymous automatic branch: structurally cookie-blind -- no code
        // path here reads the session, and `ErrorReport`'s
        // deny_unknown_fields makes a body claiming an identity fail to
        // parse rather than be partially honored.
        let Ok(report) = serde_json::from_str::<capsules::ErrorReport>(&body_text) else {
          return Response::error("Bad Request: Invalid report payload", 400)
            .unwrap()
            .with_cors(&cors);
        };
        (report, None, None)
      };

      get_db!(ctx.env, client, &cors);
      let Ok(()) = ingest_error_report(&ctx.env, &client, &report, user_id, note.as_deref()).await
      else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      Response::ok("{}").unwrap().with_status(202).with_cors(&cors)
    })
    // Erasure of the caller's identified report rows -- the automatic rows
    // never carried an identity to erase. The manual account-deletion
    // runbook runs the same statement directly (see the migration file).
    .delete_async("/errors", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      let Ok(user_uuid) = uuid::Uuid::parse_str(&auth.user_id) else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);
      let Ok(deleted) = erase_user_error_reports(&client, &user_uuid).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      Response::from_json(&serde_json::json!({ "deleted": deleted }))
        .unwrap()
        .with_cors(&cors)
    })
    // --- Organization Routes ---
    // Shared-org access for teams: individual Kratos accounts, org-level
    // RBAC, membership-row revocation. Authorization funnels through
    // helpers/orgs.rs::org_role_of + the per-route minimum-role rules
    // below; the full permission matrix lives in docs/api.md's
    // "Organizations" section. Member listing is part of GET /orgs/:org_id
    // (no separate members GET).
    .post_async("/orgs", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(payload) = req.json::<OrganizationCreateRequest>().await else {
        return Response::error("Bad Request: Invalid JSON payload", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if payload.name.trim().is_empty() {
        return Response::error("Bad Request: organization name cannot be empty", 400)
          .unwrap()
          .with_cors(&cors);
      }

      get_db!(ctx.env, client, &cors);
      // The org insert runs in a transaction, which needs the client
      // mutably; the details write below then reuses the same connection.
      let mut client = client;

      // Organization-count entitlement. Counts the organizations this
      // person already owns, so being a member of somebody else's spends
      // none of their own allowance.
      if let EntitlementCap::Refuse(message) = check_org_cap(&client, &auth.user_id).await {
        return Response::error(message, 403).unwrap().with_cors(&cors);
      }

      // Business details are optional here, and settled BEFORE the org is
      // inserted: a VAT ID that VIES definitively rejects should refuse the
      // whole creation rather than leave an org behind carrying a number we
      // already know is wrong. A VIES outage is not a rejection -- the plan
      // comes back `pending` and the sweep finishes the job.
      //
      // After the entitlement gate, not before it: a request that is going
      // to be refused anyway must not spend a VIES call, which would also
      // make org creation a VAT-lookup oracle for anyone past their cap.
      let details_request = OrganizationBusinessDetailsRequest {
        business_name: payload.business_name.clone(),
        tax_id: payload.tax_id.clone(),
        tax_id_type: payload.tax_id_type,
      };
      let plan = match plan_business_details(&details_request).await {
        Ok(plan) => plan,
        Err(message) => return Response::error(message, 400).unwrap().with_cors(&cors),
      };

      if !plan.is_empty() && ensure_business_details_columns(&client).await.is_err() {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(org) = create_organization(
        &mut client,
        &auth.user_id,
        auth.email.as_deref(),
        payload.name.trim(),
      )
      .await
      else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      // Best-effort, and deliberately so: the org exists and the caller
      // owns it, so failing the whole creation over a details write would
      // strand them with an org they were told they do not have. A failure
      // here leaves the details blank and editable.
      if !plan.is_empty()
        && let Err(e) = write_business_details(&client, &org.id, &plan).await
      {
        console_error!("Failed to store business details on new org {}: {e}", org.id);
      }

      let headers = Headers::new();
      if headers.set("Location", &format!("/orgs/{}", org.id)).is_err() {
        console_error!("Failed to set Location header for org {}", org.id);
      }

      Response::from_json(&org)?
        .with_status(201)
        .with_headers(headers)
        .with_cors(&cors)
    })
    .get_async("/orgs", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      let Ok(orgs) = list_user_organizations(&client, &auth.user_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      Response::from_json(&orgs)?.with_cors(&cors)
    })
    .get_async("/orgs/:org_id", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Some(org_id) = ctx
        .param("org_id")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
      else {
        return Response::error("Bad Request: invalid organization id", 400)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };
      let Some(caller_role) = caller_role else {
        return Response::error("Forbidden: not a member of this organization", 403)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(Some(organization)) = get_organization(&client, &org_id).await else {
        return Response::error("Not Found", 404).unwrap().with_cors(&cors);
      };

      let Ok(members) = list_org_members(&client, &org_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      // Pending invites are management-level information (they carry
      // invited addresses) -- plain members see an empty list.
      let invites = if caller_role.is_manager() {
        let Ok(invites) = list_org_invites(&client, &org_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        invites
      } else {
        Vec::new()
      };

      Response::from_json(&OrganizationDetail {
        organization,
        caller_role,
        members,
        invites,
      })?
      .with_cors(&cors)
    })
    .put_async("/orgs/:org_id", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Some(org_id) = ctx
        .param("org_id")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
      else {
        return Response::error("Bad Request: invalid organization id", 400)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(payload) = req.json::<OrganizationRenameRequest>().await else {
        return Response::error("Bad Request: Invalid JSON payload", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if payload.name.trim().is_empty() {
        return Response::error("Bad Request: organization name cannot be empty", 400)
          .unwrap()
          .with_cors(&cors);
      }

      get_db!(ctx.env, client, &cors);

      let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };
      if !caller_role.is_some_and(|r| r.is_manager()) {
        return Response::error("Forbidden: only owners/admins can rename an organization", 403)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(org) = rename_organization(&client, &org_id, payload.name.trim()).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      Response::from_json(&org)?.with_cors(&cors)
    })
    .delete_async("/orgs/:org_id", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Some(org_id) = ctx
        .param("org_id")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
      else {
        return Response::error("Bad Request: invalid organization id", 400)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };
      if caller_role != Some(OrgRole::Owner) {
        return Response::error("Forbidden: only an owner can delete an organization", 403)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(outcome) = delete_organization_if_empty(client, &org_id).await else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };
      if let Err(msg) = outcome {
        return Response::error(msg, 409).unwrap().with_cors(&cors);
      }

      Response::empty()?.with_cors(&cors)
    })
    .put_async(
      "/orgs/:org_id/members/:user_id",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };
        let Some(target_user) = ctx
          .param("user_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid user id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<OrganizationMemberRoleUpdateRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        // Role changes are owner-only -- the narrowest gate that's still
        // usable, matching this codebase's high-blast-radius convention
        // (an admin altering roles could otherwise promote themselves).
        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if caller_role != Some(OrgRole::Owner) {
          return Response::error("Forbidden: only an owner can change member roles", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(outcome) = change_member_role(client, &org_id, &target_user, payload.role).await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        match outcome {
          Ok(member) => Response::from_json(&member)?.with_cors(&cors),
          Err(msg) if msg.starts_with("Not Found") => {
            Response::error(msg, 404).unwrap().with_cors(&cors)
          }
          Err(msg) => Response::error(msg, 409).unwrap().with_cors(&cors),
        }
      },
    )
    .delete_async(
      "/orgs/:org_id/members/:user_id",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };
        let Some(target_user) = ctx
          .param("user_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid user id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        let Some(caller_role) = caller_role else {
          return Response::error("Forbidden: not a member of this organization", 403)
            .unwrap()
            .with_cors(&cors);
        };

        // Removal rules (docs/api.md matrix): anyone may remove
        // THEMSELVES (leave); owners may remove anyone; admins may remove
        // members/admins but never owners. Last-owner protection is
        // enforced inside remove_member regardless of who asks.
        let is_self = auth.user_id == target_user.to_string();
        if !is_self {
          if !caller_role.is_manager() {
            return Response::error("Forbidden: only owners/admins can remove members", 403)
              .unwrap()
              .with_cors(&cors);
          }
          if caller_role == OrgRole::Admin {
            let Ok(target_role) = org_role_of(&client, &org_id, &target_user.to_string()).await
            else {
              return Response::error("Internal Server Error", 500)
                .unwrap()
                .with_cors(&cors);
            };
            if target_role == Some(OrgRole::Owner) {
              return Response::error("Forbidden: admins cannot remove owners", 403)
                .unwrap()
                .with_cors(&cors);
            }
          }
        }

        let Ok(outcome) = remove_member(client, &org_id, &target_user).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        match outcome {
          Ok(()) => Response::empty()?.with_cors(&cors),
          Err(msg) if msg.starts_with("Not Found") => {
            Response::error(msg, 404).unwrap().with_cors(&cors)
          }
          Err(msg) => Response::error(msg, 409).unwrap().with_cors(&cors),
        }
      },
    )
    .post_async(
      "/orgs/:org_id/invites",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<OrganizationInviteCreateRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let email = payload.email.trim().to_lowercase();
        if email.is_empty() || !email.contains('@') {
          return Response::error("Bad Request: invalid invite email address", 400)
            .unwrap()
            .with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        let Some(caller_role) = caller_role else {
          return Response::error("Forbidden: not a member of this organization", 403)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_manager() {
          return Response::error("Forbidden: only owners/admins can invite members", 403)
            .unwrap()
            .with_cors(&cors);
        }
        // Granting the owner role is itself owner-only, same rationale as
        // the role-change route above.
        if payload.role == OrgRole::Owner && caller_role != OrgRole::Owner {
          return Response::error("Forbidden: only an owner can invite another owner", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(Some(organization)) = get_organization(&client, &org_id).await else {
          return Response::error("Not Found", 404).unwrap().with_cors(&cors);
        };

        // Seat entitlement, checked after authorization so a non-manager
        // learns they may not invite rather than how full the org is.
        // Pending invites count as spent seats -- see check_seat_cap.
        if let EntitlementCap::Refuse(message) = check_seat_cap(&client, &org_id).await {
          return Response::error(message, 403).unwrap().with_cors(&cors);
        }

        let Ok((token, token_hash)) = mint_invite_token() else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(invite) = create_invite(
          &client,
          &org_id,
          &email,
          payload.role,
          &auth.user_id,
          &token_hash,
        )
        .await
        else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let invite_url = build_invite_url(&ctx.env, &token);

        // Best-effort delivery through the existing Resend transport; in
        // dev (no RESEND_API_KEY) this logs the link instead. Either way
        // the response below carries the token/URL once -- write-once,
        // same convention as device connector tokens.
        send_invite_email(&ctx.env, &email, &org_id, &organization.name, &invite_url).await;

        Response::from_json(&OrganizationInviteCreated {
          invite,
          token,
          invite_url,
        })?
        .with_status(201)
        .with_cors(&cors)
      },
    )
    .get_async(
      "/orgs/:org_id/invites",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_some_and(|r| r.is_manager()) {
          return Response::error("Forbidden: only owners/admins can view invites", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(invites) = list_org_invites(&client, &org_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&invites)?.with_cors(&cors)
      },
    )
    .delete_async(
      "/orgs/:org_id/invites/:invite_id",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };
        let Some(invite_id) = ctx
          .param("invite_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid invite id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_some_and(|r| r.is_manager()) {
          return Response::error("Forbidden: only owners/admins can revoke invites", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(()) = revoke_invite(&client, &org_id, &invite_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::empty()?.with_cors(&cors)
      },
    )
    // Deliberately NOT under /orgs/:org_id -- the accepting user knows only
    // the token, not the org id (and must not need to). See
    // helpers/orgs.rs::accept_invite for the tradeoff.
    .post_async("/invites/accept", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(payload) = req.json::<OrganizationInviteAcceptRequest>().await else {
        return Response::error("Bad Request: Invalid JSON payload", 400)
          .unwrap()
          .with_cors(&cors);
      };

      if payload.token.trim().is_empty() {
        return Response::error("Bad Request: missing invite token", 400)
          .unwrap()
          .with_cors(&cors);
      }

      get_db!(ctx.env, client, &cors);

      let Ok(outcome) = accept_invite(
        client,
        payload.token.trim(),
        &auth.user_id,
        auth.email.as_deref(),
      )
      .await
      else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      match outcome {
        Ok(member) => Response::from_json(&member)?.with_status(201).with_cors(&cors),
        Err(msg) if msg.starts_with("Not Found") => {
          Response::error(msg, 404).unwrap().with_cors(&cors)
        }
        Err(msg) => Response::error(msg, 409).unwrap().with_cors(&cors),
      }
    })
    // Stripe's delivery target. Unauthenticated by design -- the HMAC over
    // the raw body IS the authentication, so nothing here consults a Kratos
    // session or a device token. Registered as an exact path with no
    // trailing-slash variant: Stripe counts a 3xx as a failed delivery and
    // would retry for three days against a redirect.
    // --- Billing (org-scoped) ---
    // Read side is member-visible; the mutating session mints (checkout,
    // and portal below) are manager-only, same split as the rest of the
    // org surface. Stripe hosts every payment page -- these routes only
    // mint redirect URLs, so card data never touches this Worker.
    .get_async(
      "/orgs/:org_id/billing",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if caller_role.is_none() {
          return Response::error("Forbidden: not a member of this organization", 403)
            .unwrap()
            .with_cors(&cors);
        }

        // Both lazy-DDL bootstraps: the overview reads the billing columns
        // AND the usage table, and this is a low-rate dashboard route where
        // the belt-and-suspenders round trip is cheap.
        if ensure_billing_tables(&client).await.is_err()
          || ensure_billing_usage_tables(&client).await.is_err()
        {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        match load_org_billing_overview(&client, &org_id).await {
          Ok(Some(overview)) => Response::from_json(&overview)?.with_cors(&cors),
          Ok(None) => Response::error("Not Found: no such organization", 404)
            .unwrap()
            .with_cors(&cors),
          Err(_) => Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors),
        }
      },
    )
    // --- Business details / tax identity ---
    // The org is the billing entity, so its tax registration lives here
    // rather than on the Kratos identity: one person can belong to two orgs
    // with two different registrations, and an identity trait could not
    // express that. Reading is member-visible (a VAT number is public
    // information, and a member who spots a typo should be able to say so);
    // writing is manager-only, same split as the rest of the org surface.
    .get_async(
      "/orgs/:org_id/business-details",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if caller_role.is_none() {
          return Response::error("Forbidden: not a member of this organization", 403)
            .unwrap()
            .with_cors(&cors);
        }

        if ensure_business_details_columns(&client).await.is_err() {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        match load_business_details(&client, &org_id).await {
          Ok(Some(details)) => Response::from_json(&details)?.with_cors(&cors),
          Ok(None) => Response::error("Not Found: no such organization", 404)
            .unwrap()
            .with_cors(&cors),
          Err(_) => Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors),
        }
      },
    )
    .put_async(
      "/orgs/:org_id/business-details",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<OrganizationBusinessDetailsRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_some_and(|r| r.is_manager()) {
          return Response::error(
            "Forbidden: only owners/admins can change billing details",
            403,
          )
          .unwrap()
          .with_cors(&cors);
        }

        // Authorization first, then the VIES call -- a non-member must not
        // be able to use this route as a free VAT-lookup oracle.
        let plan = match plan_business_details(&payload).await {
          Ok(plan) => plan,
          Err(message) => return Response::error(message, 400).unwrap().with_cors(&cors),
        };

        if ensure_business_details_columns(&client).await.is_err() {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        match write_business_details(&client, &org_id, &plan).await {
          Ok(Some(details)) => Response::from_json(&details)?.with_cors(&cors),
          Ok(None) => Response::error("Not Found: no such organization", 404)
            .unwrap()
            .with_cors(&cors),
          Err(_) => Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors),
        }
      },
    )
    .post_async(
      "/orgs/:org_id/billing/checkout",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<BillingCheckoutRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };
        if payload.plan == BillingPlan::Perch {
          return Response::error("Bad Request: the free tier is not purchasable", 400)
            .unwrap()
            .with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_some_and(|r| r.is_manager()) {
          return Response::error("Forbidden: only owners/admins can manage billing", 403)
            .unwrap()
            .with_cors(&cors);
        }

        if ensure_billing_tables(&client).await.is_err() {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let existing_customer = match get_org_stripe_customer(&client, &org_id).await {
          Ok(Some(existing)) => existing,
          Ok(None) => {
            return Response::error("Not Found: no such organization", 404)
              .unwrap()
              .with_cors(&cors);
          }
          Err(_) => {
            return Response::error("Internal Server Error", 500)
              .unwrap()
              .with_cors(&cors);
          }
        };

        let customer_id = match existing_customer {
          Some(id) => id,
          None => {
            let Ok(Some(org)) = get_organization(&client, &org_id).await else {
              return Response::error("Internal Server Error", 500)
                .unwrap()
                .with_cors(&cors);
            };
            let created = match create_customer(
              &ctx.env,
              &org_id.to_string(),
              &org.name,
              auth.email.as_deref(),
            )
            .await
            {
              Ok(created) => created,
              Err(e) => {
                console_error!("Stripe customer create failed for org {org_id}: {e}");
                return Response::error("Bad Gateway: billing provider unavailable", 502)
                  .unwrap()
                  .with_cors(&cors);
              }
            };
            // Keep-the-first COALESCE decides the winner if two managers
            // race; the loser's customer is an orphan on Stripe's side,
            // harmless and visible there. An attach failure still lets
            // this request proceed -- the webhook re-attaches via the
            // session's client_reference_id.
            match attach_stripe_customer(&client, &org_id, &created).await {
              Ok(Some(winner)) => winner,
              _ => created,
            }
          }
        };

        // SEAM -- tax identity is collected but not yet sent to Stripe.
        //
        // `load_business_details(&client, &org_id)` has the org's
        // `business_name`, `tax_id`, `tax_id_type` and `tax_id_status`.
        // Consuming them here means passing `customer_update[name]=auto`
        // plus `tax_id_data[0][type]`/`[value]` on the Checkout session (or
        // POSTing `/v1/customers/:id/tax_ids` before it), which is what
        // makes Stripe apply the B2B reverse charge to an EU sale instead
        // of adding VAT to it.
        //
        // Two things to settle when that is wired, neither of which the
        // storage layer decides:
        //   - WHICH statuses may be sent. `validated` clearly; `pending` is
        //     a judgment call (Stripe re-validates EU VAT itself, so
        //     forwarding a pending id is defensible and lets a customer buy
        //     during a VIES outage) and `invalid` clearly must not be.
        //   - `tax_id_type: other` has no Stripe type. Stripe's enum is
        //     jurisdiction-specific (`au_abn`, `ca_gst_hst`, `gb_vat`, ...)
        //     and nothing here records which one, so a non-EU registration
        //     needs the customer to name their jurisdiction before it can
        //     be forwarded. Storing it unforwarded is the deliberate state
        //     until then -- it is still what an invoice has to show.
        let prices = match resolve_checkout_prices(&ctx.env, payload.plan).await {
          Ok(prices) => prices,
          Err(e) => {
            console_error!("Checkout price resolution failed for org {org_id}: {e}");
            return Response::error("Bad Gateway: billing provider unavailable", 502)
              .unwrap()
              .with_cors(&cors);
          }
        };

        let root_url = ctx
          .env
          .var("ROOT_URL")
          .map(|v| v.to_string())
          .unwrap_or_else(|_| "https://pidgeiot.com".to_string());
        let success_url = format!("{root_url}/orgs/{org_id}?billing=success");
        let cancel_url = format!("{root_url}/orgs/{org_id}?billing=cancelled");

        let url = match create_checkout_session(
          &ctx.env,
          &customer_id,
          &org_id.to_string(),
          payload.plan,
          &prices,
          &success_url,
          &cancel_url,
        )
        .await
        {
          Ok(url) => url,
          Err(e) => {
            console_error!("Checkout session create failed for org {org_id}: {e}");
            return Response::error("Bad Gateway: billing provider unavailable", 502)
              .unwrap()
              .with_cors(&cors);
          }
        };

        Response::from_json(&BillingSessionUrl { url })?.with_cors(&cors)
      },
    )
    .post_async(
      "/orgs/:org_id/billing/portal",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_some_and(|r| r.is_manager()) {
          return Response::error("Forbidden: only owners/admins can manage billing", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let customer_id = match get_org_stripe_customer(&client, &org_id).await {
          Ok(Some(Some(customer_id))) => customer_id,
          // No Stripe customer yet: nothing for the portal to manage --
          // checkout is the flow that creates one.
          Ok(Some(None)) => {
            return Response::error(
              "Conflict: this organization has no billing account yet",
              409,
            )
            .unwrap()
            .with_cors(&cors);
          }
          Ok(None) => {
            return Response::error("Not Found: no such organization", 404)
              .unwrap()
              .with_cors(&cors);
          }
          Err(_) => {
            return Response::error("Internal Server Error", 500)
              .unwrap()
              .with_cors(&cors);
          }
        };

        let root_url = ctx
          .env
          .var("ROOT_URL")
          .map(|v| v.to_string())
          .unwrap_or_else(|_| "https://pidgeiot.com".to_string());
        let return_url = format!("{root_url}/orgs/{org_id}");

        let url = match create_portal_session(&ctx.env, &customer_id, &return_url).await {
          Ok(url) => url,
          Err(e) => {
            console_error!("Portal session create failed for org {org_id}: {e}");
            return Response::error("Bad Gateway: billing provider unavailable", 502)
              .unwrap()
              .with_cors(&cors);
          }
        };

        Response::from_json(&BillingSessionUrl { url })?.with_cors(&cors)
      },
    )
    // Moves a subscribed org between paid tiers in place. This exists
    // because Stripe's Billing Portal cannot switch a multi-product
    // subscription, and every checkout-minted subscription here is
    // multi-product (licensed tier + two metered overage prices) -- so
    // self-service tier changes have to be ours. One Subscriptions Update
    // call re-prices the licensed item and the tier-specific device-overage
    // item together, prorated immediately in both directions.
    .put_async(
      "/orgs/:org_id/billing/plan",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(auth) = require_auth_session(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(org_id) = ctx
          .param("org_id")
          .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
          return Response::error("Bad Request: invalid organization id", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(payload) = req.json::<BillingPlanChangeRequest>().await else {
          return Response::error("Bad Request: Invalid JSON payload", 400)
            .unwrap()
            .with_cors(&cors);
        };
        if payload.plan == BillingPlan::Perch {
          return Response::error(
            "Bad Request: moving to the free tier is a cancellation -- use the Stripe billing portal (Manage billing) to cancel",
            400,
          )
          .unwrap()
          .with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let Ok(caller_role) = org_role_of(&client, &org_id, &auth.user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        if !caller_role.is_some_and(|r| r.is_manager()) {
          return Response::error("Forbidden: only owners/admins can manage billing", 403)
            .unwrap()
            .with_cors(&cors);
        }

        // Both bootstraps: the state read wants the billing columns and
        // the allowance-floor write wants the usage table.
        if ensure_billing_tables(&client).await.is_err()
          || ensure_billing_usage_tables(&client).await.is_err()
        {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let state = match load_org_subscription_state(&client, &org_id).await {
          Ok(Some(state)) => state,
          Ok(None) => {
            return Response::error("Not Found: no such organization", 404)
              .unwrap()
              .with_cors(&cors);
          }
          Err(_) => {
            return Response::error("Internal Server Error", 500)
              .unwrap()
              .with_cors(&cors);
          }
        };

        let subscription_id = match state.stripe_subscription_id {
          Some(ref id) if state.status.is_entitled() => id.clone(),
          _ => {
            return Response::error(
              "Conflict: this organization has no live subscription to change -- checkout is the flow that starts one",
              409,
            )
            .unwrap()
            .with_cors(&cors);
          }
        };
        if state.plan == payload.plan {
          return Response::error(
            "Conflict: this organization is already on that plan",
            409,
          )
          .unwrap()
          .with_cors(&cors);
        }

        let prices = match resolve_checkout_prices(&ctx.env, payload.plan).await {
          Ok(prices) => prices,
          Err(e) => {
            console_error!("Plan change price resolution failed for org {org_id}: {e}");
            return Response::error("Bad Gateway: billing provider unavailable", 502)
              .unwrap()
              .with_cors(&cors);
          }
        };

        let subscription = match fetch_subscription(&ctx.env, &subscription_id).await {
          Ok(subscription) => subscription,
          Err(e) => {
            console_error!("Plan change subscription fetch failed for org {org_id}: {e}");
            return Response::error("Bad Gateway: billing provider unavailable", 502)
              .unwrap()
              .with_cors(&cors);
          }
        };

        // No licensed item means our own provisioning invariant is broken,
        // not that Stripe is misbehaving -- a 500, and loudly.
        let Some(licensed_item_id) = subscription.licensed_item_id().map(str::to_string) else {
          console_error!(
            "Plan change refused for org {org_id}: subscription {subscription_id} carries no licensed tier item"
          );
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };
        // A missing device-overage item (a subscription predating the
        // metered composition) is added by the same update rather than
        // refused -- the change is exactly the moment to converge on the
        // designed composition.
        let device_overage_item_id = subscription.device_overage_item_id().map(str::to_string);

        // Floor before Stripe: a failure here refuses the change, so a
        // downgrade can never bill the in-flight period at the new, lower
        // message allowance.
        if raise_message_allowance_floor(
          &client,
          &org_id,
          state.usage_period_start,
          state.usage_period_end,
          state.plan.included_messages(),
        )
        .await
        .is_err()
        {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        }

        let updated = match update_subscription_tier(
          &ctx.env,
          &subscription_id,
          &licensed_item_id,
          device_overage_item_id.as_deref(),
          &prices,
          payload.plan,
        )
        .await
        {
          Ok(updated) => updated,
          Err(e) => {
            console_error!("Plan change update failed for org {org_id}: {e}");
            return Response::error("Bad Gateway: billing provider unavailable", 502)
              .unwrap()
              .with_cors(&cors);
          }
        };

        // The org row itself is written by the webhook's
        // customer.subscription.updated (idempotent, out-of-order-safe);
        // the response reflects Stripe's own post-update state so the
        // dashboard doesn't have to wait for that delivery.
        let billing: capsules::OrganizationBilling = updated.into();
        Response::from_json(&billing)?.with_cors(&cors)
      },
    )
    .post_async("/billing/webhook", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);

      let Ok(secret) = ctx.env.secret(STRIPE_WEBHOOK_SECRET).map(|s| s.to_string()) else {
        console_error!("Stripe webhook: {STRIPE_WEBHOOK_SECRET} is not configured");
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(Some(signature)) = req.headers().get("Stripe-Signature") else {
        return Response::error("Bad Request: missing Stripe-Signature", 400)
          .unwrap()
          .with_cors(&cors);
      };

      // Raw bytes, never req.json() -- the signature covers exactly these
      // bytes, and any reparse-and-reserialize round trip invalidates it.
      let Ok(body) = req.bytes().await else {
        return Response::error("Bad Request: could not read body", 400)
          .unwrap()
          .with_cors(&cors);
      };

      let now = (Date::now().as_millis() / 1000) as i64;
      if let Err(e) = verify_webhook_signature(&secret, &signature, &body, now).await {
        console_error!("Stripe webhook: rejected -- {e}");
        return Response::error("Bad Request: signature verification failed", 400)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(event) = serde_json::from_slice::<StripeWebhookEvent>(&body) else {
        console_error!("Stripe webhook: signature valid but envelope did not parse");
        return Response::error("Bad Request: unrecognized event envelope", 400)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(event_created) = time::OffsetDateTime::from_unix_timestamp(event.created) else {
        console_error!("Stripe webhook: event {} has an unusable created time", event.id);
        return Response::error("Bad Request: unusable event timestamp", 400)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      if ensure_billing_tables(&client).await.is_err() {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      }

      let Ok(claim) = claim_webhook_event(
        &client,
        &event.id,
        &event.kind,
        event_created,
        event.livemode,
        event.api_version.as_deref(),
      )
      .await
      else {
        return Response::error("Internal Server Error", 500)
          .unwrap()
          .with_cors(&cors);
      };

      if claim == WebhookClaim::AlreadyProcessed {
        console_log!(
          "Stripe webhook: {} ({}) already applied, acking redelivery",
          event.id,
          event.kind
        );
        return Response::from_json(&serde_json::json!({"received": true, "duplicate": true}))?
          .with_cors(&cors);
      }

      // Applied inline rather than enqueued: this is one indexed UPDATE,
      // far inside Stripe's delivery timeout. Anything heavier belongs on
      // the existing queue, with the 200 returned before the work.
      if event.kind.starts_with("customer.subscription.") {
        let Ok(subscription) =
          serde_json::from_value::<capsules::StripeSubscriptionRow>(event.data.object.clone())
        else {
          console_error!(
            "Stripe webhook: {} carried an unreadable subscription object",
            event.id
          );
          return Response::error("Bad Request: unreadable subscription object", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let billing: capsules::OrganizationBilling = subscription.into();
        let Ok(applied) = apply_subscription(&client, &billing, event_created).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        if !applied {
          // Either no org claims this Stripe customer, or a newer event
          // already wrote this row. Both are ack-worthy -- retrying cannot
          // change either -- but the first means provisioning and Stripe
          // have diverged, so it must be visible.
          console_error!(
            "Stripe webhook: {} ({}) matched no organization row",
            event.id,
            event.kind
          );
        }
      } else if event.kind == "checkout.session.completed" {
        let Ok(session) =
          serde_json::from_value::<StripeCheckoutSessionRow>(event.data.object.clone())
        else {
          console_error!(
            "Stripe webhook: {} carried an unreadable checkout session object",
            event.id
          );
          return Response::error("Bad Request: unreadable checkout session object", 400)
            .unwrap()
            .with_cors(&cors);
        };

        // Bind the customer to the originating org first: the
        // subscription lifecycle events can arrive in any order relative
        // to this one, and they match org rows by customer id.
        if let (Some(org_id), Some(customer_id)) = (
          session
            .client_reference_id
            .as_deref()
            .and_then(|s| uuid::Uuid::parse_str(s).ok()),
          session.customer.as_deref(),
        ) && attach_stripe_customer(&client, &org_id, customer_id)
          .await
          .is_err()
        {
          console_error!(
            "Stripe webhook: {} could not attach customer to org {org_id}",
            event.id
          );
        }

        match session.subscription.as_deref() {
          Some(subscription_id) if stripe_configured(&ctx.env) => {
            match fetch_subscription(&ctx.env, subscription_id).await {
              Ok(subscription) => {
                let billing: capsules::OrganizationBilling = subscription.into();
                match apply_subscription(&client, &billing, event_created).await {
                  Ok(true) => {}
                  Ok(false) => console_error!(
                    "Stripe webhook: {} checkout completion matched no organization row",
                    event.id
                  ),
                  Err(_) => {
                    return Response::error("Internal Server Error", 500)
                      .unwrap()
                      .with_cors(&cors);
                  }
                }
              }
              // The subscription's own lifecycle events carry the same
              // state, so a fetch failure here is logged and acked rather
              // than earning a retry loop.
              Err(e) => console_error!(
                "Stripe webhook: {} could not fetch its subscription: {e}",
                event.id
              ),
            }
          }
          Some(_) => console_error!(
            "Stripe webhook: {} names a subscription but STRIPE_SECRET_KEY is not configured -- relying on subscription events alone",
            event.id
          ),
          None => {}
        }
      }

      if mark_webhook_event_processed(&client, &event.id).await.is_err() {
        // The state change already landed. Reporting failure would earn a
        // redelivery that the idempotency row can no longer suppress, so
        // ack and leave the row visible in the unprocessed index instead.
        console_error!(
          "Stripe webhook: {} applied but could not be marked processed",
          event.id
        );
      }

      Response::from_json(&serde_json::json!({"received": true}))?.with_cors(&cors)
    })
    .or_else_any_method_async("/*any", |mut req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      match req.text().await {
        Ok(b) => console_log!("{b}"),
        Err(e) => console_error!("{e}"),
      }
      Response::error("Not Found", 404).unwrap().with_cors(&cors)
    })
    .run(req, env);

  // Global Framework Escape Catchment Guard
  match router.await {
    Ok(response) => Ok(response),
    Err(err) => {
      console_error!("Gateway Isolation Panic Intercepted: {:?}", err);
      Response::error("Internal Server Error", 500)
        .unwrap()
        .with_cors(&fallback_cors)
    }
  }
}
