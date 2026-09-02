use capsules::TelemetryEndpoint;
use worker::{
  Context, Env, Message, MessageBatch, MessageExt, Method, Request, RequestInit, Result, Url,
  console_error, console_log, event,
};

use crate::helpers::ResolvedReading;
use crate::helpers::write_telemetry_default_batch;
use crate::helpers::{
  build_line_protocol_batch, check_telemetry_alerts_batch, count_billable_messages,
  post_line_protocol, stamp_forwarded_report, url_encode_component,
};
use crate::objects::pigeons::{
  PreviousTelemetryValue, TelemetryEndpointLookup, TelemetryWriteResult,
};

/// Message enqueued by two producers: the `POST /device/pigeons/:id/telemetry`
/// gateway route once it verifies the device's bearer token against the
/// owning DO, and the WebSocket `telemetry` frame handler
/// (`handle_ws_telemetry`, `objects/pigeons.rs`). `reported_at_ms` is when
/// the report was accepted -- informational only for a batch, whose
/// readings each carry their own already-clamped timestamp.
///
/// Every payload field is a pre-serialized JSON **string**, never a
/// `HashMap` or `Vec`: `Queue::send` serializes through
/// serde-wasm-bindgen, which turns a Rust map into a JS `Map`, and the
/// queue's JSON content type then `JSON.stringify`s that `Map` into `{}`
/// -- silently emptying every report. Strings survive every serializer
/// identically.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct TelemetryMessage {
  pub pigeon_id: String,
  pub reported_at_ms: u64,
  /// This report's readings, chronological, each with its timestamp
  /// already resolved and clamped by the producer -- one entry for a flat
  /// report, up to `capsules::MAX_TELEMETRY_BATCH_READINGS` for a batch.
  /// The clamp happens at the producer because that is where the receive
  /// time is known; nothing downstream re-derives a timestamp.
  ///
  /// `#[serde(default)]` because a message enqueued by a previous deploy
  /// carries `metrics_json` instead -- see `readings_of` below, which
  /// reads either.
  #[serde(default)]
  pub readings_json: Option<String>,
  /// Set when the Durable Object already merged these readings into its
  /// store and captured each one's previous values at that moment: the
  /// WebSocket path, which writes synchronously at ingest and enqueues
  /// only the history/forwarding tail. Re-running the write here would
  /// upsert a second time and re-read "previous" values that are no
  /// longer previous.
  #[serde(default)]
  pub pre_merged: bool,
  /// A flat report's metrics, as enqueued by a previous deploy. Kept only
  /// so messages already on the queue when this one deploys still land
  /// rather than being ack-dropped; nothing writes it any more.
  #[serde(default)]
  pub metrics_json: String,
  /// The `previous_values` a previous deploy's WebSocket path captured
  /// alongside `metrics_json`. Its presence is also what marks such a
  /// message as already merged, the role `pre_merged` now fills.
  #[serde(default)]
  pub previous_values_json: Option<String>,
}

impl TelemetryMessage {
  /// The readings this message carries, whichever deploy enqueued it.
  /// `None` for a message with no payload at all, which can never become
  /// valid on a retry.
  fn readings_of(&self) -> Option<Vec<ResolvedReading>> {
    if let Some(readings_json) = &self.readings_json {
      return match serde_json::from_str::<Vec<ResolvedReading>>(readings_json) {
        Ok(readings) if !readings.is_empty() => Some(readings),
        Ok(_) => None,
        Err(e) => {
          console_error!(
            "Telemetry consumer: failed to parse readings for '{}': {e}",
            self.pigeon_id
          );
          None
        }
      };
    }

    if self.metrics_json.is_empty() {
      return None;
    }

    let metrics =
      serde_json::from_str::<std::collections::HashMap<String, String>>(&self.metrics_json).ok()?;
    if metrics.is_empty() {
      return None;
    }

    let mut reading = ResolvedReading::new((self.reported_at_ms / 1000) as i64, metrics);
    if let Some(previous_values_json) = &self.previous_values_json {
      reading.previous = serde_json::from_str::<
        std::collections::HashMap<String, PreviousTelemetryValue>,
      >(previous_values_json)
      .ok();
    }
    Some(vec![reading])
  }

  /// Whether the owning Durable Object has already applied these readings.
  fn already_merged(&self) -> bool {
    self.pre_merged || self.previous_values_json.is_some()
  }
}

/// Queue consumer for `pidgeiot-telemetry` (bound as `TELEMETRY_QUEUE` in
/// both `[env.staging.queues]` and the default/production `[[queues.*]]`
/// blocks of `wrangler.toml`). Dispatches each message to its owning
/// pigeon's DO, keeping the DO's SQLite `pigeon_telemetry` table as the
/// store. Acks/retries per-message rather than failing the whole batch, so
/// one malformed pigeon_id doesn't hold up every other device's report.
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

  let Some(readings) = body.readings_of() else {
    console_error!(
      "Telemetry consumer: empty/undecodable message for '{}', dropping",
      body.pigeon_id
    );
    // Will never decode on retry either -- ack to drop.
    message.ack();
    return;
  };

  if body.already_merged() {
    dispatch_pre_merged(&stub, env, message, readings).await
  } else {
    dispatch_write(&stub, env, message, readings).await
  }
}

/// Not-yet-written path (the gateway's queue producer in `lib.rs`): the
/// device's token was verified at enqueue time but nothing has been
/// merged, so the trusted-internal `/pigeon/device/telemetry/write` route
/// (`write_telemetry_device`, `objects/pigeons.rs`) does the merge AND the
/// previous-value capture, for the whole batch, in one DO round trip.
async fn dispatch_write(
  stub: &worker::Stub,
  env: &Env,
  message: &Message<TelemetryMessage>,
  readings: Vec<ResolvedReading>,
) {
  let body = message.body();

  let Ok(payload) = serde_json::to_string(&readings) else {
    console_error!(
      "Telemetry consumer: failed to serialize readings for '{}'",
      body.pigeon_id
    );
    message.retry();
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
    Ok(mut resp) if resp.status_code() < 400 => {
      console_log!(
        "Telemetry consumer: wrote {} reading(s) for '{}'",
        readings.len(),
        body.pigeon_id
      );
      message.ack();

      match resp.json::<TelemetryWriteResult>().await {
        Ok(result) => {
          store_and_alert(
            env,
            &body.pigeon_id,
            &result.readings,
            result.telemetry_endpoint.as_ref(),
          )
          .await;
        }
        Err(e) => {
          // Fall back to the readings the message itself carried, which
          // are independent of the DO response shape, so a parsing
          // mismatch doesn't silently drop telemetry that already landed
          // in the DO. Their `previous` chains are the one thing only the
          // DO could fill in, so `RateOfChange` can't be evaluated on this
          // degraded path; `Threshold` still can. No `telemetry_endpoint`
          // either, for the same reason -- always falls to the platform
          // default here.
          console_error!(
            "Telemetry consumer: failed to parse DO write result for '{}', falling back to the enqueued readings: {e}",
            body.pigeon_id
          );
          store_and_alert(env, &body.pigeon_id, &readings, None).await;
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

/// Already-written path (`handle_ws_telemetry`, `objects/pigeons.rs`).
/// Unlike the path above, the store was already merged synchronously
/// before this message was enqueued, and each reading's true previous
/// values were captured at that same moment. Re-running the write here
/// would both merge a second time for no reason and re-read "previous"
/// values that are no longer previous (that second read would see what
/// the ingest write already stored). So this asks the DO only for the one
/// piece of state the message doesn't already carry -- this pigeon's
/// `telemetry_endpoint` -- via the read-only
/// `/pigeon/device/telemetry/endpoint` route
/// (`read_telemetry_endpoint_device`).
async fn dispatch_pre_merged(
  stub: &worker::Stub,
  env: &Env,
  message: &Message<TelemetryMessage>,
  readings: Vec<ResolvedReading>,
) {
  let body = message.body();

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
        "Telemetry consumer: {} reading(s) for '{}' already merged at ingest time",
        readings.len(),
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

      store_and_alert(env, &body.pigeon_id, &readings, telemetry_endpoint.as_ref()).await;
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
/// alert" tail for both dispatch paths above. Forwards as line protocol to
/// a configured per-pigeon `telemetry_endpoint` if one exists (alerts
/// aren't evaluated in that branch -- a report forwarded externally isn't
/// stored in our own history); otherwise writes the platform default
/// (Greptime or PG history) and evaluates alerts across the batch.
///
/// Whichever branch it takes, a batch costs the same one round trip a
/// single report does. That is the whole arithmetic of this feature: a
/// device reporting every 10s books 259,200 reports a month, and folding
/// M of them into one delivery divides the worker request, the two DO
/// round trips and the three queue operations by M. What does NOT divide
/// is what follows -- the history rows, the line-protocol lines, and the
/// billable count -- because those measure readings, not envelopes.
async fn store_and_alert(
  env: &Env,
  pigeon_id: &str,
  readings: &[ResolvedReading],
  telemetry_endpoint: Option<&TelemetryEndpoint>,
) {
  // Billable-message tally -- one per READING, not one per delivery, and
  // the same count regardless of which store the readings land in below
  // (our history OR a forwarding endpoint both cost the customer the same
  // reports). Counted here in the consumer, off the device path;
  // internally best-effort, so a failed increment undercounts in the
  // customer's favour rather than failing or delaying ingestion.
  count_billable_messages(env, pigeon_id, readings.len() as i64).await;

  match telemetry_endpoint {
    Some(endpoint) => {
      if let Err(e) = forward_line_protocol(endpoint, pigeon_id, readings).await {
        console_error!(
          "Telemetry consumer: line-protocol forward to '{}' failed for '{}': {e}",
          redact_endpoint_host(&endpoint.url),
          pigeon_id
        );
      }

      // A forwarded report leaves no history row, so this stamp is the
      // only evidence the scheduled evaluator gets that the device spoke.
      stamp_forwarded_report(env, pigeon_id).await;
    }
    None => {
      if let Err(e) = write_telemetry_default_batch(env, pigeon_id, readings).await {
        console_error!(
          "Telemetry consumer: default write failed for '{}': {e}",
          pigeon_id
        );
      }

      // Alert evaluation -- best-effort, alongside the default write
      // above, same "log and move on, never fail/retry the queue message"
      // convention.
      if let Err(e) = check_telemetry_alerts_batch(env, pigeon_id, readings).await {
        console_error!(
          "Telemetry consumer: alert evaluation failed for '{}': {e}",
          pigeon_id
        );
      }
    }
  }
}

/// Forwards a device telemetry report as an InfluxDB line protocol v2
/// HTTP write (GreptimeDB-compatible) to a pigeon's user-configured
/// `telemetry_endpoint` -- taken INSTEAD of the platform default (our own
/// GreptimeDB, or PG history) once a per-pigeon endpoint is set; the DO's
/// own latest-value merge always happens regardless. A batch goes as one
/// request carrying one line per reading, which is the protocol's own
/// multi-line form, not a concession we invented. `endpoint.url` is the
/// user's full write URL -- we only ever append `precision`/`db` query
/// params, never assume a particular path, since GreptimeDB/InfluxDB
/// deployments vary.
///
/// Line-building and the HTTP POST are shared with
/// `helpers::write_telemetry_default_batch`'s own Greptime write via
/// `build_line_protocol_batch`/`post_line_protocol` -- deliberately passes
/// `&[]` for `extra_headers`: this is a per-pigeon, user-configured URL,
/// so it must never carry this Worker's own Cloudflare Access service-token
/// headers (those are only for our own `GREPTIMEDB_ENDPOINT` origin;
/// leaking them here would be a real credential leak).
async fn forward_line_protocol(
  endpoint: &TelemetryEndpoint,
  pigeon_id: &str,
  readings: &[ResolvedReading],
) -> Result<()> {
  let line = build_line_protocol_batch(pigeon_id, readings);
  if line.is_empty() {
    return Ok(());
  }

  let mut url = endpoint.url.clone();
  url.push(if url.contains('?') { '&' } else { '?' });
  url.push_str("precision=ms");
  if let Some(db) = &endpoint.db {
    url.push_str("&db=");
    url.push_str(&url_encode_component(db));
  }

  post_line_protocol(&url, &line, endpoint.auth_token.as_deref(), &[]).await
}

/// scheme+host of a user-configured telemetry forwarding endpoint, for log
/// lines. `endpoint.url` is user-supplied and can embed credentials as
/// userinfo (`https://user:pass@host/...`) or a query param (InfluxDB-style
/// `?token=...` endpoints especially) -- neither may reach a log line now
/// that `head_sampling_rate = 1` (`wrangler.toml`) retains every one
/// instead of sampling almost all of them away.
fn redact_endpoint_host(url: &str) -> String {
  match Url::parse(url) {
    Ok(parsed) => match parsed.host_str() {
      Some(host) => format!("{}://{host}", parsed.scheme()),
      None => "(no-host)".to_string(),
    },
    Err(_) => "(unparseable)".to_string(),
  }
}

#[cfg(test)]
mod tests {
  use super::redact_endpoint_host;

  #[test]
  fn redact_endpoint_host_drops_userinfo_path_and_query() {
    assert_eq!(
      redact_endpoint_host("https://user:s3cr3t@influx.example.com:8086/write?token=abc123"),
      "https://influx.example.com"
    );
  }

  #[test]
  fn redact_endpoint_host_falls_back_on_garbage_input() {
    assert_eq!(redact_endpoint_host("not a url"), "(unparseable)");
  }
}
