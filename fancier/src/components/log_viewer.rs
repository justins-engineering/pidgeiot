use crate::helpers::dict_log::{
  LogDictionary, LogEvent, decode_chunks, level_str, render_hexdump, render_plaintext,
};
use crate::helpers::{
  build_tar, connection_state, decode_base64, download_bytes, is_page_hidden, sleep_ms,
};
use crate::models::AlertVariant;
use crate::{api, components::Alert};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdDownload, LdRefreshCw, LdTrash2};
use std::rc::Rc;

/// One `capsules::PigeonLogChunk` with its base64 `data` already decoded to
/// raw bytes, decoded once up front (`LogViewer`'s `use_resource`) rather
/// than on every row render or download click -- these are Zephyr
/// dictionary-log records, opaque binary either way, so there's nothing
/// gained by deferring the decode.
#[derive(Clone, PartialEq)]
struct DecodedChunk {
  id: i64,
  received_at: time::OffsetDateTime,
  bytes: Vec<u8>,
}

#[derive(Clone, PartialEq)]
enum LogsState {
  Loading,
  Loaded(Vec<DecodedChunk>),
  Failed,
}

/// Whether this pigeon has a usable `log_dictionary.json` stored
/// (`GET /pigeons/:id/log-dictionary` -- docs/api.md's "Log dictionary"
/// section). `Loaded` carries the parsed dictionary the decode memo below
/// runs against; `Missing` is the state that shows the inline upload
/// affordance; `Invalid` means something IS stored but this build of the
/// dashboard can't parse it (corrupt, or an unsupported database version)
/// -- shown with a replace affordance rather than pretending nothing's
/// there.
#[derive(Clone)]
enum DictState {
  Loading,
  Missing,
  Loaded(Rc<LogDictionary>),
  Invalid(String),
  Failed,
}

fn format_bytes(len: usize) -> String {
  if len < 1024 {
    format!("{len} B")
  } else {
    format!("{:.1} KB", len as f64 / 1024.0)
  }
}

fn chunk_filename(pigeon_id: &str, chunk_id: i64) -> String {
  let short_id = &pigeon_id[..12.min(pigeon_id.len())];
  format!("pigeon-{short_id}-log-{chunk_id}.bin")
}

fn decoded_filename(pigeon_id: &str) -> String {
  let short_id = &pigeon_id[..12.min(pigeon_id.len())];
  format!("pigeon-{short_id}-logs.txt")
}

fn raw_archive_filename(pigeon_id: &str) -> String {
  let short_id = &pigeon_id[..12.min(pigeon_id.len())];
  format!("pigeon-{short_id}-logs.tar")
}

/// DaisyUI badge class per log level. `badge-ghost` (NOT `badge-neutral`)
/// for the odd levels -- `--color-neutral` in this theme is the documented
/// white-on-white/black-on-black invisibility trap (see CLAUDE.md's
/// connection-badge note).
fn level_badge(level: u8) -> &'static str {
  match level {
    1 => "badge-error",
    2 => "badge-warning",
    3 => "badge-success",
    4 => "badge-info",
    _ => "badge-ghost",
  }
}

/// File input that uploads a `log_dictionary.json` for this pigeon.
/// Validates client-side first (size cap + a real parse via the same
/// `LogDictionary::parse` the decoder uses) so a wrong file is rejected
/// with a specific message before any bytes leave the browser; on a
/// successful PUT the parsed dictionary goes straight into `dict_state`,
/// so decoding starts without a refetch.
#[component]
fn DictionaryUpload(pigeon_id: String, dict_state: Signal<DictState>) -> Element {
  let mut upload_error: Signal<Option<String>> = use_signal(|| None);
  let mut uploading = use_signal(|| false);

  rsx! {
    div { class: "flex flex-col gap-1",
      div { class: "flex flex-row items-center gap-2",
        input {
          r#type: "file",
          accept: ".json,application/json",
          class: "file-input file-input-bordered file-input-sm",
          disabled: uploading(),
          onchange: move |evt: Event<FormData>| {
              let pid = pigeon_id.clone();
              async move {
                  upload_error.set(None);
                  let Some(file) = evt.files().into_iter().next() else {
                      return;
                  };
                  if file.size() > capsules::MAX_LOG_DICTIONARY_BYTES as u64 {
                      upload_error
                          .set(
                              Some(
                                  format!(
                                      "File is {} KB, which is over the {} KB limit.",
                                      file.size() / 1024,
                                      capsules::MAX_LOG_DICTIONARY_BYTES / 1024,
                                  ),
                              ),
                          );
                      return;
                  }
                  let bytes = match file.read_bytes().await {
                      Ok(b) => b.to_vec(),
                      Err(err) => {
                          upload_error.set(Some(format!("Failed to read file: {err}")));
                          return;
                      }
                  };
                  let Ok(text) = std::str::from_utf8(&bytes) else {
                      upload_error
                          .set(
                              Some(
                                  "Not a log_dictionary.json file (expected UTF-8 JSON)."
                                      .to_string(),
                              ),
                          );
                      return;
                  };
                  let dict = match LogDictionary::parse(text) {
                      Ok(d) => d,
                      Err(e) => {
                          upload_error.set(Some(format!("Not a usable log dictionary: {e}")));
                          return;
                      }
                  };
                  uploading.set(true);
                  let result = api::pigeons::put_log_dictionary(&pid, &bytes).await;
                  uploading.set(false);
                  match result {
                      Some(_info) => dict_state.set(DictState::Loaded(Rc::new(dict))),
                      None => {
                          upload_error
                              .set(
                                  Some(
                                      "Upload failed. Check your connection and try again."
                                          .to_string(),
                                  ),
                              );
                      }
                  }
              }
          },
        }
        if uploading() {
          span { class: "loading loading-spinner loading-sm" }
        }
      }
      if let Some(err) = upload_error.read().as_ref() {
        p { class: "text-error text-xs", "{err}" }
      }
    }
  }
}

/// Device log chunk list for the pigeon detail page. `GET /pigeons/:id/logs`
/// returns Zephyr `CONFIG_LOG_DICTIONARY_SUPPORT` binary records; when this
/// pigeon has a `log_dictionary.json` stored, they're decoded right here via
/// `helpers::dict_log` into readable lines -- otherwise the viewer shows an
/// inline upload affordance and, as always, the raw chunk download path
/// (which never fakes a text rendering of undecodable binary data).
/// One fetch-decode-set cycle, shared by the auto-refresh loop and the
/// manual refresh button below -- `on_latest_received` fires every cycle,
/// not just the first, so a caller's "last seen" freshens on each poll a new
/// chunk actually arrives.
async fn refresh_logs(
  pigeon_id: &str,
  mut state: Signal<LogsState>,
  on_latest_received: &EventHandler<Option<time::OffsetDateTime>>,
) {
  match api::pigeons::get_logs(pigeon_id).await {
    Some(raw_chunks) => {
      let decoded: Vec<DecodedChunk> = raw_chunks
        .into_iter()
        .filter_map(|chunk| match decode_base64(&chunk.data) {
          Some(bytes) => Some(DecodedChunk {
            id: chunk.id,
            received_at: chunk.received_at,
            bytes,
          }),
          None => {
            error!("Failed to base64-decode log chunk {} as base64", chunk.id);
            None
          }
        })
        .collect();
      on_latest_received.call(decoded.iter().map(|c| c.received_at).max());
      state.set(LogsState::Loaded(decoded));
    }
    // A failed fetch on a poll that follows a real load must not blank the
    // chunk table or tell the caller "last seen" just went away -- keep
    // showing the last good state and let the next tick retry, same as a
    // dropped request the user never has to know about. Only the very
    // first fetch (nothing loaded yet) surfaces `Failed`. `peek()`, not
    // `read()`: an untracked read so this async fn (spawned by
    // `use_future`) never subscribes to the very signal it's about to
    // write, which would otherwise restart the polling loop on its own
    // `set()` below.
    None => {
      if matches!(*state.peek(), LogsState::Loading) {
        on_latest_received.call(None);
        state.set(LogsState::Failed);
      }
    }
  }
}

#[component]
pub fn LogViewer(
  pigeon_id: String,
  /// This pigeon's own `telemetry_interval` (seconds) -- same self-calibrated
  /// auto-refresh cadence `GraphCard` uses (`connection_state::
  /// poll_interval_ms`), so the log viewer and telemetry graphs on the same
  /// page settle into the same rhythm rather than two independently-tuned
  /// polling loops against the same pigeon.
  interval_secs: Option<i64>,
  /// Fired once each fetch settles, with the newest chunk's `received_at`
  /// (or `None` on an empty/failed fetch) -- lets a caller derive "last
  /// seen" from the chunks LogViewer already fetched, instead
  /// of re-fetching potentially 200 base64-encoded chunks a second time
  /// just to read a timestamp.
  on_latest_received: EventHandler<Option<time::OffsetDateTime>>,
) -> Element {
  let time_format = time::macros::format_description!(
    "[month repr:short] [day padding:none], [year] at [hour]:[minute]:[second] UTC"
  );
  let state: Signal<LogsState> = use_signal(|| LogsState::Loading);
  let mut dict_state: Signal<DictState> = use_signal(|| DictState::Loading);
  let mut refreshing = use_signal(|| false);

  // Fetches immediately on mount, then keeps re-polling for new chunks at
  // this pigeon's self-calibrated cadence for as long as the viewer stays
  // mounted (cancelled on unmount, same as any other `use_future`) -- skips
  // the request entirely while the tab is backgrounded so an idle dashboard
  // doesn't keep hammering the DO's log ring buffer.
  {
    let fetch_id = pigeon_id.clone();
    use_future(move || {
      let id = fetch_id.clone();
      async move {
        let poll_ms = connection_state::poll_interval_ms(interval_secs);
        loop {
          if !is_page_hidden() {
            refresh_logs(&id, state, &on_latest_received).await;
          }
          sleep_ms(poll_ms).await;
        }
      }
    });
  }

  let dict_fetch_id = pigeon_id.clone();
  use_resource(move || {
    let id = dict_fetch_id.clone();
    async move {
      match api::pigeons::get_log_dictionary(&id).await {
        Some(Some(text)) => match LogDictionary::parse(&text) {
          Ok(dict) => dict_state.set(DictState::Loaded(Rc::new(dict))),
          Err(e) => dict_state.set(DictState::Invalid(e)),
        },
        Some(None) => dict_state.set(DictState::Missing),
        None => dict_state.set(DictState::Failed),
      }
    }
  });

  // Decoded event stream, recomputed only when the chunks or the
  // dictionary change. `None` until both halves are available.
  let decoded_events: Memo<Option<Vec<LogEvent>>> = use_memo(move || {
    let LogsState::Loaded(chunks) = &*state.read() else {
      return None;
    };
    let DictState::Loaded(dict) = &*dict_state.read() else {
      return None;
    };
    if chunks.is_empty() {
      return None;
    }
    let chunk_bytes: Vec<Vec<u8>> = chunks.iter().map(|c| c.bytes.clone()).collect();
    Some(decode_chunks(dict, &chunk_bytes))
  });

  let download_all_id = pigeon_id.clone();
  let download_decoded_id = pigeon_id.clone();
  let remove_dict_id = pigeon_id.clone();
  let refresh_id = pigeon_id.clone();

  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        div {
          h2 { class: "text-3xl font-bold", "Device Logs" }
          p { class: "text-xs text-base-content/50",
            "Up to the 200 most recently received chunks, oldest first."
          }
        }
        div { class: "flex flex-row gap-2",
          button {
            class: "btn btn-outline btn-sm",
            r#type: "button",
            title: "Refresh now",
            disabled: refreshing(),
            onclick: move |_| {
                let pigeon_id = refresh_id.clone();
                async move {
                    refreshing.set(true);
                    refresh_logs(&pigeon_id, state, &on_latest_received).await;
                    refreshing.set(false);
                }
            },
            if refreshing() {
              span { class: "loading loading-spinner loading-xs" }
            } else {
              Icon { icon: LdRefreshCw, width: 16, height: 16 }
            }
            " Refresh"
          }
          if decoded_events.read().is_some() {
            button {
              class: "btn btn-outline btn-sm",
              onclick: move |_| {
                  if let Some(events) = decoded_events() {
                      let text = render_plaintext(&events);
                      download_bytes(
                          text.as_bytes(),
                          &decoded_filename(&download_decoded_id),
                          "text/plain",
                      );
                  }
              },
              Icon { icon: LdDownload, width: 16, height: 16 }
              " Decoded .txt"
            }
          }
          if let LogsState::Loaded(chunks) = state.read().clone() {
            if !chunks.is_empty() {
              button {
                class: "btn btn-outline btn-sm",
                title: "Download all raw chunks as one .tar archive",
                onclick: move |_| {
                    let pigeon_id = download_all_id.clone();
                    let entries: Vec<(String, Vec<u8>)> = chunks
                        .iter()
                        .map(|chunk| (chunk_filename(&pigeon_id, chunk.id), chunk.bytes.clone()))
                        .collect();
                    let archive = build_tar(&entries);
                    download_bytes(
                        &archive,
                        &raw_archive_filename(&pigeon_id),
                        "application/x-tar",
                    );
                },
                Icon { icon: LdDownload, width: 16, height: 16 }
                " Raw ({chunks.len()}) .tar"
              }
            }
          }
        }
      }

      // Dictionary status strip: which build's dictionary decodes this
      // pigeon's chunks, or the affordance to provide one.
      match dict_state.read().clone() {
        DictState::Loading => rsx! {
          div { class: "flex items-center gap-2 text-xs text-base-content/50",
            span { class: "loading loading-spinner loading-xs" }
            "Checking for a log dictionary..."
          }
        },
        DictState::Loaded(dict) => rsx! {
          div { class: "flex flex-row flex-wrap items-center gap-2 text-xs",
            span { class: "badge badge-success badge-sm badge-outline", "dictionary" }
            span { class: "text-base-content/70",
              if let Some(build_id) = dict.build_id.as_ref() {
                "Decoding with the uploaded log dictionary (build {build_id})."
              } else {
                "Decoding with the uploaded log dictionary."
              }
            }
            div { class: "grow" }
            DictionaryUpload { pigeon_id: pigeon_id.clone(), dict_state }
            button {
              class: "btn btn-ghost btn-sm text-error",
              title: "Remove the stored dictionary",
              onclick: move |_| {
                  let pid = remove_dict_id.clone();
                  async move {
                      if api::pigeons::delete_log_dictionary(&pid).await.is_some() {
                          dict_state.set(DictState::Missing);
                      }
                  }
              },
              Icon { icon: LdTrash2, width: 16, height: 16 }
            }
          }
        },
        DictState::Missing => rsx! {
          Alert { variant: AlertVariant::Info, persistent: true,
            div { class: "flex flex-col gap-2",
              span {
                "Chunks are raw Zephyr dictionary-log binary. Upload this firmware build's "
                code { class: "text-xs", "log_dictionary.json" }
                " (from the build directory, e.g. "
                code { class: "text-xs", "build/zephyr/log_dictionary.json" }
                ") to decode them right here. It must come from the exact build the device is running. "
                "Alternatively, download raw chunks and decode offline with Zephyr's "
                code { class: "text-xs", "log_parser.py" }
                " (see the pigeon-examples README's dictionary logging section)."
              }
              DictionaryUpload { pigeon_id: pigeon_id.clone(), dict_state }
            }
          }
        },
        DictState::Invalid(e) => rsx! {
          Alert { variant: AlertVariant::Error, persistent: true,
            div { class: "flex flex-col gap-2",
              span {
                "A log dictionary is stored for this pigeon, but it can't be used: {e}. Upload a replacement "
                code { class: "text-xs", "log_dictionary.json" }
                " from the running build."
              }
              DictionaryUpload { pigeon_id: pigeon_id.clone(), dict_state }
            }
          }
        },
        DictState::Failed => rsx! {
          p { class: "text-error text-xs",
            "Couldn't check for a log dictionary. Raw chunk downloads below still work."
          }
        },
      }

      // Decoded, readable lines -- the headline view once a dictionary
      // exists. Timestamps are raw device ticks (the dictionary wire
      // format doesn't standardize a tick frequency; matching Zephyr's own
      // parser output keeps this diffable against an offline decode).
      if let Some(events) = decoded_events.read().as_ref() {
        div { class: "border border-base-content/10 rounded-box bg-base-200/40 font-mono text-xs overflow-x-auto overflow-y-auto max-h-[32rem] p-2 flex flex-col",
          for (i , event) in events.iter().enumerate() {
            {
                match event {
                    LogEvent::Message(m) if m.level == 0 => rsx! {
                      pre { key: "{i}", class: "whitespace-pre-wrap break-all", "{m.text}" }
                    },
                    LogEvent::Message(m) => rsx! {
                      div { key: "{i}", class: "flex flex-row gap-2 items-baseline py-0.5",
                        span { class: "text-base-content/40 shrink-0 w-24 text-right", "{m.timestamp}" }
                        span { class: "badge badge-xs {level_badge(m.level)} shrink-0", "{level_str(m.level)}" }
                        span { class: "text-base-content/60 shrink-0", "{m.source}:" }
                        span { class: "whitespace-pre-wrap break-all", "{m.text}" }
                      }
                      if !m.hexdump.is_empty() {
                        pre { class: "pl-28 text-base-content/70 whitespace-pre",
                          {render_hexdump(&m.hexdump, 0)}
                        }
                      }
                    },
                    LogEvent::Dropped(n) => rsx! {
                      div { key: "{i}", class: "text-warning italic py-0.5", "--- {n} messages dropped ---" }
                    },
                    LogEvent::Error { offset, reason } => rsx! {
                      div { key: "{i}", class: "text-error italic py-0.5",
                        "--- decode error at byte {offset}: {reason} ---"
                      }
                    },
                }
            }
          }
        }
      }

      match state.read().clone() {
        LogsState::Loading => rsx! {
          div { class: "loading loading-spinner text-primary m-4 self-center" }
        },
        LogsState::Failed => rsx! {
          p { class: "text-error text-sm", "Failed to load device logs. Please try again." }
        },
        LogsState::Loaded(chunks) if chunks.is_empty() => rsx! {
          p { class: "text-base-content/50 italic text-sm", "No log chunks received yet." }
        },
        LogsState::Loaded(chunks) => {
          let table = rsx! {
            div { class: "overflow-x-auto",
              table { class: "table",
                thead {
                  tr {
                    th { "ID" }
                    th { "Received" }
                    th { "Size" }
                    th {}
                  }
                }
                tbody {
                  for chunk in chunks.iter() {
                    {
                        let received_at = chunk
                            .received_at
                            .format(&time_format)
                            .unwrap_or_else(|_| "Invalid Format".to_string());
                        let size = format_bytes(chunk.bytes.len());
                        let pigeon_id = pigeon_id.clone();
                        let chunk_id = chunk.id;
                        let bytes = chunk.bytes.clone();
                        rsx! {
                          tr {
                            th { class: "font-mono", "{chunk_id}" }
                            td { class: "font-mono text-sm", "{received_at}" }
                            td { class: "font-mono text-sm", "{size}" }
                            td {
                              button {
                                class: "btn btn-square btn-ghost btn-sm",
                                title: "Download chunk",
                                onclick: move |_| {
                                    let filename = chunk_filename(&pigeon_id, chunk_id);
                                    download_bytes(&bytes, &filename, "application/octet-stream");
                                },
                                Icon { icon: LdDownload }
                              }
                            }
                          }
                        }
                    }
                  }
                }
              }
            }
          };
          // With a decoded view above, the raw table is a secondary
          // concern -- collapse it. Without one, it IS the content.
          if decoded_events.read().is_some() {
            rsx! {
              details { class: "collapse collapse-arrow border border-base-content/10 bg-base-200/40",
                summary { class: "collapse-title text-sm font-semibold", "Raw chunks ({chunks.len()})" }
                div { class: "collapse-content", {table} }
              }
            }
          } else {
            table
          }
        }
      }
    }
  }
}
