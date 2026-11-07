//! Pure edits to a custom dashboard's layout, kept apart from the view so the order, the ids and
//! the grid spans are tested without a browser.

use crate::components::{ChartKind, GraphDef, TimeRange};
use capsules::{
  DashboardLayout, Widget, WidgetBody, WidgetChart, WidgetRange, WidgetScope, WidgetSize,
};

/// Moves the widget at `index` one place up or down. `false` when it is already at that end.
pub fn move_widget(layout: &mut DashboardLayout, index: usize, up: bool) -> bool {
  let len = layout.widgets.len();
  if index >= len {
    return false;
  }
  let target = if up {
    match index.checked_sub(1) {
      Some(target) => target,
      None => return false,
    }
  } else if index + 1 < len {
    index + 1
  } else {
    return false;
  };
  layout.widgets.swap(index, target);
  true
}

/// Drops the widget with `id`, if present.
pub fn remove_widget(layout: &mut DashboardLayout, id: &str) {
  layout.widgets.retain(|w| w.id != id);
}

/// Replaces the widget with the same id, or appends it as a new one.
pub fn put_widget(layout: &mut DashboardLayout, widget: Widget) {
  match layout.widgets.iter_mut().find(|w| w.id == widget.id) {
    Some(slot) => *slot = widget,
    None => layout.widgets.push(widget),
  }
}

/// The lowest `w<n>` no widget in the layout holds yet.
pub fn fresh_widget_id(layout: &DashboardLayout) -> String {
  let mut n = 1usize;
  loop {
    let mut id = String::with_capacity(4);
    id.push('w');
    id.push_str(&n.to_string());
    if !layout.widgets.iter().any(|w| w.id == id) {
      return id;
    }
    n += 1;
  }
}

/// Grid classes for a widget's span: one column on a phone, two on a tablet, four on a desktop.
/// Literal strings so Tailwind's scanner sees every class.
pub fn size_class(size: WidgetSize) -> &'static str {
  match size {
    WidgetSize::Small => "col-span-1",
    WidgetSize::Half => "col-span-1 md:col-span-2",
    WidgetSize::Full => "col-span-1 md:col-span-2 xl:col-span-4",
  }
}

/// The kinds the editor offers, in menu order, with the size a new one starts at.
pub const KINDS: [(&str, &str, WidgetSize); 7] = [
  ("time_series", "Time series", WidgetSize::Full),
  ("latest", "Latest value", WidgetSize::Small),
  ("threshold", "Threshold tile", WidgetSize::Small),
  ("status_table", "Flock status table", WidgetSize::Full),
  ("shadow_key", "Config key", WidgetSize::Small),
  ("alerts", "Alerts", WidgetSize::Half),
  ("text", "Text note", WidgetSize::Half),
];

/// The `kind` tag of a widget body, as the layout spells it.
pub fn kind_of(body: &WidgetBody) -> &'static str {
  match body {
    WidgetBody::TimeSeries { .. } => "time_series",
    WidgetBody::Latest { .. } => "latest",
    WidgetBody::Threshold { .. } => "threshold",
    WidgetBody::StatusTable { .. } => "status_table",
    WidgetBody::ShadowKey { .. } => "shadow_key",
    WidgetBody::Alerts { .. } => "alerts",
    WidgetBody::Text { .. } => "text",
    WidgetBody::Unknown => "unknown",
  }
}

/// What a widget is called when it has no title of its own.
pub fn default_title(body: &WidgetBody) -> String {
  match body {
    WidgetBody::TimeSeries { keys, .. } => keys.join(", "),
    WidgetBody::Latest { key, .. } | WidgetBody::Threshold { key, .. } => key.clone(),
    WidgetBody::ShadowKey { key, .. } => key.clone(),
    WidgetBody::StatusTable { .. } => "Flock status".to_string(),
    WidgetBody::Alerts { .. } => "Alerts".to_string(),
    WidgetBody::Text { .. } => "Note".to_string(),
    WidgetBody::Unknown => "Unknown widget".to_string(),
  }
}

/// A widget's chart as the chart component draws it.
pub fn chart_kind(chart: WidgetChart) -> ChartKind {
  match chart {
    WidgetChart::Line => ChartKind::Line,
    WidgetChart::Step => ChartKind::Step,
    WidgetChart::Area => ChartKind::Area,
    WidgetChart::Bar => ChartKind::Bar,
    WidgetChart::Scatter => ChartKind::Scatter,
  }
}

/// The inverse of [`chart_kind`], for a saved graph becoming a widget.
pub fn widget_chart(kind: ChartKind) -> WidgetChart {
  match kind {
    ChartKind::Line => WidgetChart::Line,
    ChartKind::Step => WidgetChart::Step,
    ChartKind::Area => WidgetChart::Area,
    ChartKind::Bar => WidgetChart::Bar,
    ChartKind::Scatter => WidgetChart::Scatter,
  }
}

/// A widget's range as the range picker and the history reads spell it.
pub fn time_range(range: WidgetRange) -> TimeRange {
  match range {
    WidgetRange::Last1h => TimeRange::Last1h,
    WidgetRange::Last6h => TimeRange::Last6h,
    WidgetRange::Last24h => TimeRange::Last24h,
    WidgetRange::Last7d => TimeRange::Last7d,
    WidgetRange::Last30d => TimeRange::Last30d,
  }
}

/// The inverse of [`time_range`].
pub fn widget_range(range: TimeRange) -> WidgetRange {
  match range {
    TimeRange::Last1h => WidgetRange::Last1h,
    TimeRange::Last6h => WidgetRange::Last6h,
    TimeRange::Last24h => WidgetRange::Last24h,
    TimeRange::Last7d => WidgetRange::Last7d,
    TimeRange::Last30d => WidgetRange::Last30d,
  }
}

/// A saved graph as a time series widget: the same keys, range and chart, full width.
pub fn widget_from_graph(def: &GraphDef, scope: WidgetScope, id: String) -> Widget {
  Widget {
    id,
    size: WidgetSize::Full,
    title: def.title.clone(),
    body: WidgetBody::TimeSeries {
      scope,
      keys: def.keys.clone(),
      range: widget_range(def.range),
      chart: widget_chart(def.kind),
    },
  }
}

/// A finite reading's place on `[min, max]` as a whole percentage, clamped.
pub fn percent_of_range(value: f64, min: f64, max: f64) -> u8 {
  if !(value.is_finite() && min.is_finite() && max.is_finite() && min < max) {
    return 0;
  }
  (((value - min) / (max - min)).clamp(0.0, 1.0) * 100.0).round() as u8
}

#[cfg(test)]
mod tests {
  use super::*;

  fn text(id: &str) -> Widget {
    Widget {
      id: id.to_string(),
      size: WidgetSize::Small,
      title: String::new(),
      body: WidgetBody::Text {
        text: id.to_string(),
      },
    }
  }

  fn ids(layout: &DashboardLayout) -> Vec<&str> {
    layout.widgets.iter().map(|w| w.id.as_str()).collect()
  }

  fn layout_of(names: &[&str]) -> DashboardLayout {
    DashboardLayout {
      widgets: names.iter().map(|n| text(n)).collect(),
      ..DashboardLayout::default()
    }
  }

  #[test]
  fn up_and_down_swap_neighbours_and_stop_at_the_ends() {
    let mut layout = layout_of(&["a", "b", "c"]);
    assert!(move_widget(&mut layout, 2, true));
    assert_eq!(ids(&layout), ["a", "c", "b"]);
    assert!(move_widget(&mut layout, 0, false));
    assert_eq!(ids(&layout), ["c", "a", "b"]);
    assert!(!move_widget(&mut layout, 0, true));
    assert!(!move_widget(&mut layout, 2, false));
    assert!(!move_widget(&mut layout, 9, true));
    assert_eq!(ids(&layout), ["c", "a", "b"]);
  }

  #[test]
  fn a_put_replaces_in_place_or_appends() {
    let mut layout = layout_of(&["w1", "w2"]);
    let mut edited = text("w1");
    edited.size = WidgetSize::Full;
    put_widget(&mut layout, edited);
    assert_eq!(ids(&layout), ["w1", "w2"]);
    assert_eq!(layout.widgets[0].size, WidgetSize::Full);
    put_widget(&mut layout, text("w3"));
    assert_eq!(ids(&layout), ["w1", "w2", "w3"]);
    remove_widget(&mut layout, "w2");
    assert_eq!(ids(&layout), ["w1", "w3"]);
  }

  #[test]
  fn fresh_ids_fill_the_lowest_gap() {
    assert_eq!(fresh_widget_id(&layout_of(&[])), "w1");
    assert_eq!(fresh_widget_id(&layout_of(&["w1", "w3"])), "w2");
    assert_eq!(fresh_widget_id(&layout_of(&["w1", "w2"])), "w3");
  }

  #[test]
  fn every_kind_the_editor_offers_round_trips_its_tag() {
    for (tag, _, _) in KINDS {
      let json = format!(
        r#"{{"id":"w","kind":"{tag}","pigeon":"p","key":"k","flock":"00000000-0000-0000-0000-000000000000","scope":{{"pigeon":"p"}},"min":0,"max":1}}"#
      );
      let widget: Widget = serde_json::from_str(&json).unwrap();
      assert_eq!(kind_of(&widget.body), tag);
    }
  }

  #[test]
  fn a_saved_graph_becomes_the_same_time_series() {
    let def = GraphDef {
      id: "g".into(),
      title: "Battery".into(),
      keys: vec!["battery_mv".into()],
      range: TimeRange::Last7d,
      kind: ChartKind::Step,
    };
    let widget = widget_from_graph(&def, WidgetScope::Pigeon("p".into()), "w4".into());
    assert_eq!(widget.title, "Battery");
    assert_eq!(
      widget.body,
      WidgetBody::TimeSeries {
        scope: WidgetScope::Pigeon("p".into()),
        keys: vec!["battery_mv".into()],
        range: WidgetRange::Last7d,
        chart: WidgetChart::Step,
      }
    );
    assert_eq!(time_range(WidgetRange::Last7d), TimeRange::Last7d);
    assert_eq!(chart_kind(WidgetChart::Step), ChartKind::Step);
  }

  #[test]
  fn readings_place_on_their_range_clamped() {
    assert_eq!(percent_of_range(50.0, 0.0, 100.0), 50);
    assert_eq!(percent_of_range(-5.0, 0.0, 100.0), 0);
    assert_eq!(percent_of_range(150.0, 0.0, 100.0), 100);
    assert_eq!(percent_of_range(f64::NAN, 0.0, 100.0), 0);
    assert_eq!(percent_of_range(3.0, 5.0, 5.0), 0);
  }
}
