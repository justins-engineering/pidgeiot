use dioxus::prelude::*;

/// The sample boards this fleet currently builds firmware for (task #20,
/// phase 1) -- shown as `<datalist>` suggestions on every `board` input
/// (pigeon create/edit, firmware upload) rather than a hard `<select>`,
/// since `board` is an open Zephyr `CONFIG_BOARD_TARGET` string (any
/// vendor-agnostic board this fleet's device library supports builds for
/// is a legal value, not just these three) -- see CLAUDE.md's
/// vendor-agnostic device strategy. A plain text input with suggestions
/// keeps that door open instead of hardcoding a closed enum.
pub const KNOWN_BOARDS: &[&str] = &[
  "circuitdojo_feather/nrf9160/ns",
  "circuitdojo_feather_nrf9151/nrf9151/ns",
  "esp32c6_devkitc/esp32c6/hpcore",
];

/// Shared `<datalist>` id every `board` text input in this crate points its
/// `list` attribute at (`CreatePigeonModal`, `UpdatePigeonModal`,
/// `FirmwareModal`'s upload field) -- one definition, reused by id rather
/// than duplicated per form, so the known-boards list only needs updating
/// in one place. Safe to render more than once on the same page (ids would
/// collide) only because no two of those forms are ever mounted
/// simultaneously (each is a conditionally-rendered/native-`<dialog>`
/// modal) -- if that ever changes, this needs a unique id per caller.
pub const BOARD_DATALIST_ID: &str = "known-boards";

#[component]
pub fn BoardDatalist() -> Element {
  rsx! {
    datalist { id: BOARD_DATALIST_ID,
      for board in KNOWN_BOARDS {
        option { value: "{board}" }
      }
    }
  }
}
