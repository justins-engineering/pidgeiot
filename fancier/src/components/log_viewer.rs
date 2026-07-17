use crate::helpers::{decode_base64, download_bytes};
use crate::models::AlertVariant;
use crate::{api, components::Alert};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdDownload;

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

/// Device log chunk list for the pigeon detail page (task #25, part A).
/// `GET /pigeons/:id/logs` (docs/api.md's "Logs" section) is live on
/// staging and prod now, independent of the firmware-upload half of this
/// task. Chunks are Zephyr `CONFIG_LOG_DICTIONARY_SUPPORT` binary records --
/// this dashboard has no access to the firmware build's own
/// `log_dictionary.json` and therefore cannot decode them into text itself
/// (see the sibling `pigeon-examples` repo's README); the only thing this
/// view can responsibly do with the bytes is hand them back to the user
/// as-is for host-side decoding, never fake a text rendering of binary
/// data.
#[component]
pub fn LogViewer(pigeon_id: String) -> Element {
  let time_format = time::macros::format_description!(
    "[month repr:short] [day padding:none], [year] at [hour]:[minute]:[second] UTC"
  );
  let mut state: Signal<LogsState> = use_signal(|| LogsState::Loading);

  let fetch_id = pigeon_id.clone();
  use_resource(move || {
    let id = fetch_id.clone();
    async move {
      match api::pigeons::get_logs(&id).await {
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
          state.set(LogsState::Loaded(decoded));
        }
        None => state.set(LogsState::Failed),
      }
    }
  });

  let download_all_id = pigeon_id.clone();

  rsx! {
    div { class: "w-full flex flex-col justify-between gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "flex flex-row gap-4 items-center justify-between md:px-4",
        div {
          h2 { class: "text-3xl font-bold", "Device Logs" }
          p { class: "text-xs text-base-content/50",
            "Up to the 200 most recently received chunks, oldest first."
          }
        }
        if let LogsState::Loaded(chunks) = state.read().clone() {
          if !chunks.is_empty() {
            button {
              class: "btn btn-outline btn-sm",
              onclick: move |_| {
                  let pigeon_id = download_all_id.clone();
                  for chunk in &chunks {
                      let filename = chunk_filename(&pigeon_id, chunk.id);
                      download_bytes(&chunk.bytes, &filename, "application/octet-stream");
                  }
              },
              Icon { icon: LdDownload, width: 16, height: 16 }
              " Download All ({chunks.len()})"
            }
          }
        }
      }

      Alert { variant: AlertVariant::Info, persistent: true,
        "Chunks are raw Zephyr dictionary-log binary, not readable text -- decoding needs your firmware build's own "
        code { class: "text-xs", "log_dictionary.json" }
        ", which never leaves your machine. Download a chunk, then run: "
        code { class: "text-xs block mt-1",
          "python3 zephyr/scripts/logging/dictionary/log_parser.py build/https_init/zephyr/log_dictionary.json <captured-chunk-file>"
        }
        " -- see the pigeon-examples README's dictionary logging section for the full flow (and the "
        code { class: "text-xs", "colorama" }
        " dependency it calls out)."
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
        LogsState::Loaded(chunks) => rsx! {
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
                for chunk in chunks {
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
        },
      }
    }
  }
}
