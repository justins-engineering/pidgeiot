// GPS/asset-tracking math for the pigeon detail view's `TrackWidget`
// (components::track_widget) -- a new device sample reports GPS fixes as
// ordinary numeric telemetry keys (gps_lat/gps_lon/gps_alt_m/
// gps_speed_mps/gps_heading_deg/gps_sats/gps_fix_quality). Only
// gps_lat/gps_lon are required to plot a fix -- everything else is
// detected defensively (a device sample that doesn't report e.g.
// gps_sats yet still gets a usable track), not hard-required.
//
// Kept `fancier`-only (not `capsules`): this is a display concern over
// telemetry dovecote already stores as opaque flat key/value pairs --
// the backend has no notion of "GPS" at all, same reasoning as
// `graph_widget`'s client-side-only `GraphDef`.
//
// All the math below is deliberately pure (no web_sys/Dioxus) so it's
// unit-testable on the host target without a wasm build -- same
// precedent as `capsules::connection_state` and `graph_widget`'s
// `numeric_keys_from_*`.
use capsules::{TelemetryHistoryPoint, TelemetryLatest};
use std::collections::BTreeMap;

/// The exact telemetry keys this module knows how to read.
pub const KEY_LAT: &str = "gps_lat";
pub const KEY_LON: &str = "gps_lon";
pub const KEY_ALT_M: &str = "gps_alt_m";
pub const KEY_SPEED_MPS: &str = "gps_speed_mps";
pub const KEY_HEADING_DEG: &str = "gps_heading_deg";
pub const KEY_SATS: &str = "gps_sats";
pub const KEY_FIX_QUALITY: &str = "gps_fix_quality";

/// One assembled GPS fix. `reported_at` is the shared timestamp dovecote
/// stamps onto every key/value pair in a single telemetry report
/// (`Date::now()` at ingestion time, see `report_telemetry_device` in
/// dovecote/src/objects/pigeons.rs) -- grouping `TelemetryHistoryPoint`s
/// by exact `reported_at` reassembles whichever keys a device reported
/// together back into one fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsFix {
  pub reported_at: i64,
  pub lat: f64,
  pub lon: f64,
  pub alt_m: Option<f64>,
  pub speed_mps: Option<f64>,
  pub heading_deg: Option<f64>,
  pub sats: Option<f64>,
  pub fix_quality: Option<f64>,
}

#[derive(Default)]
struct PartialFix {
  lat: Option<f64>,
  lon: Option<f64>,
  alt_m: Option<f64>,
  speed_mps: Option<f64>,
  heading_deg: Option<f64>,
  sats: Option<f64>,
  fix_quality: Option<f64>,
}

/// Reassembles a pigeon's telemetry history into a time-ordered list of
/// GPS fixes. A point with a non-numeric value (`value_num: None`) or a
/// key this module doesn't recognize is ignored; a report missing either
/// `gps_lat` or `gps_lon` is dropped entirely (nothing to plot).
pub fn gps_fixes_from_history(points: &[TelemetryHistoryPoint]) -> Vec<GpsFix> {
  let mut by_time: BTreeMap<i64, PartialFix> = BTreeMap::new();
  for p in points {
    let Some(v) = p.value_num else { continue };
    let entry = by_time.entry(p.reported_at.unix_timestamp()).or_default();
    match p.key.as_str() {
      KEY_LAT => entry.lat = Some(v),
      KEY_LON => entry.lon = Some(v),
      KEY_ALT_M => entry.alt_m = Some(v),
      KEY_SPEED_MPS => entry.speed_mps = Some(v),
      KEY_HEADING_DEG => entry.heading_deg = Some(v),
      KEY_SATS => entry.sats = Some(v),
      KEY_FIX_QUALITY => entry.fix_quality = Some(v),
      _ => {}
    }
  }
  by_time
    .into_iter()
    .filter_map(|(reported_at, f)| {
      Some(GpsFix {
        reported_at,
        lat: f.lat?,
        lon: f.lon?,
        alt_m: f.alt_m,
        speed_mps: f.speed_mps,
        heading_deg: f.heading_deg,
        sats: f.sats,
        fix_quality: f.fix_quality,
      })
    })
    .collect()
}

/// Gate for whether `TrackWidget` should render at all ("shown only when
/// the pigeon's telemetry history contains BOTH gps_lat and gps_lon").
/// Checked against the pigeon's *latest* telemetry snapshot (already
/// fetched by `PigeonView` for the connection badge) rather than a
/// separate history fetch -- same reuse-existing-signal precedent as
/// `graph_widget::numeric_keys_from_latest`.
pub fn latest_has_gps_fix(latest: &[TelemetryLatest]) -> bool {
  let has =
    |key: &str| latest.iter().any(|l| l.key == key && l.value.trim().parse::<f64>().is_ok());
  has(KEY_LAT) && has(KEY_LON)
}

/// The two keys that are technically numeric (so `graph_widget`'s generic
/// numeric-key filter would otherwise surface them) but are near-useless
/// as a LINE graph -- a wandering absolute coordinate has no meaningful
/// y-axis reading on its own. `TrackWidget` is the correct visualization
/// for these two; the other gps_* keys (altitude/speed/heading/sats/fix
/// quality) are ordinary scalars and stay in the graph key picker.
pub fn is_line_graph_excluded(key: &str) -> bool {
  key == KEY_LAT || key == KEY_LON
}

/// Formats a signed decimal-degree latitude as e.g. "40.7128°N" -- 4
/// decimal places (~11m of precision at the equator) with a hemisphere
/// suffix instead of a leading minus sign, which reads more naturally on
/// a dashboard than a bare signed float.
pub fn format_lat(lat: f64) -> String {
  format!("{:.4}°{}", lat.abs(), if lat >= 0.0 { "N" } else { "S" })
}

pub fn format_lon(lon: f64) -> String {
  format!("{:.4}°{}", lon.abs(), if lon >= 0.0 { "E" } else { "W" })
}

pub fn format_coord(lat: f64, lon: f64) -> String {
  format!("{}, {}", format_lat(lat), format_lon(lon))
}

/// "Current position" summary line, read straight from the pigeon's
/// latest telemetry snapshot (not history) -- same source
/// `latest_has_gps_fix` gates on. `None` when either coordinate is
/// missing or non-numeric in the latest report.
pub fn current_position_line(latest: &[TelemetryLatest]) -> Option<String> {
  let value_of = |key: &str| {
    latest
      .iter()
      .find(|l| l.key == key)
      .and_then(|l| l.value.trim().parse::<f64>().ok())
  };
  let lat = value_of(KEY_LAT)?;
  let lon = value_of(KEY_LON)?;
  let mut line = format_coord(lat, lon);
  let sats = value_of(KEY_SATS);
  let fix_quality = value_of(KEY_FIX_QUALITY);
  match (sats, fix_quality) {
    (Some(sats), Some(fq)) => {
      line.push_str(&format!(" · {} sats · fix quality {}", sats as i64, fq as i64))
    }
    (Some(sats), None) => line.push_str(&format!(" · {} sats", sats as i64)),
    (None, Some(fq)) => line.push_str(&format!(" · fix quality {}", fq as i64)),
    (None, None) => {}
  }
  Some(line)
}

// --- Bounding box / projection math ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
  pub min_lat: f64,
  pub max_lat: f64,
  pub min_lon: f64,
  pub max_lon: f64,
}

impl Bounds {
  pub fn of(fixes: &[GpsFix]) -> Option<Bounds> {
    if fixes.is_empty() {
      return None;
    }
    let (mut min_lat, mut max_lat) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_lon, mut max_lon) = (f64::INFINITY, f64::NEG_INFINITY);
    for f in fixes {
      min_lat = min_lat.min(f.lat);
      max_lat = max_lat.max(f.lat);
      min_lon = min_lon.min(f.lon);
      max_lon = max_lon.max(f.lon);
    }
    Some(Bounds { min_lat, max_lat, min_lon, max_lon })
  }

  pub fn mean_lat(&self) -> f64 {
    (self.min_lat + self.max_lat) / 2.0
  }

  pub fn lat_span(&self) -> f64 {
    self.max_lat - self.min_lat
  }

  pub fn lon_span(&self) -> f64 {
    self.max_lon - self.min_lon
  }
}

/// cos(lat) longitude correction -- 1 degree of longitude covers cos(lat)
/// times less ground distance than 1 degree of latitude away from the
/// equator, so scaling raw degrees 1:1 would stretch a track
/// east-to-west the further it is from the equator. Clamped away from
/// zero so a track near the poles (not a realistic pigeon deployment,
/// but defensive) doesn't blow up the horizontal scale.
const MIN_LON_SCALE: f64 = 0.05;

pub fn lon_scale_for_lat(mean_lat_deg: f64) -> f64 {
  mean_lat_deg.to_radians().cos().abs().max(MIN_LON_SCALE)
}

/// Below this cos-corrected span (in degrees -- roughly 11m at the
/// equator) a track counts as "stationary": `TrackWidget` renders a
/// single centered marker instead of zooming a polyline into meaningless
/// GPS jitter.
const DEGENERATE_SPAN_DEG: f64 = 0.0001;

pub fn is_stationary(bounds: &Bounds, lon_scale: f64) -> bool {
  bounds.lat_span() < DEGENERATE_SPAN_DEG && bounds.lon_span() * lon_scale < DEGENERATE_SPAN_DEG
}

/// Projects lat/lon into a `plot_w`x`plot_h` pixel box (local
/// coordinates -- the caller adds any margin/translation), padded by
/// `pad_frac` on each side and scaled equally on both axes (the larger of
/// the two cos-corrected spans drives the scale) so the track's shape
/// isn't stretched to fill a non-square plot area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackProjector {
  min_lat: f64,
  min_lon: f64,
  lon_scale: f64,
  scale: f64,
  offset_x: f64,
  offset_y: f64,
  plot_h: f64,
}

impl TrackProjector {
  pub fn new(bounds: &Bounds, plot_w: f64, plot_h: f64, pad_frac: f64) -> TrackProjector {
    let lon_scale = lon_scale_for_lat(bounds.mean_lat());
    // Two versions of each span: the REAL one (may be exactly 0 for a
    // single point or a truly stationary track) drives where content is
    // centered, since a real point at `min_lat`/`min_lon` must project to
    // the middle of the plot box when there's no real spread to speak
    // of. The DEGENERATE_SPAN_DEG-floored version only feeds the scale
    // computation, so dividing by a real span of 0 doesn't produce an
    // infinite (or wildly overzoomed) `scale`.
    let real_lat_span = bounds.lat_span().max(0.0);
    let real_lon_span_eff = (bounds.lon_span() * lon_scale).max(0.0);
    let lat_span_for_scale = real_lat_span.max(DEGENERATE_SPAN_DEG);
    let lon_span_eff_for_scale = real_lon_span_eff.max(DEGENERATE_SPAN_DEG);
    let span = lat_span_for_scale.max(lon_span_eff_for_scale);
    let padded_span = span * (1.0 + 2.0 * pad_frac.max(0.0));
    let usable = plot_w.min(plot_h).max(1.0);
    let scale = usable / padded_span;
    let content_w = real_lon_span_eff * scale;
    let content_h = real_lat_span * scale;
    let offset_x = (plot_w - content_w) / 2.0;
    let offset_y = (plot_h - content_h) / 2.0;
    TrackProjector {
      min_lat: bounds.min_lat,
      min_lon: bounds.min_lon,
      lon_scale,
      scale,
      offset_x,
      offset_y,
      plot_h,
    }
  }

  /// Pixel coordinates, y-flipped so north renders "up" (SVG y grows
  /// downward).
  pub fn project(&self, lat: f64, lon: f64) -> (f64, f64) {
    let x = self.offset_x + (lon - self.min_lon) * self.lon_scale * self.scale;
    let y_up = self.offset_y + (lat - self.min_lat) * self.scale;
    (x, self.plot_h - y_up)
  }
}

fn dist2(a: (f64, f64), b: (f64, f64)) -> f64 {
  let dx = a.0 - b.0;
  let dy = a.1 - b.1;
  dx * dx + dy * dy
}

/// Nearest-point lookup for the track's hover readout -- unlike
/// `TelemetryChart` (nearest by *time*, since that's a line-over-time
/// chart), a spatial track's nearest point is nearest in screen space to
/// the mouse.
pub fn nearest_point_index(points: &[(f64, f64)], target: (f64, f64)) -> Option<usize> {
  points
    .iter()
    .enumerate()
    .min_by(|(_, a), (_, b)| dist2(**a, target).total_cmp(&dist2(**b, target)))
    .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
  use super::*;
  use time::OffsetDateTime;

  fn history_point(
    key: &str,
    value: &str,
    value_num: Option<f64>,
    reported_at: OffsetDateTime,
  ) -> TelemetryHistoryPoint {
    TelemetryHistoryPoint {
      pigeon_id: "p1".to_string(),
      key: key.to_string(),
      value: value.to_string(),
      value_num,
      reported_at,
    }
  }

  fn latest(key: &str, value: &str) -> TelemetryLatest {
    TelemetryLatest {
      key: key.to_string(),
      value: value.to_string(),
      reported_at: OffsetDateTime::UNIX_EPOCH,
    }
  }

  fn fix(lat: f64, lon: f64) -> GpsFix {
    GpsFix {
      reported_at: 0,
      lat,
      lon,
      alt_m: None,
      speed_mps: None,
      heading_deg: None,
      sats: None,
      fix_quality: None,
    }
  }

  #[test]
  fn assembles_fixes_grouped_by_shared_timestamp() {
    let t1 = OffsetDateTime::from_unix_timestamp(1000).unwrap();
    let t2 = OffsetDateTime::from_unix_timestamp(2000).unwrap();
    let points = vec![
      history_point("gps_lat", "40.0", Some(40.0), t1),
      history_point("gps_lon", "-74.0", Some(-74.0), t1),
      history_point("gps_sats", "6", Some(6.0), t1),
      history_point("gps_lat", "40.01", Some(40.01), t2),
      history_point("gps_lon", "-74.01", Some(-74.01), t2),
    ];
    let fixes = gps_fixes_from_history(&points);
    assert_eq!(fixes.len(), 2);
    assert_eq!(fixes[0].reported_at, 1000);
    assert_eq!(fixes[0].lat, 40.0);
    assert_eq!(fixes[0].lon, -74.0);
    assert_eq!(fixes[0].sats, Some(6.0));
    assert_eq!(fixes[1].sats, None);
  }

  #[test]
  fn drops_reports_missing_either_coordinate() {
    let t1 = OffsetDateTime::from_unix_timestamp(1000).unwrap();
    let points = vec![
      history_point("gps_lat", "40.0", Some(40.0), t1),
      history_point("battery_mv", "3300", Some(3300.0), t1),
    ];
    assert!(gps_fixes_from_history(&points).is_empty());
  }

  #[test]
  fn ignores_non_numeric_points() {
    let t1 = OffsetDateTime::from_unix_timestamp(1000).unwrap();
    let points = vec![
      history_point("gps_lat", "bad", None, t1),
      history_point("gps_lon", "-74.0", Some(-74.0), t1),
    ];
    assert!(gps_fixes_from_history(&points).is_empty());
  }

  #[test]
  fn latest_has_gps_fix_requires_both_numeric_keys() {
    assert!(latest_has_gps_fix(&[latest("gps_lat", "40.0"), latest("gps_lon", "-74.0")]));
    assert!(!latest_has_gps_fix(&[latest("gps_lat", "40.0")]));
    assert!(!latest_has_gps_fix(&[
      latest("gps_lat", "nope"),
      latest("gps_lon", "-74.0")
    ]));
  }

  #[test]
  fn line_graph_exclusion_targets_only_lat_lon() {
    assert!(is_line_graph_excluded("gps_lat"));
    assert!(is_line_graph_excluded("gps_lon"));
    assert!(!is_line_graph_excluded("gps_speed_mps"));
    assert!(!is_line_graph_excluded("battery_mv"));
  }

  #[test]
  fn formats_coordinates_with_hemisphere_suffixes() {
    assert_eq!(format_coord(40.7128, -74.0060), "40.7128°N, 74.0060°W");
    assert_eq!(format_coord(-33.8688, 151.2093), "33.8688°S, 151.2093°E");
  }

  #[test]
  fn current_position_line_includes_optional_fields_when_present() {
    let latest_full = vec![
      latest("gps_lat", "40.7128"),
      latest("gps_lon", "-74.0060"),
      latest("gps_sats", "8"),
      latest("gps_fix_quality", "4"),
    ];
    assert_eq!(
      current_position_line(&latest_full),
      Some("40.7128°N, 74.0060°W · 8 sats · fix quality 4".to_string())
    );

    let latest_minimal = vec![latest("gps_lat", "40.7128"), latest("gps_lon", "-74.0060")];
    assert_eq!(
      current_position_line(&latest_minimal),
      Some("40.7128°N, 74.0060°W".to_string())
    );

    let latest_missing_lon = vec![latest("gps_lat", "40.7128")];
    assert_eq!(current_position_line(&latest_missing_lon), None);
  }

  #[test]
  fn bounds_of_empty_is_none() {
    assert_eq!(Bounds::of(&[]), None);
  }

  #[test]
  fn bounds_of_computes_min_max() {
    let fixes = vec![fix(40.0, -74.0), fix(40.5, -74.5), fix(39.9, -73.9)];
    let bounds = Bounds::of(&fixes).unwrap();
    assert_eq!(bounds.min_lat, 39.9);
    assert_eq!(bounds.max_lat, 40.5);
    assert_eq!(bounds.min_lon, -74.5);
    assert_eq!(bounds.max_lon, -73.9);
  }

  #[test]
  fn lon_scale_shrinks_away_from_the_equator() {
    let equator = lon_scale_for_lat(0.0);
    let mid = lon_scale_for_lat(45.0);
    let pole = lon_scale_for_lat(89.9999);
    assert!((equator - 1.0).abs() < 1e-9);
    assert!(mid < equator && mid > pole);
    assert!(pole >= MIN_LON_SCALE);
  }

  #[test]
  fn stationary_detection() {
    let tiny = Bounds {
      min_lat: 40.0,
      max_lat: 40.00001,
      min_lon: -74.0,
      max_lon: -74.00001,
    };
    let moved = Bounds {
      min_lat: 40.0,
      max_lat: 40.01,
      min_lon: -74.0,
      max_lon: -74.01,
    };
    assert!(is_stationary(&tiny, lon_scale_for_lat(tiny.mean_lat())));
    assert!(!is_stationary(&moved, lon_scale_for_lat(moved.mean_lat())));
  }

  #[test]
  fn projector_maps_bounds_corners_within_padded_plot() {
    let bounds = Bounds {
      min_lat: 40.0,
      max_lat: 41.0,
      min_lon: -74.0,
      max_lon: -73.0,
    };
    let projector = TrackProjector::new(&bounds, 400.0, 400.0, 0.1);
    let (x_min, y_of_max_lat) = projector.project(bounds.min_lat, bounds.min_lon);
    let (x_max, y_of_min_lat) = projector.project(bounds.max_lat, bounds.max_lon);
    // North (max_lat) must render above south (min_lat) -- smaller y.
    assert!(y_of_min_lat < y_of_max_lat);
    // Both corners must land inside the plot box, not clipped outside it.
    for coord in [x_min, x_max, y_of_max_lat, y_of_min_lat] {
      assert!((0.0..=400.0).contains(&coord));
    }
  }

  #[test]
  fn projector_centers_a_single_point() {
    let bounds = Bounds {
      min_lat: 40.0,
      max_lat: 40.0,
      min_lon: -74.0,
      max_lon: -74.0,
    };
    let projector = TrackProjector::new(&bounds, 400.0, 300.0, 0.1);
    let (x, y) = projector.project(40.0, -74.0);
    assert!((x - 200.0).abs() < 1.0);
    assert!((y - 150.0).abs() < 1.0);
  }

  #[test]
  fn nearest_point_index_picks_closest() {
    let points = vec![(0.0, 0.0), (10.0, 10.0), (100.0, 100.0)];
    assert_eq!(nearest_point_index(&points, (1.0, 1.0)), Some(0));
    assert_eq!(nearest_point_index(&points, (95.0, 95.0)), Some(2));
  }

  #[test]
  fn nearest_point_index_empty_is_none() {
    assert_eq!(nearest_point_index(&[], (0.0, 0.0)), None);
  }
}
