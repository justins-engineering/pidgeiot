use capsules::TelemetryEndpoint;
use worker::{
  Context, Env, Message, MessageBatch, MessageExt, Method, Request, RequestInit, Result,
  console_error, console_log, event,
};

use crate::helpers::write_telemetry_default;
use crate::helpers::{
  build_line_protocol, check_telemetry_alerts, post_line_protocol, url_encode_component,
};
use crate::objects::pigeons::{
  PreviousTelemetryValue, TelemetryEndpointLookup, TelemetryWriteResult,
};

/// Message enqueued by two producers: the `POST /device/pigeons/:id/telemetry`
/// gateway route (`lib.rs`) once it has verified the device's bearer token
/// against the owning DO -- see `verify_device_via_do` (`helpers/pigeons.rs`)
/// and `verify_telemetry_device`/`write_telemetry_device`
/// (`objects/pigeons.rs`) -- and the WebSocket `telemetry` frame handler,
/// `handle_ws_telemetry` (`objects/pigeons.rs`, task #32/#41). `reported_at_ms`
/// is when the report was accepted; it's informational only for now -- the
/// DO's own `pigeon_telemetry` rows still stamp `reported_at` at write time
/// via SQLite's `unixepoch()` default, unchanged from the pre-queue
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
  /// Pre-serialized JSON of this report's `PreviousTelemetryValue` map (same
  /// "must be a string, not a raw `HashMap`" reasoning as `metrics_json`
  /// above -- it would hit the identical serde-wasm-bindgen -> JS `Map` ->
  /// `JSON.stringify` == `{}` bug otherwise), captured by `handle_ws_telemetry`
  /// (`objects/pigeons.rs`) at WS-ingest time, BEFORE its own upsert
  /// overwrote `pigeon_telemetry` (task #41).
  ///
  /// `None` for HTTP-sourced messages (`report_telemetry_device`'s
  /// queue-producer route in `lib.rs`): that route enqueues right after a
  /// bare auth check, with no DO round trip that could capture a previous
  /// value, so there is nothing to carry -- `write_telemetry_device`
  /// (dispatched below) does that capture itself, same as before task #41.
  /// `dispatch_to_do` uses this field's presence (not the environment) to
  /// decide which DO route a message goes to. `#[serde(default)]` so
  /// messages enqueued before this field existed decode as `None` rather
  /// than failing to deserialize.
  #[serde(default)]
  pub previous_values_json: Option<String>,
}

/// Queue consumer for `pidgeiot-telemetry` (bound as `TELEMETRY_QUEUE` in
/// both `[env.staging.queues]` and the default/production `[[queues.*]]`
/// blocks of `wrangler.toml` -- promoted to production 2026-07-17,
/// contrary to an earlier claim in this codebase's history that only
/// staging bound it; see task #41). Dispatches each message to its owning
/// pigeon's DO, keeping the DO's SQLite `pigeon_telemetry` table as the
/// store, unchanged from the pre-queue direct-write path. Acks/retries
/// per-message rather than failing the whole batch on one bad message, so a
/// single malformed pigeon_id doesn't hold up every other device's report
/// in the batch.
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

  match &body.previous_values_json {
    Some(previous_values_json) => {
      dispatch_ws_sourced(&stub, env, message, previous_values_json).await
    }
    None => dispatch_http_sourced(&stub, env, message).await,
  }
}

/// HTTP-sourced queue message path (`report_telemetry_device`'s
/// queue-producer route in `lib.rs`) -- unchanged from before task #41: no
/// pre-upsert has happened yet, so the trusted-internal
/// `/pigeon/device/telemetry/write` route (`write_telemetry_device`,
/// `objects/pigeons.rs`) does the read-before-upsert capture AND the
/// upsert itself, in one DO round trip.
async fn dispatch_http_sourced(
  stub: &worker::Stub,
  env: &Env,
  message: &Message<TelemetryMessage>,
) {
  let body = message.body();

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

      match resp.json::<TelemetryWriteResult>().await {
        Ok(TelemetryWriteResult {
          metrics,
          telemetry_endpoint,
          previous_values,
        }) => {
          store_and_alert(
            env,
            &body.pigeon_id,
            &metrics,
            telemetry_endpoint.as_ref(),
            &previous_values,
            body.reported_at_ms,
          )
          .await;
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
              // `previous_values` isn't available here -- the DO's own
              // response (the only place an HTTP-sourced message carries
              // it) is exactly what failed to parse -- so RateOfChange
              // can't be evaluated on this degraded path; Threshold still
              // can, same as before task #41. No `telemetry_endpoint`
              // either, for the same reason -- always falls to the
              // platform default here.
              store_and_alert(
                env,
                &body.pigeon_id,
                &metrics,
                None,
                &std::collections::HashMap::new(),
                body.reported_at_ms,
              )
              .await;
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

/// WS-sourced queue message path (`handle_ws_telemetry`,
/// `objects/pigeons.rs`, task #41). Unlike the HTTP-sourced path above,
/// `pigeon_telemetry` was already upserted synchronously, before this
/// message was even enqueued, and this report's true previous values were
/// already captured at that same moment -- see
/// `TelemetryMessage::previous_values_json`'s doc comment. Re-running
/// `write_telemetry_device` here would both upsert a second time for no
/// reason and re-read "previous" values that are no longer previous (the
/// exact bug this task fixes, since that second read would see the value
/// `handle_ws_telemetry` already wrote). So this path skips
/// `write_telemetry_device` entirely and only asks the DO for the one
/// piece of state it doesn't already have -- this pigeon's
/// `telemetry_endpoint` -- via the read-only `/pigeon/device/telemetry/endpoint`
/// route (`read_telemetry_endpoint_device`), using the metrics and
/// previous-values already carried on the message itself.
async fn dispatch_ws_sourced(
  stub: &worker::Stub,
  env: &Env,
  message: &Message<TelemetryMessage>,
  previous_values_json: &str,
) {
  let body = message.body();

  let Ok(metrics) =
    serde_json::from_str::<std::collections::HashMap<String, String>>(&body.metrics_json)
  else {
    console_error!(
      "Telemetry consumer: failed to parse metrics_json for WS-sourced message '{}', dropping",
      body.pigeon_id
    );
    // Will never parse on retry either.
    message.ack();
    return;
  };

  let previous_values: std::collections::HashMap<String, PreviousTelemetryValue> =
    serde_json::from_str(previous_values_json).unwrap_or_else(|e| {
      console_error!(
        "Telemetry consumer: failed to parse previous_values_json for '{}': {e}",
        body.pigeon_id
      );
      std::collections::HashMap::new()
    });

  let Ok(do_req) = Request::new(
    "https://internal/pigeon/device/telemetry/endpoint",
    Method::Get,
  ) else {
    console_error!(
      "Telemetry consumer: failed to build endpoint-lookup request for '{}'",
      body.pigeon_id
    );
    message.retry();
    return;
  };

  match stub.fetch_with_request(do_req).await {
    Ok(mut resp) if resp.status_code() < 400 => {
      console_log!(
        "Telemetry consumer: WS-sourced metrics for '{}' already upserted at ingest time",
        body.pigeon_id
      );
      message.ack();

      let telemetry_endpoint = match resp.json::<TelemetryEndpointLookup>().await {
        Ok(lookup) => lookup.telemetry_endpoint,
        Err(e) => {
          console_error!(
            "Telemetry consumer: failed to parse endpoint lookup for '{}': {e}",
            body.pigeon_id
          );
          None
        }
      };

      store_and_alert(
        env,
        &body.pigeon_id,
        &metrics,
        telemetry_endpoint.as_ref(),
        &previous_values,
        body.reported_at_ms,
      )
      .await;
    }
    Ok(resp) => {
      console_error!(
        "Telemetry consumer: endpoint lookup for '{}' returned {}",
        body.pigeon_id,
        resp.status_code()
      );
      message.retry();
    }
    Err(e) => {
      console_error!(
        "Telemetry consumer: endpoint lookup fetch failed for '{}': {e}",
        body.pigeon_id
      );
      message.retry();
    }
  }
}

/// Shared "where does this report's history go, and should it trip an
/// alert" tail for both dispatch paths above -- factored out (task #41) to
/// avoid a third copy once the WS-sourced path needed the same decision.
/// Forwards as line protocol to a configured per-pigeon `telemetry_endpoint`
/// if one exists (`previous_values` unused in that branch: RateOfChange
/// isn't evaluated when a report is forwarded externally rather than
/// stored in our own history, unchanged from before this task); otherwise
/// writes the platform default (Greptime or PG history,
/// `write_telemetry_default`) and evaluates alerts against
/// `previous_values`.
async fn store_and_alert(
  env: &Env,
  pigeon_id: &str,
  metrics: &std::collections::HashMap<String, String>,
  telemetry_endpoint: Option<&TelemetryEndpoint>,
  previous_values: &std::collections::HashMap<String, PreviousTelemetryValue>,
  reported_at_ms: u64,
) {
  match telemetry_endpoint {
    Some(endpoint) => {
      if let Err(e) = forward_line_protocol(endpoint, pigeon_id, metrics, reported_at_ms).await {
        console_error!(
          "Telemetry consumer: line-protocol forward to '{}' failed for '{}': {e}",
          endpoint.url,
          pigeon_id
        );
      }
    }
    None => {
      if let Err(e) = write_telemetry_default(env, pigeon_id, metrics, reported_at_ms).await {
        console_error!(
          "Telemetry consumer: default write failed for '{}': {e}",
          pigeon_id
        );
      }

      // Alert evaluation (task #32, extended #39, threaded through for
      // WS-sourced messages by #41) -- best-effort, alongside the default
      // write above, same "log and move on, never fail/retry the queue
      // message" convention.
      if let Err(e) =
        check_telemetry_alerts(env, pigeon_id, metrics, previous_values, reported_at_ms).await
      {
        console_error!(
          "Telemetry consumer: alert evaluation failed for '{}': {e}",
          pigeon_id
        );
      }
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
