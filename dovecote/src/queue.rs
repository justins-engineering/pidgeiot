use std::collections::HashMap;
use worker::{
  Context, Env, Message, MessageBatch, MessageExt, Method, Request, RequestInit, Result,
  console_error, event,
};

/// Message enqueued by the `POST /device/pigeons/:id/telemetry` gateway
/// route (`lib.rs`) once it has verified the device's bearer token against
/// the owning DO -- see `verify_device_via_do` (`helpers/pigeons.rs`) and
/// `verify_telemetry_device`/`write_telemetry_device`
/// (`objects/pigeons.rs`). `reported_at_ms` is when the gateway accepted
/// the report; it's informational only for now -- the DO's own
/// `pigeon_telemetry` rows still stamp `reported_at` at write time via
/// SQLite's `unixepoch()` default, unchanged from the pre-queue
/// direct-write path.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct TelemetryMessage {
  pub pigeon_id: String,
  pub metrics: HashMap<String, String>,
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
    dispatch_to_do(&namespace, &message).await;
  }

  Ok(())
}

async fn dispatch_to_do(namespace: &worker::ObjectNamespace, message: &Message<TelemetryMessage>) {
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

  let Ok(payload) = serde_json::to_string(&body.metrics) else {
    console_error!(
      "Telemetry consumer: failed to serialize metrics for '{}'",
      body.pigeon_id
    );
    message.ack();
    return;
  };

  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  init.body = Some(payload.into());

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
    Ok(resp) if resp.status_code() < 400 => message.ack(),
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
