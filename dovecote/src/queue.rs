use worker::{
  Context, Env, Message, MessageBatch, MessageExt, Method, Request, RequestInit, Result,
  console_error, console_log, event,
};

use crate::helpers::write_telemetry_history;

/// Message enqueued by the `POST /device/pigeons/:id/telemetry` gateway
/// route (`lib.rs`) once it has verified the device's bearer token against
/// the owning DO -- see `verify_device_via_do` (`helpers/pigeons.rs`) and
/// `verify_telemetry_device`/`write_telemetry_device`
/// (`objects/pigeons.rs`). `reported_at_ms` is when the gateway accepted
/// the report; it's informational only for now -- the DO's own
/// `pigeon_telemetry` rows still stamp `reported_at` at write time via
/// SQLite's `unixepoch()` default, unchanged from the pre-queue
/// direct-write path.
///
/// `metrics_json` carries the device's flat `{"key":"val"}` report as a
/// pre-serialized JSON string, NOT a `HashMap`: `Queue::send` serializes
/// through serde-wasm-bindgen, which turns a Rust map into a JS `Map`, and
/// the queue's default JSON content type then `JSON.stringify`s that `Map`
/// into `{}` -- silently emptying every report (observed live: the DO write
/// 400'd "Empty telemetry report" on every consumed message). Strings
/// survive every serializer identically. `#[serde(default)]` lets messages
/// from before this fix decode as empty and be ack-dropped instead of
/// wedging the whole batch.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct TelemetryMessage {
  pub pigeon_id: String,
  #[serde(default)]
  pub metrics_json: String,
  pub reported_at_ms: u64,
}

/// Queue consumer for `pidgeiot-telemetry` (bound as `TELEMETRY_QUEUE`,
/// staging-only for now -- see `[env.staging.queues]` in `wrangler.toml`).
/// Dispatches each message to its owning pigeon's DO at the
/// trusted-internal `/pigeon/device/telemetry/write` route (no auth check
/// there -- see that handler's doc comment for why that's safe), keeping
/// the DO's SQLite `pigeon_telemetry` table as the store, unchanged from
/// the pre-queue direct-write path. Acks/retries per-message rather than
/// failing the whole batch on one bad message, so a single malformed
/// pigeon_id doesn't hold up every other device's report in the batch.
#[event(queue)]
pub async fn queue_consumer(
  message_batch: MessageBatch<TelemetryMessage>,
  env: Env,
  _ctx: Context,
) -> Result<()> {
  let Ok(namespace) = env.durable_object("PIGEONS") else {
    console_error!("Telemetry consumer: failed to bind PIGEONS namespace");
    message_batch.retry_all();
    return Ok(());
  };

  for message in message_batch.messages()? {
    dispatch_to_do(&namespace, &env, &message).await;
  }

  Ok(())
}

async fn dispatch_to_do(
  namespace: &worker::ObjectNamespace,
  env: &Env,
  message: &Message<TelemetryMessage>,
) {
  let body = message.body();

  let Ok(obj_id) = namespace.id_from_string(&body.pigeon_id) else {
    console_error!(
      "Telemetry consumer: malformed pigeon_id '{}'",
      body.pigeon_id
    );
    // Will never parse on retry either -- ack to drop it rather than
    // retrying forever.
    message.ack();
    return;
  };

  let Ok(stub) = obj_id.get_stub() else {
    console_error!(
      "Telemetry consumer: failed to get DO stub for '{}'",
      body.pigeon_id
    );
    message.retry();
    return;
  };

  if body.metrics_json.is_empty() {
    console_error!(
      "Telemetry consumer: empty/legacy message for '{}', dropping",
      body.pigeon_id
    );
    // Pre-metrics_json messages (or an empty report that slipped through)
    // will never become valid on retry -- ack to drop.
    message.ack();
    return;
  }

  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  init.body = Some(body.metrics_json.clone().into());

  let Ok(do_req) = Request::new_with_init("https://internal/pigeon/device/telemetry/write", &init)
  else {
    console_error!(
      "Telemetry consumer: failed to build DO request for '{}'",
      body.pigeon_id
    );
    message.retry();
    return;
  };

  match stub.fetch_with_request(do_req).await {
    Ok(resp) if resp.status_code() < 400 => {
      console_log!("Telemetry consumer: wrote metrics for '{}'", body.pigeon_id);
      message.ack();

      // Best-effort PG history write (task #18, part 1) -- re-parses the
      // same metrics_json already sent to the DO above rather than reading
      // it back from the DO's response, so this has no dependency on that
      // response's shape. Mirrors this codebase's established best-effort
      // PG sync convention: log and move on, never fail/retry the queue
      // message over a history-write failure once the DO write (the
      // source of truth) has already succeeded.
      match serde_json::from_str::<std::collections::HashMap<String, String>>(&body.metrics_json)
      {
        Ok(metrics) => {
          if let Err(e) = write_telemetry_history(env, &body.pigeon_id, &metrics).await {
            console_error!(
              "Telemetry consumer: history write failed for '{}': {e}",
              body.pigeon_id
            );
          }
        }
        Err(e) => console_error!(
          "Telemetry consumer: failed to re-parse metrics_json for history write ('{}'): {e}",
          body.pigeon_id
        ),
      }
    }
    Ok(resp) => {
      console_error!(
        "Telemetry consumer: DO write for '{}' returned {}",
        body.pigeon_id,
        resp.status_code()
      );
      message.retry();
    }
    Err(e) => {
      console_error!(
        "Telemetry consumer: DO fetch failed for '{}': {e}",
        body.pigeon_id
      );
      message.retry();
    }
  }
}
