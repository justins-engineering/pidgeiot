use crate::helpers::{
  authenticate_browser, create_user_flock, delete_pigeon_pg_db, get_db_client, get_hyperdrive_conn,
  get_user_flocks, insert_pigeon_pg_db, is_flock_owner, list_flock_firmware,
  proxy_binary_to_pigeon_do, proxy_to_pigeon_do, query_telemetry_history_for_flock,
  query_telemetry_history_for_pigeon, sha256_hex, update_pigeon_pg_db, update_shadow_pg_db,
  update_telemetry_endpoint_pg_db, upsert_acl_pg_db, upsert_flock_firmware, verify_cf_access,
  verify_device_via_do,
};
use crate::queue::TelemetryMessage;
use capsules::{
  FirmwareTarget, FirmwareUploadQuery, FlockCreateRequest, Pigeon, PigeonAcl, PigeonDetail,
  PigeonShadow, TelemetryEndpoint, TelemetryHistoryQuery,
};
use futures::future::join_all;
use worker::{
  Context, Date, Env, Headers, Method, Range, Request, RequestInit, Response, ResponseBuilder,
  RouteContext, Router, console_error, console_log, event,
};

mod helpers;
mod objects;
mod queue;

/// `worker::Cors::apply_headers` joins every configured origin into the
/// `Access-Control-Allow-Origin` header with commas (see
/// `worker-0.8.5/src/cors.rs`) — it does not match against the request's
/// `Origin` header at all. A comma-joined value is invalid per the CORS
/// spec (the header may only ever be a single origin or `*`), so browsers
/// silently reject it the moment more than one origin is configured. We
/// therefore do the matching ourselves here and always hand
/// `Cors::with_origins` exactly one value: `ROOT_URL` if the request's
/// `Origin` matches it, otherwise `ROOT_URL` anyway as an inert default —
/// it simply won't match whatever the disallowed request's Origin was.
///
/// `ROOT_URL` (`[vars]`/`[env.dev.vars]`, wrangler.toml) is the frontend's
/// own origin in both environments — `https://pidgeiot.com` in production,
/// the local `dx serve` address in dev — so there's nothing else to
/// configure here.
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
    .with_exposed_headers(vec!["Location"])
    .with_credentials(true)
}

/// Validates the Kratos cookie and returns the User ID as a String.
pub async fn require_auth(req: &Request, env: &Env) -> worker::Result<String> {
  let session = crate::authenticate_browser(req, env)
    .await
    .map_err(|_| worker::Error::RustError("Unauthorized".to_string()))?;

  session
    .identity
    .map(|identity| identity.id)
    .ok_or_else(|| worker::Error::RustError("Session missing identity".to_string()))
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
/// `bytes=<start>-` for open-ended, or `bytes=-<suffix>` for a trailing
/// slice) into an R2 [`Range`] for `GET /device/pigeons/:id/firmware`
/// (task #23) — the nRF9160 downloads a firmware image in small chunks
/// straight to flash rather than buffering the whole ~300KB-1MB image in
/// its ~256KB of RAM, so ranged reads aren't an optimization here, they're
/// required. Only a single range is supported (a comma-separated multi-range
/// request just uses the first one) — the device downloads sequentially,
/// never in parallel/multi-range. Returns `None` on anything malformed,
/// letting the caller fall back to serving the whole object.
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

/// Computes the inclusive `(start, end)` byte range actually being served
/// for `Content-Range`/`Content-Length`, given the request's parsed `Range`
/// (if any) and the firmware's total size (from the shadow-assigned
/// `FirmwareTarget`, treated as the authoritative total rather than
/// whatever R2's own `Object::size()`/`Object::range()` report for a
/// ranged fetch, which is ambiguous in the `worker` crate's own docs).
/// Clamps an out-of-bounds request down to the object's actual end rather
/// than erroring — a device racing a shrinking/reassigned image is a rare
/// edge case, not worth a hard 416 here.
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
    .post_async("/pigeons/:pigeon_id/token/refresh", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let mut do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/token/refresh").await?;

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
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // No X-User-Id / Kratos session here — the DO verifies the device's
      // own Authorization header (forwarded by proxy_to_pigeon_do) against
      // this pigeon's stored public key.
      proxy_to_pigeon_do(req, "", &obj_id, "/device/shadow")
        .await?
        .with_cors(&cors)
    })
    .post_async("/device/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // Same device-auth model as the GET route above — no X-User-Id here.
      let do_response = proxy_to_pigeon_do(req, "", &obj_id, "/device/shadow/report").await?;
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
    .post_async(
      "/device/pigeons/:pigeon_id/telemetry",
      |mut req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        // Telemetry queue (task #14) — staging-only for now (see
        // [env.staging.queues] in wrangler.toml). Environments with no
        // TELEMETRY_QUEUE binding (dev, prod today) fall through to the
        // original synchronous direct-DO-write path unchanged.
        let Ok(telemetry_queue) = ctx.env.queue("TELEMETRY_QUEUE") else {
          // Same device-auth model as the shadow device routes above.
          return proxy_to_pigeon_do(req, "", &obj_id, "/device/telemetry")
            .await?
            .with_cors(&cors);
        };

        // The queue has no authentication of its own, so the device's
        // bearer token must be verified against the DO *before* anything
        // is enqueued — an unauthenticated/forged report must never reach
        // the queue. This costs one extra DO round trip (verify) on top of
        // the eventual consumer write, versus one combined round trip in
        // the non-queue path above.
        let auth_header = req.headers().get("Authorization").ok().flatten();

        let Ok(metrics) = req
          .json::<std::collections::HashMap<String, String>>()
          .await
        else {
          return Response::error("Bad Request: Invalid JSON", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if metrics.is_empty() {
          return Response::error("Bad Request: Empty telemetry report", 400)
            .unwrap()
            .with_cors(&cors);
        }

        let verify_resp =
          verify_device_via_do(auth_header, &obj_id, "/device/telemetry/verify").await?;
        if verify_resp.status_code() >= 400 {
          return verify_resp.with_cors(&cors);
        }

        // Pre-serialize the metrics map here: a HashMap round-tripped
        // through Queue::send arrives at the consumer as an empty object
        // (serde-wasm-bindgen map -> JS Map -> JSON.stringify == "{}"),
        // so the queue message carries a JSON string instead -- see
        // TelemetryMessage in queue.rs.
        let Ok(metrics_json) = serde_json::to_string(&metrics) else {
          console_error!("Failed to serialize telemetry for pigeon {pigeon_id}");
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        let message = TelemetryMessage {
          pigeon_id: pigeon_id.clone(),
          metrics_json,
          reported_at_ms: Date::now().as_millis(),
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
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      // Same device-auth model as the other /device/pigeons/:id/* routes —
      // no X-User-Id here, the DO verifies the bearer token itself. Body is
      // a raw binary dictionary-log chunk, not JSON — proxy_binary_to_pigeon_do
      // forwards it byte-for-byte instead of through proxy_to_pigeon_do's
      // text()-based forwarding, which would corrupt non-UTF-8 bytes.
      proxy_binary_to_pigeon_do(req, &obj_id, "/device/logs")
        .await?
        .with_cors(&cors)
    })
    .get_async(
      "/device/pigeons/:pigeon_id/firmware",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

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
        let do_response = proxy_to_pigeon_do(req, "", &obj_id, "/device/firmware/target").await?;
        if do_response.status_code() >= 400 {
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
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &user_id, &obj_id, "/logs/get")
        .await?
        .with_cors(&cors)
    })
    .get_async("/flocks", |req, ctx: RouteContext<()>| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      get_db!(ctx.env, client, &cors);

      let user_flocks = get_user_flocks(&client, &user_id).await?;
      Response::from_json(&user_flocks)?.with_cors(&cors)
    })
    .post_async("/flocks", |mut req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
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

      let flock = create_user_flock(&client, &user_id, &payload.name).await?;
      Response::from_json(&flock)?.with_cors(&cors)
    })
    .post_async("/pigeons/batch", |mut req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
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

      let fetch_tasks = pigeon_ids.into_iter().map(|id| {
        let namespace_clone = pigeon_namespace.clone();
        let u_id = user_id.clone();

        async move {
          let stub = namespace_clone.id_from_string(&id).ok()?.get_stub().ok()?;

          let headers = worker::Headers::new();
          headers.append("X-User-Id", &u_id).ok()?;

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
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500)
          .unwrap()
          .with_cors(&cors);
      };

      let obj_id = namespace.unique_id().map_err(|e| {
        console_error!("Failed to create unique DO ID: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/create").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let pcr = parse_do_response::<PigeonDetail>(do_response).await?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = insert_pigeon_pg_db(client, &pcr).await {
            console_error!("External DB Sync Error for pigeon {}: {e}", pcr.pigeon.id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&pcr)?.with_cors(&cors)
    })
    .get_async(
      "/pigeons/:pigeon_id",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
        proxy_to_pigeon_do(req, &user_id, &obj_id, "/get")
          .await?
          .with_cors(&cors)
      },
    )
    .get_async(
      "/pigeons/:pigeon_id/detail",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
        proxy_to_pigeon_do(req, &user_id, &obj_id, "/detail")
          .await?
          .with_cors(&cors)
      },
    )
    .put_async("/pigeons/:pigeon_id", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/update").await?;
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
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/delete").await?;
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

        Response::empty()?.with_cors(&cors)
      },
    )
    // --- Shadow Routes ---
    .get_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &user_id, &obj_id, "/shadow/get")
        .await?
        .with_cors(&cors)
    })
    .put_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/shadow/update").await?;
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
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &user_id, &obj_id, "/acl/list")
        .await?
        .with_cors(&cors)
    })
    .post_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/acl/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&cors);
      }

      let acl = parse_do_response::<PigeonAcl>(do_response).await?;

      match get_db_client(&ctx.env).await {
        Ok(client) => {
          if let Err(e) = upsert_acl_pg_db(client, &pigeon_id, &acl).await {
            console_error!("External DB Sync Error for ACL {}: {e}", pigeon_id);
          }
        }
        Err(err) => console_error!("Sync skipped: Hyperdrive connection failed: {err}"),
      }

      Response::from_json(&acl)?.with_cors(&cors)
    })
    // --- Telemetry Routes (task #18) ---
    .get_async("/pigeons/:pigeon_id/telemetry", |req, ctx| async move {
      let cors = build_cors(&ctx.env, &req);
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)
          .unwrap()
          .with_cors(&cors);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);
      proxy_to_pigeon_do(req, &user_id, &obj_id, "/telemetry/get")
        .await?
        .with_cors(&cors)
    })
    .put_async(
      "/pigeons/:pigeon_id/telemetry-endpoint",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id, &cors);

        let do_response =
          proxy_to_pigeon_do(req, &user_id, &obj_id, "/telemetry-endpoint/update").await?;
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
    .get_async(
      "/pigeons/:pigeon_id/telemetry/history",
      |req, ctx| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
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
        // itself is in Postgres -- check the DO first via the ACL probe
        // route before ever touching Postgres.
        let authz_resp = proxy_to_pigeon_do(req, &user_id, &obj_id, "/authz/check").await?;
        if authz_resp.status_code() >= 400 {
          return authz_resp.with_cors(&cors);
        }

        get_db!(ctx.env, client, &cors);

        let points = query_telemetry_history_for_pigeon(
          &client,
          &pigeon_id,
          query.key.as_deref(),
          query.since,
          query.until,
        )
        .await?;

        Response::from_json(&points)?.with_cors(&cors)
      },
    )
    .get_async(
      "/flocks/:flock_id/telemetry/history",
      |req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
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

        // Flocks have no per-entity ACL table (unlike pigeons) -- ownership
        // is folded directly into the query's WHERE clause (see
        // query_telemetry_history_for_flock's doc comment).
        get_db!(ctx.env, client, &cors);

        let points = query_telemetry_history_for_flock(
          &client,
          &flock_id,
          &user_id,
          query.key.as_deref(),
          query.since,
          query.until,
        )
        .await?;

        Response::from_json(&points)?.with_cors(&cors)
      },
    )
    // --- Firmware Routes (task #23) ---
    .post_async(
      "/flocks/:flock_id/firmware",
      |mut req, ctx: RouteContext<()>| async move {
        let cors = build_cors(&ctx.env, &req);
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)
            .unwrap()
            .with_cors(&cors);
        };

        let Some(flock_id) = ctx.param("flock_id").cloned() else {
          return Response::error("Flock ID cannot be empty or invalid", 400)
            .unwrap()
            .with_cors(&cors);
        };

        let Ok(query) = req.query::<FirmwareUploadQuery>() else {
          return Response::error("Bad Request: Missing 'version' query parameter", 400)
            .unwrap()
            .with_cors(&cors);
        };

        if query.version.trim().is_empty() {
          return Response::error("Bad Request: 'version' cannot be empty", 400)
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

        let Ok(owner) = is_flock_owner(&client, &flock_id, &user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        if !owner {
          return Response::error("Forbidden: Only the flock owner can upload firmware", 403)
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
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
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

        let Ok(owner) = is_flock_owner(&client, &flock_id, &user_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        if !owner {
          return Response::error("Forbidden: Only the flock owner can view firmware", 403)
            .unwrap()
            .with_cors(&cors);
        }

        let Ok(images) = list_flock_firmware(&client, &flock_id).await else {
          return Response::error("Internal Server Error", 500)
            .unwrap()
            .with_cors(&cors);
        };

        Response::from_json(&images)?.with_cors(&cors)
      },
    )
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
