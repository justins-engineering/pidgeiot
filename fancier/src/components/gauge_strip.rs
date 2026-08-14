// Latest value as a proportion of a known range, for readings that have
// one. A percentage is the obvious case: 40.7% means something on its own
// because the scale it belongs to is implied by the unit.
//
// Deliberately NOT a chart kind. The chart component's five kinds all plot
// a series against time; this plots one number against its own bounds and
// has no time axis at all, which is why it lives beside the stat strip
// rather than in `ChartKind`. Adding it there would have meant a kind that
// silently ignores every point but the last.
//
// It also refuses to guess. A reading with no declared range gets no bar,
// because a bar whose full width is invented tells the reader a proportion
// that nobody measured -- the same objection that decides line-vs-area on
// the charts.
use dioxus::prelude::*;

/// A reading that can be drawn as a proportion: its bounds are known, and
/// they mean something.
#[derive(Clone, PartialEq)]
pub struct GaugeReading {
  pub key: String,
  pub label: String,
  /// Shown after the number. Empty for a bare count.
  pub unit: String,
  pub value: f64,
  /// The range the bar spans. Both ends are real values from the domain,
  /// not the min/max of whatever happened to be reported.
  pub min: f64,
  pub max: f64,
}

impl GaugeReading {
  /// Where the value sits in its range, clamped. A reading outside its
  /// declared bounds is a real thing -- a sensor drifting past 100% -- and
  /// pinning the bar is more honest than letting it overflow its track,
  /// since the number beside it still shows the true value.
  fn fraction(&self) -> f64 {
    let span = self.max - self.min;
    if span <= 0.0 {
      return 0.0;
    }
    ((self.value - self.min) / span).clamp(0.0, 1.0)
  }

  fn percent(&self) -> f64 {
    self.fraction() * 100.0
  }
}

#[component]
pub fn GaugeStrip(readings: Vec<GaugeReading>, caption: Option<String>) -> Element {
  if readings.is_empty() {
    return rsx! {};
  }

  rsx! {
    div { class: "rounded-box border border-base-content/10 bg-base-100 p-5 flex flex-col gap-4",
      for reading in readings.iter() {
        div { key: "{reading.key}", class: "flex flex-col gap-1.5",
          div { class: "flex items-baseline justify-between gap-3 text-sm",
            span { class: "font-mono text-base-content/70 truncate", "{reading.label}" }
            span { class: "font-bold shrink-0", "{reading.value}{reading.unit}" }
          }
          // A native <progress> rather than a styled div: it carries the
          // value, min and max to assistive tech without a parallel set of
          // aria attributes that could drift from what is drawn.
          progress {
            class: "progress progress-primary w-full",
            value: "{reading.percent()}",
            max: "100",
            "aria-label": "{reading.label}",
          }
          div { class: "flex justify-between text-[11px] text-base-content/50",
            span { "{reading.min}{reading.unit}" }
            span { "{reading.max}{reading.unit}" }
          }
        }
      }
      if let Some(caption) = caption {
        p { class: "text-xs text-base-content/60 leading-relaxed", "{caption}" }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::GaugeReading;

  fn reading(value: f64, min: f64, max: f64) -> GaugeReading {
    GaugeReading {
      key: "soil_moisture_pct".into(),
      label: "Soil Moisture".into(),
      unit: "%".into(),
      value,
      min,
      max,
    }
  }

  #[test]
  fn fraction_spans_the_declared_range() {
    assert_eq!(reading(0.0, 0.0, 100.0).fraction(), 0.0);
    assert_eq!(reading(50.0, 0.0, 100.0).fraction(), 0.5);
    assert_eq!(reading(100.0, 0.0, 100.0).fraction(), 1.0);
  }

  /// A range need not start at zero -- a battery reading between 3.0V and
  /// 4.2V is a proportion of that span, not of the voltage axis.
  #[test]
  fn a_range_that_does_not_start_at_zero_still_spans_correctly() {
    assert_eq!(reading(3.6, 3.0, 4.2).fraction(), 0.5);
  }

  /// A sensor reading past its declared bound pins the bar rather than
  /// overflowing it. The number beside the bar still shows the truth.
  #[test]
  fn out_of_range_readings_pin_rather_than_overflow() {
    assert_eq!(reading(140.0, 0.0, 100.0).fraction(), 1.0);
    assert_eq!(reading(-20.0, 0.0, 100.0).fraction(), 0.0);
  }

  /// A degenerate range would divide by zero. Draw nothing rather than
  /// something arbitrary.
  #[test]
  fn a_zero_width_range_draws_empty() {
    assert_eq!(reading(50.0, 10.0, 10.0).fraction(), 0.0);
    assert_eq!(reading(50.0, 90.0, 10.0).fraction(), 0.0);
  }
}
