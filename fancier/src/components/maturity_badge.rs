use dioxus::prelude::*;

/// Feature-maturity labeling for marketing/docs content (features page,
/// landing-page partials) -- a different axis than `ConnectionState`/
/// `ConnectionBadge` (which is per-device runtime status). A shipped,
/// production-ready feature renders no badge at all -- only the two
/// non-default states below get a caller-visible affordance, so a
/// missing badge always means "this just works today."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maturity {
  /// Real, working code -- hardware- or staging-verified per CLAUDE.md
  /// -- but not yet promoted everywhere (e.g. not on production yet).
  Beta,
  /// Design-stage only; nothing runs yet. Never attach this to a
  /// feature that's actually reachable today.
  Planned,
}

impl Maturity {
  /// DaisyUI badge color. `Planned` deliberately avoids `badge-neutral`
  /// for the same reason `ConnectionState::Unknown` does (see
  /// `helpers::connection_state`): this theme's `--color-neutral` is
  /// literal white-on-white in light mode and black-on-black in dark
  /// mode. `badge-ghost` derives from `--color-base-content` instead,
  /// so it's always legible; the explicit border keeps it visible
  /// against `badge-ghost`'s otherwise-transparent fill.
  fn badge_class(self) -> &'static str {
    match self {
      Maturity::Beta => "badge-warning badge-outline",
      Maturity::Planned => "badge-ghost border-base-content/30",
    }
  }

  fn label(self) -> &'static str {
    match self {
      Maturity::Beta => "Beta",
      Maturity::Planned => "Planned",
    }
  }
}

#[component]
pub fn MaturityBadge(maturity: Maturity) -> Element {
  rsx! {
    span { class: "badge badge-sm {maturity.badge_class()}", "{maturity.label()}" }
  }
}
