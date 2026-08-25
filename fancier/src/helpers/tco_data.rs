// The self-hosting cost comparison on /self-hosting/, read from
// `public/data/self-hosting-tco.json` the same way the pricing comparison
// reads its own file: a copy baked in at build time so the prerendered HTML
// carries real numbers before any JavaScript runs, and a re-read of the
// deployed file on mount so a corrected figure ships with an asset deploy.
//
// It reuses `pricing_data`'s `Row`, `Column` and provenance rather than
// defining its own. The shapes are identical -- a labelled thing with a
// figure per column, a source, and a date it was last checked -- and a
// second copy of them would be free to drift in exactly the way the whole
// data-file arrangement exists to prevent.
//
// Hours are deliberately absent from the figures. What an hour of the
// reader's time is worth is theirs to know, so the page states the loaded
// band and lets them finish the arithmetic themselves; a number we invented
// for them would be the least believable thing on the page.
use super::pricing_data::{Column, Row, fetch_asset};
use serde::Deserialize;
use std::sync::LazyLock;

pub const ASSET_PATH: &str = "/data/self-hosting-tco.json";

const BAKED: &str = include_str!("../../public/data/self-hosting-tco.json");

static BAKED_TCO: LazyLock<Tco> =
  LazyLock::new(|| serde_json::from_str(BAKED).expect("self-hosting-tco.json is not valid"));

pub fn baked() -> Tco {
  BAKED_TCO.clone()
}

pub async fn fetch_published() -> Option<Tco> {
  fetch_asset(ASSET_PATH).await
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Tco {
  pub stale_after_days: i64,
  pub heading: String,
  pub subhead: String,
  pub columns: Vec<Column>,
  pub rows: Vec<Row>,
  pub hours: Hours,
  /// What the table cannot say for itself: the case for self-hosting
  /// anyway. A comparison page that only argues one way is an
  /// advertisement, and a reader can tell.
  pub concession: String,
  pub exit: Section,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Hours {
  pub heading: String,
  pub body: String,
  pub rate_low: String,
  pub rate_high: String,
  pub source: String,
  pub last_verified: String,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Section {
  pub heading: String,
  pub body: String,
}

// The file is hand-edited on a path that skips a Rust rebuild, so these are
// what stand between a slip in it and a wrong number on a public page. They
// mirror the pricing file's guards, because the failure modes are the same
// ones: an unreadable figure, a mangled thousands separator, and a total
// that no longer equals the lines above it.
#[cfg(test)]
mod the_tco_file_says_what_it_claims {
  use super::BAKED_TCO;
  use crate::helpers::pricing_data::Provenance;

  #[test]
  fn every_option_prices_every_column() {
    for row in &BAKED_TCO.rows {
      for column in &BAKED_TCO.columns {
        let figure = row.figure(column).unwrap_or_else(|| {
          panic!(
            "{} has no {} figure, so the table would print an empty cell in a three-column \
             comparison",
            row.id, column.key
          )
        });
        assert_eq!(
          figure.unit, column.unit,
          "{} states {} in {}, but that column is headed {}",
          row.id, column.key, figure.unit, column.unit
        );
        assert!(
          figure.amount().is_some(),
          "{}'s {} figure {:?} does not parse as an amount",
          row.id,
          column.key,
          figure.value
        );
      }
    }
  }

  /// The arithmetic a reader would check first, and the one an edit is most
  /// likely to break: change the licence or the server and forget the line
  /// that adds them up.
  #[test]
  fn the_subtotal_is_the_lines_above_it() {
    for row in &BAKED_TCO.rows {
      let part = |key: &str| {
        BAKED_TCO
          .columns
          .iter()
          .find(|c| c.key == key)
          .and_then(|c| row.figure(c))
          .and_then(|f| f.amount())
          .unwrap_or_default()
      };
      let (software, server, subtotal) = (part("software"), part("server"), part("subtotal"));
      assert!(
        (subtotal - (software + server)).abs() < 0.01,
        "{}: {software} plus {server} is not {subtotal}",
        row.id
      );
    }
  }

  /// On a phone the row stacks and reads top to bottom, so the line that
  /// adds the others up has to be the last one.
  #[test]
  fn the_subtotal_is_the_last_column() {
    assert_eq!(
      BAKED_TCO.columns.last().map(|c| c.key.as_str()),
      Some("subtotal")
    );
  }

  /// We are one of the three options, and a comparison of what other people
  /// charge that omits what we charge is not a comparison.
  #[test]
  fn our_own_price_is_one_of_the_options() {
    assert_eq!(
      BAKED_TCO
        .rows
        .iter()
        .filter(|r| r.provenance() == Provenance::Ours)
        .count(),
      1
    );
  }

  #[test]
  fn every_figure_someone_else_charges_cites_a_page_they_publish() {
    for row in &BAKED_TCO.rows {
      if row.provenance() == Provenance::Ours {
        continue;
      }
      let source = row
        .source
        .as_deref()
        .unwrap_or_else(|| panic!("{} cites no source", row.id));
      assert!(source.starts_with("https://"), "{}: {source}", row.id);
    }
  }

  /// The page's whole argument is that the hours are the cost nobody
  /// budgets. Losing the concession that follows would turn it into an
  /// advertisement, and losing the exit answer would leave the objection
  /// self-hosting is really about unanswered.
  #[test]
  fn the_page_still_argues_against_itself() {
    assert!(
      BAKED_TCO.concession.to_lowercase().contains("self-host"),
      "the case for self-hosting anyway is gone, which leaves only our side of it"
    );
    assert!(
      BAKED_TCO.exit.body.to_lowercase().contains("leave"),
      "the exit answer is gone, and being able to leave is the thing self-hosting is bought for"
    );
    assert!(
      !BAKED_TCO.hours.rate_low.is_empty() && !BAKED_TCO.hours.rate_high.is_empty(),
      "the hourly band is what makes the hours arithmetic checkable rather than asserted"
    );
  }
}
