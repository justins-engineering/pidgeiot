use crate::helpers::{
  authenticate_browser, create_user_flock, get_hyperdrive_conn, get_user_flocks,
};
use capsules::CreateFlockPayload;
use futures::future::join_all;
use once_cell::sync::Lazy;
use uuid::Uuid;
use worker::{Context, Env, Method, Request, Response, Router, console_error, console_log, event};

mod helpers;
mod objects;

// macro_rules! unwrap_or_return_response {
//   ($expr:expr) => {
//     match $expr {
//       Ok(val) => val,
//       Err(err_resp) => return err_resp,
//     }
//   };
// }

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

/// Establishes a Hyperdrive connection, spawns the background driver,
/// and hands back a ready-to-use Client.
pub async fn get_db_client(env: &Env) -> worker::Result<tokio_postgres::Client> {
  let (client, connection) = crate::get_hyperdrive_conn(env).await?;

  // Abstract the Wasm background task away from the route handlers!
  worker::wasm_bindgen_futures::spawn_local(async move {
    if let Err(e) = connection.await {
      console_error!("Postgres connection error: {}", e);
    }
  });

  Ok(client)
}

// fn get_flock_id(ctx: &worker::RouteContext<()>) -> Result<i64, worker::Result<Response>> {
//   let Some(id_str) = ctx.param("flock_id") else {
//     return Err(Response::error("Missing flock_id", 400));
//   };

//   let Ok(flock_id) = id_str.parse() else {
//     return Err(Response::error("Bad flock_id", 400));
//   };

//   Ok(flock_id)
// }

#[event(fetch, respond_with_errors)]
async fn main(req: Request, env: Env, _ctx: Context) -> worker::Result<Response> {
  Router::new()
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
        return Response::error("DB Error", 500);
      };

      // 3. Query the Control Plane using our strict helper
      let mut user_flocks = match get_user_flocks(&client, &user_id).await {
        Ok(flocks) => flocks,
        Err(e) => return Err(e),
      };

      // 4. Connect to the PIGEONS Durable Object Namespace
      let Ok(pigeon_namespace) = ctx.durable_object("PIGEONS") else {
        return Response::error("Failed to bind to PIGEONS namespace", 500);
      };

      // 5. Scatter-Gather the Live Edge State
      for flock in &mut user_flocks {
        let flock_uuid: Uuid = Uuid::parse_str(&flock.id).unwrap();

        // Query Yugabyte for the pigeon IDs in this specific flock
        let pigeon_rows = client
          .query_typed(
            "SELECT id FROM pigeons WHERE flock_id = $1",
            &[(&flock_uuid, tokio_postgres::types::Type::UUID)],
          )
          .await
          .map_err(|e| worker::Error::RustError(e.to_string()))?;

        // We assign the total row count to our strict model's pigeon_count
        flock.pigeon_count = pigeon_rows.len() as i64;

        let mut fetch_tasks: Vec<
          std::pin::Pin<Box<dyn std::future::Future<Output = Option<worker::Response>>>>,
        > = Vec::new();

        for row in pigeon_rows {
          let namespace_clone = pigeon_namespace.clone();
          let pigeon_id: Uuid = row.get("id");
          let pigeon_id_str = pigeon_id.to_string();

          fetch_tasks.push(Box::pin(async move {
            let Ok(stub) = namespace_clone
              .id_from_string(&pigeon_id_str)
              .and_then(|id| id.get_stub())
            else {
              return None;
            };

            // Ask the DO for its live memory state
            stub
              .fetch_with_str(&format!("http://internal/pigeons/{}/live", pigeon_id_str))
              .await
              .ok()
          }));
        }

        // Execute all fetches concurrently
        let responses = join_all(fetch_tasks).await;

        let mut active_pigeons = 0;

        for mut resp in responses.into_iter().flatten() {
          if let Ok(state) = resp.json::<serde_json::Value>().await {
            // Example: Count how many are actively connected to WebSockets
            if state["status"] == "active" {
              active_pigeons += 1;
            }
          }
        }

        // In this implementation, I am overwriting pigeon_count with active_pigeons.
        // If your frontend Dioxus model needs BOTH total_count and active_count,
        // you must update `src/models/flock.rs` to include a new `active_pigeons: i64` field!
        flock.pigeon_count = active_pigeons;
      }

      // 6. Return the strongly-typed JSON array
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
      // We expect the frontend to just send `{"name": "My New Flock"}`
      let Ok(payload) = req.json::<CreateFlockPayload>().await else {
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
