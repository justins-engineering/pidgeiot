use capsules::TelemetryEndpoint;
use worker::{
  Context, Env, Message, MessageBatch, MessageExt, Method, Request, RequestInit, Result,
  console_error, console_log, event,
};

use crate::helpers::write_telemetry_default;
use crate::helpers::{
  build_line_protocol, check_telemetry_alerts, post_line_protocol, url_encode_component,
};
use crate::objects::pigeons::TelemetryWriteResult;

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
    Ok(mut resp) if resp.status_code() < 400 => {
      console_log!("Telemetry consumer: wrote metrics for '{}'", body.pigeon_id);
      message.ack();

      // The DO's write response (task #18, part 2) hands back this
      // pigeon's telemetry_endpoint alongside the metrics it just wrote,
      // so we can decide where this report goes without a second DO round
      // trip: forward as line protocol if a PER-PIGEON endpoint is
      // configured (still takes precedence over everything else), otherwise
      // `write_telemetry_default` (task #26) decides between the
      // platform's own GreptimeDB and our best-effort PG history fallback
      // -- matches this codebase's established best-effort PG sync
      // convention either way: log and move on, never fail/retry the
      // queue message once the DO write (the source of truth) already
      // succeeded.
      match resp.json::<TelemetryWriteResult>().await {
        Ok(TelemetryWriteResult {
          metrics,
          telemetry_endpoint: Some(endpoint),
          previous_values: _,
        }) => {
          if let Err(e) =
            forward_line_protocol(&endpoint, &body.pigeon_id, &metrics, body.reported_at_ms).await
          {
            console_error!(
              "Telemetry consumer: line-protocol forward to '{}' failed for '{}': {e}",
              endpoint.url,
              body.pigeon_id
            );
          }
        }
        Ok(TelemetryWriteResult {
          metrics,
          telemetry_endpoint: None,
          previous_values,
        }) => {
          if let Err(e) =
            write_telemetry_default(env, &body.pigeon_id, &metrics, body.reported_at_ms).await
          {
            console_error!(
              "Telemetry consumer: default write failed for '{}': {e}",
              body.pigeon_id
            );
          }

          // Alert evaluation (task #32, extended #39) -- best-effort,
          // alongside the default write above, same "log and move on,
          // never fail/retry the queue message" convention.
          if let Err(e) = check_telemetry_alerts(
            env,
            &body.pigeon_id,
            &metrics,
            &previous_values,
            body.reported_at_ms,
          )
          .await
          {
            console_error!(
              "Telemetry consumer: alert evaluation failed for '{}': {e}",
              body.pigeon_id
            );
          }
        }
        Err(e) => {
          // Fall back to the pre-task-#18 behavior: re-parse the queue
          // message's own metrics_json (independent of the DO response
          // shape) and write our own default (task #26: Greptime-or-PG),
          // so a response-parsing mismatch doesn't silently drop
          // telemetry that already landed in the DO.
          console_error!(
            "Telemetry consumer: failed to parse DO write result for '{}', falling back to default write: {e}",
            body.pigeon_id
          );
          match serde_json::from_str::<std::collections::HashMap<String, String>>(
            &body.metrics_json,
          ) {
            Ok(metrics) => {
              if let Err(e) = write_telemetry_default(
                env,
                &body.pigeon_id,
                &metrics,
                body.reported_at_ms,
              )
              .await
              {
                console_error!(
                  "Telemetry consumer: default write failed for '{}': {e}",
                  body.pigeon_id
                );
              }

              // Alert evaluation (task #32, extended #39) -- same
              // best-effort convention as the fallback write above.
              // `previous_values` isn't available here -- the DO's own
              // response (the only place it's carried) is exactly what
              // failed to parse -- so RateOfChange can't be evaluated on
              // this degraded path; Threshold still can, same as before.
              if let Err(e) = check_telemetry_alerts(
                env,
                &body.pigeon_id,
                &metrics,
                &std::collections::HashMap::new(),
                body.reported_at_ms,
              )
              .await
              {
                console_error!(
                  "Telemetry consumer: alert evaluation failed for '{}': {e}",
                  body.pigeon_id
                );
              }
            }
            Err(e) => console_error!(
              "Telemetry consumer: failed to re-parse metrics_json for default write ('{}'): {e}",
              body.pigeon_id
            ),
          }
        }
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

/// Forwards one device telemetry report as an InfluxDB line protocol v2
/// HTTP write (GreptimeDB-compatible) to a pigeon's user-configured
/// `telemetry_endpoint` (task #18, part 2) -- taken INSTEAD of the
/// platform default (our own GreptimeDB, or PG history -- see
/// `write_telemetry_default`, task #26) once a per-pigeon endpoint is set
/// (see `capsules::TelemetryEndpoint`'s doc comment; the DO's own
/// latest-value upsert always happens regardless). `endpoint.url` is the
/// user's full write URL (e.g. `https://host:4000/v1/influxdb/write`) --
/// we only ever append `precision`/`db` query params, never assume a
/// particular path, since GreptimeDB/InfluxDB deployments vary.
///
/// Line-building and the actual HTTP POST are shared with
/// `helpers::write_telemetry_default`'s own Greptime write via
/// `build_line_protocol`/`post_line_protocol` (`helpers/greptime.rs`,
/// task #26) -- **deliberately passes `&[]` for `extra_headers`**: this is
/// a per-pigeon, user-configured URL, so it must never carry this Worker's
/// own Cloudflare Access service-token headers (those are only for our own
/// `GREPTIMEDB_ENDPOINT` origin -- see `greptime.rs`'s doc comments on
/// why leaking them here would be a real credential leak).
async fn forward_line_protocol(
  endpoint: &TelemetryEndpoint,
  pigeon_id: &str,
  metrics: &std::collections::HashMap<String, String>,
  reported_at_ms: u64,
) -> Result<()> {
  let line = build_line_protocol(pigeon_id, metrics, reported_at_ms);

  let mut url = endpoint.url.clone();
  url.push(if url.contains('?') { '&' } else { '?' });
  url.push_str("precision=ms");
  if let Some(db) = &endpoint.db {
    url.push_str("&db=");
    url.push_str(&url_encode_component(db));
  }

  post_line_protocol(&url, &line, endpoint.auth_token.as_deref(), &[]).await
}
