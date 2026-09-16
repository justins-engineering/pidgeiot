//! Hover geometry for the hand-rolled SVG charts (`components::telemetry_chart`,
//! `components::track_widget`): where a mouse or a finger landed, in the
//! chart's own user units, and where the tooltip that reports it goes.

use dioxus::prelude::*;

/// One axis of a pointer's position, from client (CSS pixel) space into a
/// plot's own user units.
///
/// `rect_start`/`rect_len` are the SVG root's bounding box on this axis and
/// `canvas_len` its viewBox length, so their ratio is whatever scale the
/// browser rendered it at; `margin` is the plot's offset inside the viewBox.
/// A box with no length (a chart that has not been laid out) has no position
/// to report and maps to the plot origin.
pub fn plot_axis(client: f64, rect_start: f64, rect_len: f64, canvas_len: f64, margin: f64) -> f64 {
  if rect_len <= 0.0 {
    return 0.0;
  }
  (client - rect_start) * canvas_len / rect_len - margin
}

/// Where a pointer event landed inside a chart's plot area, in user units.
/// `None` when there is no SVG to measure against.
pub fn pointer_plot_point(
  evt: &Event<PointerData>,
  canvas: (f64, f64),
  margin: (f64, f64),
) -> Option<(f64, f64)> {
  let (left, top, width, height) = svg_client_box(evt)?;
  let client = evt.data().client_coordinates();
  Some((
    plot_axis(client.x, left, width, canvas.0, margin.0),
    plot_axis(client.y, top, height, canvas.1, margin.1),
  ))
}

/// Where a tooltip sits beside a crosshair, as an inline style. A percentage
/// of the chart's own width, so it tracks at whatever scale the SVG rendered;
/// `tooltip_w` is what the tooltip is allowed to be, in user units, and near
/// the right edge it flips to the crosshair's left rather than detaching from
/// it to stay inside the box.
pub fn tooltip_style(x: f64, canvas_w: f64, tooltip_w: f64) -> String {
  const GAP: f64 = 12.0;

  let flip = x + GAP + tooltip_w > canvas_w;
  let left = if flip { x - GAP } else { x + GAP };
  let percent = ((left / canvas_w * 10000.0).round() / 100.0).clamp(0.0, 100.0);
  let mut style = String::with_capacity(48);
  style.push_str("left: ");
  style.push_str(&percent.to_string());
  style.push_str("%;");
  if flip {
    style.push_str(" transform: translateX(-100%);");
  }
  style
}

/// The SVG root's bounding box in client pixels, as `(left, top, width, height)`.
#[cfg(feature = "web")]
fn svg_client_box(evt: &Event<PointerData>) -> Option<(f64, f64, f64, f64)> {
  use dioxus::web::WebEventExt;
  use wasm_bindgen::JsCast;

  // `target`, not `currentTarget`: a touch is captured by the element it
  // started on, and dispatch is over by the time this reads the event.
  let target = evt.data().try_as_web_event()?.target()?;
  let svg = target
    .dyn_ref::<web_sys::Element>()?
    .closest("svg")
    .ok()??;
  let area = svg.get_bounding_client_rect();
  Some((area.left(), area.top(), area.width(), area.height()))
}

/// The server build prerenders without a DOM, so there is nothing to measure.
#[cfg(not(feature = "web"))]
fn svg_client_box(_evt: &Event<PointerData>) -> Option<(f64, f64, f64, f64)> {
  None
}

#[cfg(test)]
mod tests {
  use super::{plot_axis, tooltip_style};

  /// The scale the chart happens to render at cannot change which sample a
  /// pointer is over, which is the whole point of going through the box.
  #[test]
  fn the_same_pointer_maps_to_the_same_unit_at_every_scale() {
    // A pointer a quarter of the way across a 640-unit canvas, with the
    // chart drawn at 1x, 2x and 0.5x and its box starting at client x 100.
    let unscaled = plot_axis(260.0, 100.0, 640.0, 640.0, 48.0);
    let doubled = plot_axis(420.0, 100.0, 1280.0, 640.0, 48.0);
    let halved = plot_axis(180.0, 100.0, 320.0, 640.0, 48.0);
    assert_eq!(unscaled, 112.0);
    assert_eq!(doubled, 112.0);
    assert_eq!(halved, 112.0);
  }

  /// The box moves with the scroll container, so a scrolled chart needs no
  /// separate scroll term.
  #[test]
  fn a_box_scrolled_off_the_left_still_maps_from_its_own_edge() {
    assert_eq!(plot_axis(12.0, -100.0, 640.0, 640.0, 48.0), 64.0);
  }

  #[test]
  fn an_unlaid_out_chart_maps_to_its_origin() {
    assert_eq!(plot_axis(260.0, 100.0, 0.0, 640.0, 48.0), 0.0);
  }

  #[test]
  fn a_tooltip_sits_beside_the_crosshair_in_percent_of_the_chart() {
    assert_eq!(tooltip_style(100.0, 640.0, 160.0), "left: 17.5%;");
  }

  /// The old clamp pinned the tooltip and let the crosshair walk away from
  /// it; flipping keeps the two together at both ends.
  #[test]
  fn a_tooltip_near_the_right_edge_flips_instead_of_detaching() {
    assert_eq!(
      tooltip_style(600.0, 640.0, 160.0),
      "left: 91.88%; transform: translateX(-100%);"
    );
  }
}
