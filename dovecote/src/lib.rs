use crate::helpers::{
  authenticate_browser, create_user_flock, delete_pigeon_pg_db, get_db_client, get_hyperdrive_conn,
  get_user_flocks, insert_pigeon_pg_db, proxy_to_pigeon_do, require_device_auth,
  update_pigeon_pg_db, update_shadow_pg_db, upsert_acl_pg_db,
};
use capsules::{FlockCreateRequest, Pigeon, PigeonAcl, PigeonDetail, PigeonShadow};
use futures::future::join_all;
use once_cell::sync::Lazy;
use worker::{
  Context, Env, Method, Request, RequestInit, Response, Router, console_error, console_log, event,
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

  // Extract the identity ID, failing if it doesn't exist
  let identity = session
    .identity
    .ok_or_else(|| worker::Error::RustError("Session missing identity".to_string()))?;

  Ok(identity.id)
}

#[event(fetch, respond_with_errors)]
async fn main(req: Request, env: Env, _ctx: Context) -> worker::Result<Response> {
  Router::new()
    .get_async("/device/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let Some(pigeon_id) = ctx.param("pigeon_id") else {
        return Response::error("Pigeon ID cannot be empty or invalid", 400);
      };

      let Ok(()) = require_device_auth(&req, &ctx.env, pigeon_id) else {
        return Response::error("Unauthorized", 401);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let Ok(stub) = namespace.id_from_string(pigeon_id) else {
        return Response::error("Bad Request", 500);
      };

      proxy_to_pigeon_do(req, "", &stub, "/shadow/get").await
    })
    .options_async("/*any", |_req, _ctx| async move {
      Response::empty()?.with_cors(&CORS)
    })
    .get_async("/flocks", |req, ctx: worker::RouteContext<()>| async move {
      // 1. Authoritative Identity Check
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      // 2. Establish the Hyperdrive DB Connection
      let Ok(client) = get_db_client(&ctx.env).await else {
        console_error!("Failed to establish Hyperdrive connection");
        return Response::error("DB Error", 500);
      };

      // 3. Query the Control Plane using our strict helper
      let user_flocks = match get_user_flocks(&client, &user_id).await {
        Ok(flocks) => flocks,
        Err(e) => return Err(e),
      };

      // 4. Return the strongly-typed JSON array
      Response::from_json(&user_flocks)?.with_cors(&CORS)
    })
    .post_async("/flocks", |mut req, ctx| async move {
      // 1. Authoritative Identity Check
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      // 2. Establish the Hyperdrive DB Connection
      let Ok(client) = get_db_client(&ctx.env).await else {
        return Response::error("DB Error", 500);
      };

      // 3. Parse the incoming JSON body
      let Ok(payload) = req.json::<FlockCreateRequest>().await else {
        return Response::error("Invalid JSON payload", 400);
      };

      // Ensure they didn't just send an empty string
      if payload.name.trim().is_empty() {
        return Response::error("Flock name cannot be empty", 400);
      }

      // 4. Create the flock in the Control Plane
      match create_user_flock(&client, &user_id, &payload.name).await {
        // 5. Return the newly created, fully populated Flock object to Dioxus!
        Ok(flock) => Response::from_json(&flock)?.with_cors(&CORS),
        Err(e) => Err(e),
      }
    })
    .post_async("/pigeons/batch", |mut req, ctx| async move {
      // 1. Authoritative Identity Check
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      // 2. Get Pigeon IDs
      let Ok(pigeon_ids) = req.json::<Vec<String>>().await else {
        return Response::error("Pigeon IDs cannot be empty or invalid", 400);
      };

      let mut fetch_tasks = Vec::new();

      let Ok(pigeon_namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      for id in pigeon_ids {
        let namespace_clone = pigeon_namespace.clone();
        let u_id = user_id.clone();

        fetch_tasks.push(async move {
          let Ok(stub) = namespace_clone
            .id_from_string(&id)
            .and_then(|do_id| do_id.get_stub())
          else {
            return Response::error("Bad Request", 500);
          };

          let do_req = RequestInit::default();

          let Ok(_headers) = do_req.headers.set("X-User-Id", &u_id) else {
            return Response::error("Failed to set 'X-User-Id'", 500);
          };

          let Ok(do_req) = Request::new_with_init("https://internal/pigeon/get", &do_req) else {
            return Response::error("Bad Request", 500);
          };

          stub.fetch_with_request(do_req).await
        });
      }

      // Execute all fetches concurrently
      let responses = join_all(fetch_tasks).await;

      let mut pigeons: Vec<Pigeon> = Vec::new();

      for mut resp in responses.into_iter().flatten() {
        if let Ok(pigeon) = resp.json::<Pigeon>().await {
          pigeons.push(pigeon);
        }
      }

      Response::from_json(&pigeons)?.with_cors(&CORS)
    })
    .post_async("/flock/pigeons", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let pigeon_id = namespace.unique_id().map_err(|e| {
        console_error!("Failed to create unique DO ID: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      let mut do_response: Response =
        proxy_to_pigeon_do(req, &user_id, &pigeon_id, "/create").await?;

      if do_response.status_code() >= 400 {
        return Ok(do_response);
      }

      let pcr = do_response.json::<PigeonDetail>().await.map_err(|e| {
        console_error!("Failed to parse DO response: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      let Ok(client) = get_db_client(&ctx.env).await else {
        return Response::error("DB Error", 500);
      };

      if let Err(e) = insert_pigeon_pg_db(client, &pcr).await {
        console_error!(
          "Failed to sync pigeon {} to external DB: {e}",
          pcr.pigeon.id
        );
        // Don't fail the request — the pigeon exists in the DO, sync can be retried
      }

      Response::from_json(&pcr)?.with_cors(&CORS)
    })
    .get_async(
      "/pigeons/:pigeon_id",
      |req, ctx: worker::RouteContext<()>| async move {
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401);
        };

        let Ok(namespace) = ctx.durable_object("PIGEONS") else {
          return Response::error("Failed to bind to PIGEONS namespace", 500);
        };

        let Some(pigeon_id) = ctx.param("pigeon_id") else {
          return Response::error("Pigeon ID cannot be empty or invalid", 400);
        };

        let Ok(stub) = namespace.id_from_string(pigeon_id) else {
          return Response::error("Bad Request", 500);
        };

        proxy_to_pigeon_do(req, &user_id, &stub, "/get").await
      },
    )
    .get_async(
      "/pigeons/:pigeon_id/detail",
      |req, ctx: worker::RouteContext<()>| async move {
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401);
        };

        let Ok(namespace) = ctx.durable_object("PIGEONS") else {
          return Response::error("Failed to bind to PIGEONS namespace", 500);
        };

        let Some(pigeon_id) = ctx.param("pigeon_id") else {
          return Response::error("Pigeon ID cannot be empty or invalid", 400);
        };

        let Ok(stub) = namespace.id_from_string(pigeon_id) else {
          return Response::error("Bad Request", 500);
        };

        proxy_to_pigeon_do(req, &user_id, &stub, "/detail").await
      },
    )
    .put_async("/pigeons/:pigeon_id", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      let Some(pigeon_id) = ctx.param("pigeon_id") else {
        return Response::error("Pigeon ID cannot be empty or invalid", 400);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let Ok(stub) = namespace.id_from_string(pigeon_id) else {
        return Response::error("Bad Request", 500);
      };

      let mut do_response = proxy_to_pigeon_do(req, &user_id, &stub, "/update").await?;

      if do_response.status_code() >= 400 {
        return Ok(do_response);
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
    .delete_async(
      "/pigeons/:pigeon_id",
      |req, ctx: worker::RouteContext<()>| async move {
        let Ok(user_id) = require_auth(&req, &ctx.env).await else {
          return Response::error("Unauthorized", 401);
        };

        let Ok(namespace) = ctx.durable_object("PIGEONS") else {
          return Response::error("Failed to bind to PIGEONS namespace", 500);
        };

        let Some(pigeon_id) = ctx.param("pigeon_id") else {
          return Response::error("Pigeon ID cannot be empty or invalid", 400);
        };

        let Ok(stub) = namespace.id_from_string(pigeon_id) else {
          return Response::error("Bad Request", 500);
        };

        let do_response = proxy_to_pigeon_do(req, &user_id, &stub, "/delete").await?;

        if do_response.status_code() >= 400 {
          return Ok(do_response);
        }

        if let Ok(client) = get_db_client(&ctx.env).await
          && let Err(e) = delete_pigeon_pg_db(client, pigeon_id).await
        {
          console_error!("Failed to sync pigeon {} to external DB: {e}", pigeon_id);
        }

        Response::empty()?.with_cors(&CORS)
      },
    )
    // Shadow routes
    .get_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      let Some(pigeon_id) = ctx.param("pigeon_id") else {
        return Response::error("Pigeon ID cannot be empty or invalid", 400);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let Ok(stub) = namespace.id_from_string(pigeon_id) else {
        return Response::error("Bad Request", 500);
      };

      proxy_to_pigeon_do(req, &user_id, &stub, "/shadow/get").await
    })
    .post_async("/pigeons/:pigeon_id/shadow", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      let Some(pigeon_id) = ctx.param("pigeon_id") else {
        return Response::error("Pigeon ID cannot be empty or invalid", 400);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let Ok(stub) = namespace.id_from_string(pigeon_id) else {
        return Response::error("Bad Request", 500);
      };

      let mut do_response = proxy_to_pigeon_do(req, &user_id, &stub, "/shadow/update").await?;

      if do_response.status_code() >= 400 {
        return Ok(do_response);
      }

      let shadow = do_response.json::<PigeonShadow>().await.map_err(|e| {
        console_error!("Failed to parse DO shadow response: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = update_shadow_pg_db(client, pigeon_id, &shadow).await
      {
        console_error!("Failed to sync shadow for pigeon {pigeon_id} to external DB: {e}");
      }

      Response::from_json(&shadow)?.with_cors(&CORS)
    })
    // ACL routes
    .get_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      let Some(pigeon_id) = ctx.param("pigeon_id") else {
        return Response::error("Pigeon ID cannot be empty or invalid", 400);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let Ok(stub) = namespace.id_from_string(pigeon_id) else {
        return Response::error("Bad Request", 500);
      };

      proxy_to_pigeon_do(req, &user_id, &stub, "/acl/list").await
    })
    .post_async("/pigeons/:pigeon_id/acl", |req, ctx| async move {
      let Ok(user_id) = require_auth(&req, &ctx.env).await else {
        return Response::error("Unauthorized", 401);
      };

      let Some(pigeon_id) = ctx.param("pigeon_id") else {
        return Response::error("Pigeon ID cannot be empty or invalid", 400);
      };

      let Ok(namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      let Ok(stub) = namespace.id_from_string(pigeon_id) else {
        return Response::error("Bad Request", 500);
      };

      let mut do_response = proxy_to_pigeon_do(req, &user_id, &stub, "/acl/update").await?;

      if do_response.status_code() >= 400 {
        return Ok(do_response);
      }

      let acl = do_response.json::<PigeonAcl>().await.map_err(|e| {
        console_error!("Failed to parse DO ACL response: {e}");
        worker::Error::RustError("Internal Server Error".into())
      })?;

      if let Ok(client) = get_db_client(&ctx.env).await
        && let Err(e) = upsert_acl_pg_db(client, pigeon_id, &acl).await
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
      Response::error("Not Found", 404)
    })
    .run(req, env)
    .await
}
