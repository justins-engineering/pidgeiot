// Latest value per telemetry key, as a compact tile set. Reads the exact
// `GET /pigeons/:id/telemetry` snapshot the pigeon page already fetches for
// its connection badge -- no new route, no new device traffic.
//
// This is the one visualization that snapshot honestly supports. It is a
// set of independent readings, each from whenever its own key was last
// reported, NOT a series: there is no time axis here and there must not be
// one, which is why every tile carries its own age rather than the card
// carrying one shared "as of" line that would be wrong for any key that
// arrived at a different moment.
//
// No sparkline and no delta, though the house stat-tile contract allows
// both: both need a previous value, and latest-value-per-key does not have
// one. The graphs below on the same page are where change over time lives.
use crate::helpers::connection_state::format_last_seen;
use crate::helpers::gps_track;
use capsules::TelemetryLatest;
use dioxus::prelude::*;
use time::OffsetDateTime;

/// Groups the integer part of an already-formatted number, so a tile reads
/// 3,300 rather than 3300. Hand-rolled because the alternative is a
/// locale/format crate for one line of thousands separators.
fn group_thousands(text: &str) -> String {
  let (sign, rest) = match text.strip_prefix('-') {
    Some(rest) => ("-", rest),
    None => ("", text),
  };
  let (integer, fraction) = match rest.split_once('.') {
    Some((i, f)) => (i, Some(f)),
    None => (rest, None),
  };

  let mut grouped = String::with_capacity(integer.len() + integer.len() / 3);
  for (i, c) in integer.chars().enumerate() {
    if i > 0 && (integer.len() - i) % 3 == 0 {
      grouped.push(',');
    }
    grouped.push(c);
  }

  match fraction {
    Some(f) => format!("{sign}{grouped}.{f}"),
    None => format!("{sign}{grouped}"),
  }
}

/// Tile values compact above ten thousand, where the extra digits stop
/// being readable at a glance and the tile's `title` still carries the
/// exact reported string. Below that the number is shown in full, since
/// rounding a reading like 3,300 mV would be losing precision to save four
/// characters.
fn compact_value(v: f64) -> String {
  let abs = v.abs();
  if abs >= 1e9 {
    format!("{:.1}B", v / 1e9)
  } else if abs >= 1e6 {
    format!("{:.1}M", v / 1e6)
  } else if abs >= 1e4 {
    format!("{:.1}K", v / 1e3)
  } else {
    let rounded = (v * 100.0).round() / 100.0;
    let text = if rounded.fract() == 0.0 {
      format!("{rounded:.0}")
    } else {
      format!("{rounded}")
    };
    group_thousands(&text)
  }
}

/// Numeric readings get a tile; everything else is a chip. `gps_lat`/
/// `gps_lon` parse as numbers but are dropped from both -- a raw
/// coordinate read as two unrelated decimals says nothing, and the track
/// widget directly above renders the same fix as an actual position.
fn split_readings(
  latest: &[TelemetryLatest],
) -> (Vec<(&TelemetryLatest, f64)>, Vec<&TelemetryLatest>) {
  let mut numeric: Vec<(&TelemetryLatest, f64)> = Vec::new();
  let mut other: Vec<&TelemetryLatest> = Vec::new();

  for reading in latest {
    if gps_track::is_line_graph_excluded(&reading.key) {
      continue;
    }
    match reading.value.trim().parse::<f64>() {
      Ok(v) => numeric.push((reading, v)),
      Err(_) => other.push(reading),
    }
  }

  numeric.sort_by(|a, b| a.0.key.cmp(&b.0.key));
  other.sort_by(|a, b| a.key.cmp(&b.key));
  (numeric, other)
}

#[component]
pub fn TelemetryStatTiles(latest: Vec<TelemetryLatest>) -> Element {
  let now = OffsetDateTime::now_utc();
  let (numeric, other) = split_readings(&latest);

  if numeric.is_empty() && other.is_empty() {
    return rsx! {};
  }

  rsx! {
    div { class: "w-full flex flex-col gap-4 bg-base-100 p-6 rounded-box border border-base-content/10 shadow-sm",
      div { class: "md:px-4",
        h2 { class: "text-3xl font-bold", "Latest readings" }
        p { class: "text-xs text-base-content/50 mt-1",
          "The most recent value reported for each key. Each is timed on its own: a device does not have to report every key together."
        }
      }

      if !numeric.is_empty() {
        div { class: "grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3 md:px-4",
          for (reading , value) in numeric.iter() {
            div {
              key: "{reading.key}",
              class: "border border-base-content/10 rounded-box p-3 flex flex-col gap-0.5",
              // The exact reported string, since the displayed value
              // compacts above ten thousand.
              title: "{reading.key} = {reading.value}",
              span { class: "text-xs text-base-content/60 truncate", "{reading.key}" }
              span { class: "text-2xl font-semibold leading-tight", "{compact_value(*value)}" }
              span { class: "text-[11px] text-base-content/50",
                "{format_last_seen(Some(reading.reported_at), now)}"
              }
            }
          }
        }
      }

      if !other.is_empty() {
        div { class: "flex flex-col gap-2 md:px-4",
          p { class: "text-xs text-base-content/60",
            "Non-numeric readings. These carry no value a chart can plot."
          }
          div { class: "flex flex-wrap gap-2",
            for reading in other.iter() {
              div {
                key: "{reading.key}",
                class: "badge badge-ghost gap-1.5 py-3",
                title: "{format_last_seen(Some(reading.reported_at), now)}",
                span { class: "text-base-content/60", "{reading.key}" }
                span { class: "font-semibold", "{reading.value}" }
              }
            }
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{compact_value, group_thousands, split_readings};
  use capsules::TelemetryLatest;
  use time::OffsetDateTime;

  fn latest(key: &str, value: &str) -> TelemetryLatest {
    TelemetryLatest {
      key: key.to_string(),
      value: value.to_string(),
      reported_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  #[test]
  fn group_thousands_handles_sign_and_fraction() {
    assert_eq!(group_thousands("3300"), "3,300");
    assert_eq!(group_thousands("-1234567"), "-1,234,567");
    assert_eq!(group_thousands("999"), "999");
    assert_eq!(group_thousands("1234.56"), "1,234.56");
  }

  /// A battery reading is shown in full; compacting it would trade real
  /// precision for four characters.
  #[test]
  fn values_below_ten_thousand_keep_every_digit() {
    assert_eq!(compact_value(3300.0), "3,300");
    assert_eq!(compact_value(-71.5), "-71.5");
    assert_eq!(compact_value(9999.0), "9,999");
  }

  #[test]
  fn large_values_compact() {
    assert_eq!(compact_value(12_900.0), "12.9K");
    assert_eq!(compact_value(4_200_000.0), "4.2M");
    assert_eq!(compact_value(3_000_000_000.0), "3.0B");
  }

  #[test]
  fn non_numeric_readings_become_chips_not_tiles() {
    let readings = vec![
      latest("battery_mv", "3300"),
      latest("fw_version", "0.13.3"),
      latest("state", "charging"),
    ];
    let (numeric, other) = split_readings(&readings);
    assert_eq!(numeric.len(), 1);
    assert_eq!(numeric[0].0.key, "battery_mv");
    assert_eq!(
      other.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
      vec!["fw_version", "state"]
    );
  }

  /// The track widget renders the same fix as a position; two loose
  /// decimals beside it would be worse, not more.
  #[test]
  fn raw_coordinates_are_left_to_the_track_widget() {
    let readings = vec![
      latest("gps_lat", "40.7128"),
      latest("gps_lon", "-74.0060"),
      latest("gps_sats", "8"),
    ];
    let (numeric, other) = split_readings(&readings);
    assert_eq!(numeric.len(), 1);
    assert_eq!(numeric[0].0.key, "gps_sats");
    assert!(other.is_empty());
  }
}
