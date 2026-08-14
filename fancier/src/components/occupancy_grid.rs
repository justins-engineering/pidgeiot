// A binary state across many devices at once, as a grid of cells. Bay
// occupancy is the motivating case -- 24 sensors each reporting occupied or
// free, where the useful view is the whole zone at a glance rather than any
// one sensor's history.
//
// This is a fleet view, not a device view: it needs one reading from each
// of many pigeons, which is the opposite shape from every chart here (many
// readings from one pigeon). That is why it takes a flat list of cells
// rather than a `ChartSeries`, and why it is not a `ChartKind`.
//
// NOT WIRED TO A PAGE YET. The public demo serves exactly one pigeon, so it
// cannot show this honestly, and the flock views that could feed it do not
// fetch per-pigeon telemetry yet. The component and its tests are here so
// the shape is settled and tested before it is depended on.
use dioxus::prelude::*;

/// One device's current state. `None` means the device has not reported --
/// visibly distinct from both states, because "we don't know" is a third
/// answer and colouring it as free would invent an observation.
#[derive(Clone, PartialEq)]
pub struct OccupancyCell {
  /// Shown on hover and to assistive tech. The device's name, not its id.
  pub label: String,
  pub occupied: Option<bool>,
}

/// Counts for the summary line. Unknown is carried separately rather than
/// folded into free, so the totals always add up to the cell count.
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub struct OccupancyTally {
  pub occupied: usize,
  pub free: usize,
  pub unknown: usize,
}

#[allow(dead_code)]
pub fn tally(cells: &[OccupancyCell]) -> OccupancyTally {
  let mut t = OccupancyTally {
    occupied: 0,
    free: 0,
    unknown: 0,
  };
  for cell in cells {
    match cell.occupied {
      Some(true) => t.occupied += 1,
      Some(false) => t.free += 1,
      None => t.unknown += 1,
    }
  }
  t
}

#[component]
pub fn OccupancyGrid(
  title: String,
  cells: Vec<OccupancyCell>,
  /// Rendered under the grid. The caller knows what the states mean --
  /// occupied/free for a bay, open/closed for a valve.
  occupied_label: String,
  free_label: String,
) -> Element {
  if cells.is_empty() {
    return rsx! {};
  }

  let t = tally(&cells);
  let total = cells.len();

  rsx! {
    div { class: "rounded-box border border-base-content/10 bg-base-100 p-5 flex flex-col gap-3",
      div { class: "flex items-center gap-3 flex-wrap",
        span { class: "font-bold", "{title}" }
        span { class: "text-sm text-base-content/60", "{total} devices" }
      }

      // Wraps rather than scrolls: a zone of 200 bays should get taller,
      // not disappear off the side of a phone.
      div { class: "grid grid-cols-8 sm:grid-cols-12 gap-2",
        for (i , cell) in cells.iter().enumerate() {
          span {
            key: "{i}",
            class: match cell.occupied {
                Some(true) => "rounded h-[26px] bg-primary",
                Some(false) => "rounded h-[26px] bg-base-300",
                None => "rounded h-[26px] border border-dashed border-base-content/30",
            },
            title: match cell.occupied {
                Some(true) => format!("{} — {occupied_label}", cell.label),
                Some(false) => format!("{} — {free_label}", cell.label),
                None => format!("{} — no report", cell.label),
            },
          }
        }
      }

      // Every state named in text as well as coloured. A status never means
      // anything by hue alone -- the same rule the chart's firing reference
      // follows.
      div { class: "flex flex-wrap gap-x-5 gap-y-1 text-sm",
        span {
          span { class: "font-bold", "{t.occupied}" }
          " {occupied_label}"
        }
        span {
          span { class: "font-bold", "{t.free}" }
          " {free_label}"
        }
        if t.unknown > 0 {
          span { class: "text-base-content/60",
            span { class: "font-bold", "{t.unknown}" }
            " not reporting"
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{OccupancyCell, tally};

  fn cell(occupied: Option<bool>) -> OccupancyCell {
    OccupancyCell {
      label: "bay".into(),
      occupied,
    }
  }

  #[test]
  fn counts_each_state_separately() {
    let cells = vec![
      cell(Some(true)),
      cell(Some(true)),
      cell(Some(false)),
      cell(None),
    ];
    let t = tally(&cells);
    assert_eq!(t.occupied, 2);
    assert_eq!(t.free, 1);
    assert_eq!(t.unknown, 1);
  }

  /// The three counts must account for every cell. If unknown were folded
  /// into free, a zone with dead sensors would read as emptier than it is
  /// -- which for parking is the error that sends a driver to a full bay.
  #[test]
  fn the_counts_always_add_up_to_the_cell_count() {
    let cells = vec![
      cell(Some(true)),
      cell(None),
      cell(None),
      cell(Some(false)),
      cell(Some(true)),
    ];
    let t = tally(&cells);
    assert_eq!(t.occupied + t.free + t.unknown, cells.len());
  }

  #[test]
  fn an_all_silent_zone_is_unknown_not_free() {
    let t = tally(&[cell(None), cell(None)]);
    assert_eq!(t.free, 0);
    assert_eq!(t.unknown, 2);
  }
}
