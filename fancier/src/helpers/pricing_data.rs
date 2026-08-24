// The competitor comparison on /pricing/, read from a data file rather than
// written into rsx. Competitor prices perish -- a vendor reprices and our
// table is quietly wrong, with nothing on the page admitting how old the
// number is -- so the figures live in `public/data/pricing-comparison.json`
// with a source URL and a checked-on date each, and the page renders
// whatever that file says.
//
// The same file is read twice, deliberately:
//
//   * `include_str!` bakes a copy into the wasm, so the prerendered HTML
//     carries the full table before any JavaScript runs (a crawler, a
//     markdown-preferring agent, or a reader on a failed wasm load all see
//     real numbers, not an empty shell).
//   * `fetch_published` re-reads the deployed file on mount, so correcting
//     a figure is an asset deploy, not a Rust rebuild. Between the two, a
//     stale baked copy is only ever visible for the moment before the
//     fetch lands, and only to a visitor whose JavaScript runs.
//
// It is the same path in both directions -- `public/` is Dioxus's verbatim
// passthrough directory (Dioxus.toml `asset_dir`), so the file ships at the
// stable, unhashed URL this module fetches. Anything routed through
// `asset!()` would be content-hashed instead, which changes the URL on
// every edit and would defeat the whole point.
//
// Deliberately NOT routed through `api::helpers`: that dispatcher prefixes
// dovecote's host, sends credentials, and treats a 401 as "this tab's
// session is gone" (see `session_lost`). This is a same-origin static file
// on a signed-out marketing page and has no business touching any of that.
use dioxus::logger::tracing::error;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use time::{Date, OffsetDateTime};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Where the deployed copy lives, for the runtime re-fetch. Root-relative:
/// the file is served from the site's own origin, never dovecote's.
pub const ASSET_PATH: &str = "/data/pricing-comparison.json";

const BAKED: &str = include_str!("../../public/data/pricing-comparison.json");

static BAKED_COMPARISON: LazyLock<Comparison> = LazyLock::new(|| {
  serde_json::from_str(BAKED).expect("pricing-comparison.json is not a valid comparison")
});

/// The comparison as it was when this binary was built. The starting value
/// for the page's signal, so the prerender and the first hydrated render
/// agree, and the fallback whenever the deployed file cannot be read or
/// parsed -- a typo in a hand-edited figure leaves the last known-good
/// table standing rather than blanking it.
pub fn baked() -> Comparison {
  BAKED_COMPARISON.clone()
}

/// Fields the page does not render (`_how_to_update`, `schema`) are simply
/// not asked for: serde ignores what it is not told about, so the file can
/// carry notes for whoever edits it without compiling them into the bundle.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Comparison {
  /// Past this many days since `last_verified`, a figure is shown with a
  /// recheck cue. In the data file rather than a constant here so the
  /// threshold is tunable on the same asset-deploy path as the figures.
  pub stale_after_days: i64,
  pub scenario: Scenario,
  /// Both the column headers and the order cells are rendered in. A row's
  /// `figures` is keyed on `Column::key`.
  pub columns: Vec<Column>,
  pub rows: Vec<Row>,
  /// Raw infrastructure, kept out of `rows` on purpose. It is not a
  /// product you can buy, and a bare total sitting in a price table
  /// invites exactly the anchoring the surrounding paragraph exists to
  /// answer, so it is cited in that prose instead of ranked against tiers.
  pub build_your_own: Row,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Scenario {
  pub heading: String,
  pub subhead: String,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Column {
  pub key: String,
  pub label: String,
  pub unit: String,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Row {
  pub id: String,
  pub platform: String,
  pub plan: String,
  pub figures: HashMap<String, Figure>,
  status: String,
  last_verified: String,
  #[serde(default)]
  pub source: Option<String>,
  #[serde(default)]
  pub note: Option<String>,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Figure {
  pub value: String,
  /// Machine-readable, and checked against the column's own unit by the
  /// tests below -- a figure that says what it measures cannot be moved to
  /// another column, or have its column relabelled underneath it, without
  /// something failing.
  pub unit: String,
}

/// How much weight a figure carries. Kept as a string in the file and
/// resolved here rather than deserialized into an enum: an unrecognized
/// token degrades to `Unknown` and shows no claim, where a strict enum
/// would reject the whole file and take every other figure down with it.
/// The tests below are what keep a typo from reaching that fallback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provenance {
  /// Our own published price. Not perishable in the way the others are --
  /// it is set on this very page, a few sections up.
  Ours,
  /// Re-fetched from the vendor's own page on `last_verified`.
  Verified,
  /// Read once and not re-confirmed since.
  SingleFetch,
  /// Checked, and the vendor publishes nothing that prices this fleet.
  Unpriced,
  Unknown,
}

impl Provenance {
  fn from_token(token: &str) -> Self {
    match token {
      "ours" => Self::Ours,
      "verified" => Self::Verified,
      "single-fetch" => Self::SingleFetch,
      "unpriced" => Self::Unpriced,
      _ => Self::Unknown,
    }
  }

  /// The qualifier shown beside the date. `Verified` gets none: a
  /// re-confirmed figure with its date already says everything, and a badge
  /// on the majority case would make the page look busier than the
  /// information warrants.
  pub fn qualifier(&self) -> Option<&'static str> {
    match self {
      Self::Ours | Self::Verified => None,
      Self::SingleFetch => Some("one check"),
      Self::Unpriced => Some("nothing published"),
      Self::Unknown => Some("unconfirmed"),
    }
  }
}

/// Rendered in a cell with no figure. A vendor whose pricing cannot be
/// determined says so; it never gets an estimate standing in for a number
/// they do not publish.
pub const NOT_PUBLISHED: &str = "not published";

impl Row {
  pub fn provenance(&self) -> Provenance {
    Provenance::from_token(&self.status)
  }

  pub fn figure(&self, column: &Column) -> Option<&Figure> {
    self.figures.get(&column.key)
  }

  /// The source's host, so a citation reads as a place a reader already
  /// recognizes rather than a full url wrapping across a table cell.
  pub fn source_host(&self) -> Option<&str> {
    self.source.as_deref().map(|url| {
      url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(url)
    })
  }

  /// `None` for a date the file states in some shape this cannot read. The
  /// callers below fall back to showing the raw text, so a mistyped date
  /// costs its own row's staleness cue and nothing else.
  fn verified_on(&self) -> Option<Date> {
    Date::parse(
      &self.last_verified,
      time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
  }

  /// "12 Aug 2026", matching how the page wrote this date back when there
  /// was one date for the whole table.
  pub fn verified_label(&self) -> String {
    self
      .verified_on()
      .and_then(|d| {
        d.format(time::macros::format_description!(
          "[day padding:none] [month repr:short] [year]"
        ))
        .ok()
      })
      .unwrap_or_else(|| self.last_verified.clone())
  }

  /// Whether this figure has gone long enough unchecked to say so out
  /// loud. Our own price never qualifies -- it is published on this page,
  /// so it cannot silently diverge from a source we do not control.
  pub fn is_stale(&self, today: Date, stale_after_days: i64) -> bool {
    if self.provenance() == Provenance::Ours {
      return false;
    }
    self
      .verified_on()
      .is_some_and(|d| (today - d).whole_days() > stale_after_days)
  }
}

/// Today, for staleness. During the prerender this is the build date, which
/// is the honest answer for a page that has not been hydrated yet; the
/// client recomputes it on mount.
pub fn today() -> Date {
  OffsetDateTime::now_utc().date()
}

/// The deployed data file, or `None` on any failure -- an offline visitor,
/// a deploy mid-flight, a hand-edit that broke the JSON. Every one of those
/// leaves the baked copy on screen, which is a real table with real dates,
/// so there is no error state worth showing a reader over it.
pub async fn fetch_published() -> Option<Comparison> {
  let window = web_sys::window()?;
  let response = JsFuture::from(window.fetch_with_str(ASSET_PATH))
    .await
    .ok()?
    .dyn_into::<web_sys::Response>()
    .ok()?;

  if !response.ok() {
    error!(
      "{ASSET_PATH} fetch failed with status: {}",
      response.status()
    );
    return None;
  }

  let body = JsFuture::from(response.text().ok()?)
    .await
    .ok()?
    .as_string()?;

  match serde_json::from_str(&body) {
    Ok(comparison) => Some(comparison),
    Err(e) => {
      error!("{ASSET_PATH} did not parse as a comparison: {e}");
      None
    }
  }
}

// The file is hand-edited by whoever last checked a vendor's page, on a
// path that deliberately skips a Rust rebuild -- so these are the only
// thing standing between a slip in it and a wrong number on a public page.
// They run against the baked copy, which is the same bytes the asset deploy
// ships.
#[cfg(test)]
mod the_data_file_says_what_it_claims {
  use super::{BAKED_COMPARISON, Provenance, Row};

  fn every_row() -> impl Iterator<Item = &'static Row> {
    BAKED_COMPARISON
      .rows
      .iter()
      .chain(std::iter::once(&BAKED_COMPARISON.build_your_own))
  }

  #[test]
  fn every_status_is_one_this_code_recognizes() {
    for row in every_row() {
      assert_ne!(
        row.provenance(),
        Provenance::Unknown,
        "{} has a status this code does not know, so it would render unconfirmed",
        row.id
      );
    }
  }

  #[test]
  fn every_date_parses() {
    for row in every_row() {
      assert!(
        row.verified_on().is_some(),
        "{}'s last_verified is not a date, so its row can never go stale",
        row.id
      );
    }
  }

  #[test]
  fn every_figure_is_measured_in_its_column_s_unit() {
    for row in every_row() {
      for column in &BAKED_COMPARISON.columns {
        if let Some(figure) = row.figure(column) {
          assert_eq!(
            figure.unit, column.unit,
            "{} states {} in {}, but that column is headed {}",
            row.id, column.key, figure.unit, column.unit
          );
        }
      }
    }
  }

  #[test]
  fn no_row_carries_a_figure_no_column_renders() {
    for row in every_row() {
      for key in row.figures.keys() {
        assert!(
          BAKED_COMPARISON.columns.iter().any(|c| &c.key == key),
          "{} carries a {key} figure, which no column renders",
          row.id
        );
      }
    }
  }

  // The two halves of "we do not publish a number we could not find": the
  // status a reader sees and the absent cell it describes. Either one alone
  // can be edited, so they are asserted against each other.
  #[test]
  fn unpriced_is_exactly_the_rows_missing_a_figure() {
    for row in every_row() {
      let complete = BAKED_COMPARISON
        .columns
        .iter()
        .all(|c| row.figure(c).is_some());
      assert_eq!(
        complete,
        row.provenance() != Provenance::Unpriced,
        "{} is marked {:?} but {} a figure in every column",
        row.id,
        row.provenance(),
        if complete { "carries" } else { "is missing" }
      );
    }
  }

  #[test]
  fn every_competitor_figure_can_be_traced_to_a_page_someone_can_open() {
    for row in every_row() {
      if row.provenance() == Provenance::Ours {
        continue;
      }
      let source = row
        .source
        .as_deref()
        .unwrap_or_else(|| panic!("{} cites no source", row.id));
      assert!(
        source.starts_with("https://"),
        "{}'s source is not a fetchable url: {source}",
        row.id
      );
    }
  }

  // Our own row is the reference every other number is read against, so its
  // absence would turn the table into a competitor list.
  #[test]
  fn our_own_price_is_in_the_table() {
    assert_eq!(
      BAKED_COMPARISON
        .rows
        .iter()
        .filter(|r| r.provenance() == Provenance::Ours)
        .count(),
      1
    );
  }

  #[test]
  fn a_source_is_cited_by_its_host() {
    let row = BAKED_COMPARISON
      .rows
      .iter()
      .find(|r| r.id == "thingsboard-cloud-business")
      .unwrap();
    assert_eq!(row.source_host(), Some("thingsboard.io"));
    let ours = BAKED_COMPARISON
      .rows
      .iter()
      .find(|r| r.provenance() == Provenance::Ours)
      .unwrap();
    assert_eq!(ours.source_host(), None);
  }

  #[test]
  fn a_threshold_that_would_flag_everything_or_nothing_is_not_a_threshold() {
    assert!((30..=365).contains(&BAKED_COMPARISON.stale_after_days));
  }

  #[test]
  fn a_date_past_the_threshold_reads_as_stale_and_one_inside_it_does_not() {
    let row = BAKED_COMPARISON
      .rows
      .iter()
      .find(|r| r.provenance() == Provenance::SingleFetch)
      .expect("the table has a single-fetch row to age");
    let checked = row.verified_on().unwrap();
    let window = BAKED_COMPARISON.stale_after_days;

    assert!(!row.is_stale(checked + time::Duration::days(window), window));
    assert!(row.is_stale(checked + time::Duration::days(window + 1), window));
  }

  #[test]
  fn our_own_price_never_goes_stale() {
    let ours = BAKED_COMPARISON
      .rows
      .iter()
      .find(|r| r.provenance() == Provenance::Ours)
      .unwrap();
    let long_after = ours.verified_on().unwrap() + time::Duration::days(3650);
    assert!(!ours.is_stale(long_after, BAKED_COMPARISON.stale_after_days));
  }
}
