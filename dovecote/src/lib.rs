use crate::helpers::{
  authenticate_browser, create_user_flock, delete_pigeon_pg_db, get_db_client, get_hyperdrive_conn,
  get_user_flocks, insert_pigeon_pg_db, proxy_to_pigeon_do, require_device_auth,
  update_pigeon_pg_db, update_shadow_pg_db, upsert_acl_pg_db,
};
use capsules::{FlockCreateRequest, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow};
use futures::future::join_all;
use once_cell::sync::Lazy;
use worker::{
  Context, Env, Method, Request, RequestInit, Response, RouteContext, Router, console_error,
  console_log, event,
};

mod helpers;
mod objects;

static CORS: Lazy<worker::Cors> = Lazy::new(|| {
  worker::Cors::new()
    .with_origins(vec!["https://pidgeiot.com"])
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
});

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
  ($ctx:expr, $pigeon_id:ident, $namespace:ident, $obj_id:ident) => {
    let Some($pigeon_id) = $ctx.param("pigeon_id").cloned() else {
      return Response::error("Pigeon ID cannot be empty or invalid", 400)?.with_cors(&CORS);
    };

    let Ok($namespace) = $ctx.durable_object("PIGEONS") else {
      return Response::error("Failed to bind to PIGEONS namespace", 500)?.with_cors(&CORS);
    };

    let Ok($obj_id) = $namespace.id_from_string(&$pigeon_id) else {
      return Response::error("Bad Request", 500)?.with_cors(&CORS);
    };
  };
}

/// Helper to establish a DB client, mapping failures to HTTP 500 responses.
macro_rules! get_db {
  ($env:expr, $client:ident) => {
    let Ok($client) = get_db_client(&$env).await else {
      console_error!("Failed to establish Hyperdrive connection");
      return Response::error("DB Error", 500)?.with_cors(&CORS);
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
  Router::new()
    .options_async("/*any", |_req, _ctx| async move {
      Response::empty()?.with_cors(&CORS)
    })
    .post_async("/pigeons/:pigeon_id/token/refresh", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };

      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);

      let mut do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/token/refresh").await?;

      if do_response.status_code() >= 400 {
        return do_response.with_cors(&CORS);
      }

      let pigeon = do_response.json::<Pigeon>().await.map_err(|e| {
        console_error!("Failed to parse DO response: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = update_pigeon_pg_db(client, &pigeon).await
      {
        console_error!("Failed to sync pigeon {} to external DB: {e}", pigeon.id);
      }

      Response::from_json(&pigeon)?.with_cors(&CORS)
    })
    .get_async("/device/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);

      if require_device_auth(&req, &ctx.env, &pigeon_id).is_err() {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      }

      proxy_to_pigeon_do(req, "", &obj_id, "/shadow/get")
        .await?
        .with_cors(&CORS)
    })
    .get_async("/flocks", |req, ctx: RouteContext<()>| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };

      get_db!(ctx.env, client);

      let user_flocks = get_user_flocks(&client, &user_id).await?;
      Response::from_json(&user_flocks)?.with_cors(&CORS)
    })
    .post_async("/flocks", |mut req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };

      let Ok(payload) = req.json::<FlockCreateRequest>().await else {
        return Response::error("Invalid JSON payload", 400)?.with_cors(&CORS);
      };

      if payload.name.trim().is_empty() {
        return Response::error("Flock name cannot be empty", 400)?.with_cors(&CORS);
      }

      get_db!(ctx.env, client);

      let flock = create_user_flock(&client, &user_id, &payload.name).await?;
      Response::from_json(&flock)?.with_cors(&CORS)
    })
    .post_async("/pigeons/batch", |mut req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };

      let Ok(pigeon_ids) = req.json::<Vec<String>>().await else {
        return Response::error("Pigeon IDs cannot be empty or invalid", 400)?.with_cors(&CORS);
      };

      if pigeon_ids.len() > 48 {
        return Response::error("Batch size exceeds subrequest limits", 400)?.with_cors(&CORS);
      }

      let Ok(pigeon_namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500)?.with_cors(&CORS);
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

      Response::from_json(&pigeons)?.with_cors(&CORS)
    })
    .post_async("/flock/pigeons", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500)?.with_cors(&CORS);
      };

      let obj_id = namespace.unique_id().map_err(|e| {
        console_error!("Failed to create unique DO ID: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/create").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&CORS);
      }

      let pcr = parse_do_response::<PigeonDetail>(do_response).await?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = insert_pigeon_pg_db(client, &pcr).await
      {
        console_error!(
          "Failed to sync pigeon {} to external DB: {e}",
          pcr.pigeon.id
        );
      }

      Response::from_json(&pcr)?.with_cors(&CORS)
    })
    .get_async(
      "/pigeons/:pigeon_id",
      |req, ctx: RouteContext<()>| async move {
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)?.with_cors(&CORS);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);
        proxy_to_pigeon_do(req, &user_id, &obj_id, "/get")
          .await?
          .with_cors(&CORS)
      },
    )
    .get_async(
      "/pigeons/:pigeon_id/detail",
      |req, ctx: RouteContext<()>| async move {
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)?.with_cors(&CORS);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);
        proxy_to_pigeon_do(req, &user_id, &obj_id, "/detail")
          .await?
          .with_cors(&CORS)
      },
    )
    .put_async("/pigeons/:pigeon_id", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&CORS);
      }

      let pigeon = parse_do_response::<Pigeon>(do_response).await?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = update_pigeon_pg_db(client, &pigeon).await
      {
        console_error!("Failed to sync pigeon {} to external DB: {e}", pigeon.id);
      }

      Response::from_json(&pigeon)?.with_cors(&CORS)
    })
    .delete_async(
      "/pigeons/:pigeon_id",
      |req, ctx: RouteContext<()>| async move {
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401)?.with_cors(&CORS);
        };
        get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);

        let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/delete").await?;
        if do_response.status_code() >= 400 {
          return do_response.with_cors(&CORS);
        }

        if let Ok(client) = get_db_client(&ctx.env).await
          && let Err(e) = delete_pigeon_pg_db(client, &pigeon_id).await
        {
          console_error!("Failed to sync pigeon {} to external DB: {e}", pigeon_id);
        }

        Response::empty()?.with_cors(&CORS)
      },
    )
    // --- Shadow Routes ---
    .get_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);
      proxy_to_pigeon_do(req, &user_id, &obj_id, "/shadow/get")
        .await?
        .with_cors(&CORS)
    })
    .put_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/shadow/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&CORS);
      }

      let shadow = parse_do_response::<PigeonShadow>(do_response).await?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = update_shadow_pg_db(client, &pigeon_id, &shadow).await
      {
        console_error!("Failed to sync shadow for pigeon {pigeon_id} to external DB: {e}");
      }

      Response::from_json(&shadow)?.with_cors(&CORS)
    })
    // --- ACL Routes ---
    .get_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);
      proxy_to_pigeon_do(req, &user_id, &obj_id, "/acl/list")
        .await?
        .with_cors(&CORS)
    })
    .post_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401)?.with_cors(&CORS);
      };
      get_pigeon_do!(ctx, pigeon_id, namespace, obj_id);

      let do_response = proxy_to_pigeon_do(req, &user_id, &obj_id, "/acl/update").await?;
      if do_response.status_code() >= 400 {
        return do_response.with_cors(&CORS);
      }

      let acl = parse_do_response::<PigeonAcl>(do_response).await?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = upsert_acl_pg_db(client, &pigeon_id, &acl).await
      {
        console_error!("Failed to sync ACL for pigeon {pigeon_id} to external DB: {e}");
      }

      Response::from_json(&acl)?.with_cors(&CORS)
    })
    .or_else_any_method_async("/*any", |mut req, _ctx| async move {
      match req.text().await {
        Ok(b) => console_log!("{b}"),
        Err(e) => console_error!("{e}"),
      }
      Response::error("Not Found", 404)?.with_cors(&CORS)
    })
    .run(req, env)
    .await
}
