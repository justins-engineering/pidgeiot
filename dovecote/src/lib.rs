use crate::helpers::{
  authenticate_browser, create_user_flock, delete_pigeon_pg_db, get_db_client, get_hyperdrive_conn,
  get_user_flocks, insert_pigeon_pg_db, proxy_to_pigeon_do, update_pigeon_pg_db,
  update_shadow_pg_db, upsert_acl_pg_db, verify_cf_access, verify_device_via_do,
};
use crate::queue::TelemetryMessage;
use capsules::{FlockCreateRequest, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow};
use futures::future::join_all;
use worker::{
  Context, Date, Env, Method, Request, RequestInit, Response, RouteContext, Router, console_error,
  console_log, event,
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

        Response::ok("{}").unwrap().with_status(202).with_cors(&cors)
      },
    )
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
