// The competitor comparison, shared by the sales page and the full
// comparison page. Both render the same rows from the same file with the
// same provenance; they differ only in how much of it they show, which is
// `pricing_data::View`. Keeping one implementation is the point: two
// copies would be free to disagree about a figure, and the whole reason
// these numbers live in a data file is that they must not.
use crate::helpers::pricing_data::{self, Column, Profile, Provenance, Row, View};
use dioxus::prelude::*;
use time::Date;

/// Where one figure came from and when we last looked. Deliberately quiet:
/// a small line under the platform name, not a column of its own, because
/// a reader comparing prices should be able to ignore it right up until it
/// matters. It stops being quiet only when the figure has gone unchecked
/// past the data file's own threshold, which is the one case where the
/// number on screen might not be the number the vendor charges.
#[component]
fn FigureSource(row: Row, stale: bool) -> Element {
  rsx! {
    div { class: "mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs font-normal text-base-content/60",
      if let (Some(href), Some(host)) = (row.source.clone(), row.source_host()) {
        a {
          class: "link link-hover",
          href,
          target: "_blank",
          rel: "nofollow noopener",
          "{host}"
        }
      } else {
        span { "our own published price" }
      }
      span { class: if stale { "text-warning font-medium" }, "checked {row.verified_label()}" }
      if let Some(qualifier) = row.provenance().qualifier() {
        span { "· {qualifier}" }
      }
      if stale {
        span {
          class: "badge badge-warning badge-xs",
          title: "This figure has gone long enough unchecked that the vendor may have repriced since.",
          "recheck"
        }
      }
    }
  }
}

/// One vendor: what they charge, and in one line, the thing that actually
/// decides it. The difference sits with the platform name rather than in a
/// list below the table, because a reader comparing prices will not scroll
/// to find out what a price buys.
#[component]
fn ComparisonRow(
  row: Row,
  columns: Vec<Column>,
  view: View,
  today: Date,
  stale_after: i64,
) -> Element {
  let stale = row.is_stale(today, stale_after);

  rsx! {
    tr { class: if row.provenance() == Provenance::Ours { "font-bold" },
      td {
        div {
          "{row.platform}"
          if let Some(plan) = row.plan.as_ref() {
            span { class: "font-normal text-base-content/70", " · {plan}" }
          }
        }
        if let Some(note) = row.note_for(view) {
          div { class: "mt-0.5 text-sm font-normal text-base-content/70", "{note}" }
        }
        FigureSource { row: row.clone(), stale }
      }
      for column in columns.iter() {
        td { key: "{column.key}", class: "text-right whitespace-nowrap align-top",
          if let Some(figure) = row.figure(column) {
            div { "{figure.value}" }
            if let Some(qualifier) = figure.qualifier.as_ref() {
              div { class: "text-xs font-normal text-base-content/60", "{qualifier}" }
            }
          } else {
            span { class: "font-normal italic text-base-content/50",
              "{pricing_data::NOT_PUBLISHED}"
            }
          }
        }
      }
    }
  }
}

/// One cadence: one table of things you can buy, then the separate
/// question of assembling it yourself.
#[component]
pub fn ProfileComparison(profile: Profile, view: View, today: Date, stale_after: i64) -> Element {
  let columns = profile.columns(view);
  let rows = profile.rows(view);

  rsx! {
    div { class: "mt-16 first:mt-0",
      h3 { class: "text-2xl md:text-3xl font-extrabold tracking-tight",
        "{profile.heading(view)}"
      }
      p { class: "mt-3 text-base-content/70 leading-relaxed", "{profile.subhead(view)}" }

      div { class: "mt-6 overflow-x-auto rounded-2xl border border-base-300 bg-base-100",
        table { class: "table",
          thead {
            tr {
              th { "Platform" }
              for column in columns.iter() {
                th { key: "{column.key}", class: "text-right",
                  div { "{column.label}" }
                  div { class: "font-normal text-base-content/60", "{column.unit}" }
                }
              }
            }
          }
          tbody {
            for row in rows.iter() {
              ComparisonRow {
                key: "{row.id}",
                row: row.clone(),
                columns: columns.clone(),
                view,
                today,
                stale_after,
              }
            }
          }
        }
      }

      if let Some(closing) = profile.closing.as_ref() {
        p { class: "mt-5 text-base-content/80 leading-relaxed font-medium", "{closing}" }
      }

      if view == View::Full {
        // Raw infrastructure, deliberately not a row in the table above. A
        // rate in cents sitting in a price column anchors a reader before any
        // feature list reaches them, so they meet the reason first and the
        // rates second, as a footnote rather than a ranking.
        div { class: "mt-6 rounded-2xl border border-base-300 bg-base-100 p-5",
          p { class: "text-sm text-base-content/75 leading-relaxed",
            "{profile.build_your_own_intro}"
          }
          p { class: "mt-3 text-xs text-base-content/50",
            "Per device per month at {reference_label(&profile)}:"
          }
          ul { class: "mt-1.5 flex flex-col gap-1.5 text-xs text-base-content/60",
            for row in profile.build_your_own.iter() {
              li { key: "{row.id}", class: "flex flex-wrap items-baseline gap-x-2",
                span { class: "font-bold text-base-content/80",
                  "{row.platform}"
                  if let Some(plan) = row.plan.as_ref() {
                    span { class: "font-normal", ", {plan}" }
                  }
                }
                span { class: "font-bold text-base-content/80",
                  "{rate_at_reference(&row, &profile)}"
                }
                if let Some(note) = row.note.as_ref() {
                  span { "{note}" }
                }
                if let Some(host) = row.source_host() {
                  span { class: if row.is_stale(today, stale_after) { "text-warning font-medium" },
                    "{host}, checked {row.verified_label()}"
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}

/// The rate to print for a build-it-yourself line: the one at the
/// profile's reference scale, named beside the list so it is never a bare
/// number. Quoting each row's own cheapest instead would have printed
/// Azure's ten-thousand-device rate next to AWS's thousand-device one,
/// with nothing on the page saying they were measured differently.
fn rate_at_reference(row: &Row, profile: &Profile) -> String {
  profile
    .columns
    .iter()
    .find(|column| column.key == profile.reference_column)
    .and_then(|column| row.figure(column))
    .map(|figure| figure.value.clone())
    .unwrap_or_else(|| pricing_data::NOT_PUBLISHED.to_string())
}

/// The reference column's own label, for the line that says what scale
/// those rates are quoted at.
fn reference_label(profile: &Profile) -> String {
  profile
    .columns
    .iter()
    .find(|column| column.key == profile.reference_column)
    .map(|column| column.label.clone())
    .unwrap_or_default()
}

/// Every table on a page, for one view. The data hook lives here rather
/// than in either page so both fetch identically: same file, same
/// post-hydration write, same fallback to the baked copy.
#[component]
pub fn ComparisonTables(view: View) -> Element {
  let mut comparison = use_signal(pricing_data::baked);

  // Same shape as views::demo's poll, and never runs during the
  // synchronous SSG pass for the same reason -- so the prerendered tables
  // are the baked copy, and this is only the hydrated page catching up
  // with whatever the deployed file says now.
  //
  // Sets the signal even when the fetch fails, because `today()` below is
  // evaluated at render time and the prerendered HTML evaluated it at
  // build time. A prerendered page hydrates by adopting its own markup, so
  // without a post-hydration write nothing would re-render and the table
  // would keep the build's verdict on its own staleness.
  use_future(move || async move {
    let published = pricing_data::fetch_published().await;
    comparison.set(published.unwrap_or_else(pricing_data::baked));
  });

  let data = comparison();
  let today = pricing_data::today();
  let stale_after = data.stale_after_days;

  rsx! {
    for profile in data.profiles.iter() {
      ProfileComparison {
        key: "{profile.id}",
        profile: profile.clone(),
        view,
        today,
        stale_after,
      }
    }
  }
}
