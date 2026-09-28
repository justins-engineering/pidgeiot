//! The pure half of the telemetry CSV export: where one raw history page is cut and where the
//! next begins, how points become RFC 4180 rows a spreadsheet will not execute, and the file
//! name. `components::telemetry_export` drives the walk.

use capsules::TelemetryHistoryPoint;
use std::collections::HashMap;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

/// Most raw pages one export fetches, so at most 200,000 points in one file.
pub const EXPORT_MAX_PAGES: usize = 40;

/// The file's first line. Every row has these seven fields in this order.
pub const CSV_HEADER: &str = "reported_at,pigeon_id,pigeon_name,flock_name,key,value,value_num\r\n";

/// Longest `reported_at` in RFC 3339: `2026-09-26T00:00:00.123456789Z`.
const REPORTED_AT_MAX: usize = 30;

/// Six commas and a CRLF.
const ROW_SEPARATORS: usize = 8;

/// Longest scope name kept in a file name, in bytes.
const NAME_MAX: usize = 64;

const NAME_INFIX: &str = "-telemetry-";

/// What to do after one raw page, from [`plan_page`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStep {
  /// The range is covered.
  Done,
  /// Fetch the next page with `until` set to this instant, inclusive.
  Continue(OffsetDateTime),
  /// The page budget is spent; points at or before this instant are left out.
  Capped(OffsetDateTime),
  /// More points than one page holds share this instant, so no `until` pages past it. `None`
  /// when the server flagged a cut page and sent no points.
  Stalled(Option<OffsetDateTime>),
}

/// Plans one raw page (oldest first, the newest slice of its range): returns the index the kept
/// points start at, and the next step.
///
/// A cut page holds every point newer than its oldest instant but perhaps only some of the points
/// at it, because readings are stamped in whole seconds and many share one. Those are dropped here
/// and fetched whole by the next page, whose inclusive `until` is that instant.
pub fn plan_page(
  points: &[TelemetryHistoryPoint],
  truncated: bool,
  page_index: usize,
) -> (usize, PageStep) {
  if !truncated {
    return (0, PageStep::Done);
  }
  let Some(oldest) = points.first().map(|p| p.reported_at) else {
    return (0, PageStep::Stalled(None));
  };
  let keep_from = points.partition_point(|p| p.reported_at <= oldest);
  if keep_from == points.len() {
    return (keep_from, PageStep::Stalled(Some(oldest)));
  }
  if page_index + 1 >= EXPORT_MAX_PAGES {
    return (keep_from, PageStep::Capped(oldest));
  }
  (keep_from, PageStep::Continue(oldest))
}

/// Whether `s` is a finite number by the parse dovecote uses for `value_num`. Infinities and NaN
/// are not: no spreadsheet reads them as numbers.
pub fn is_number(s: &str) -> bool {
  s.parse::<f64>().is_ok_and(f64::is_finite)
}

/// Appends one CSV field. Text a spreadsheet would run as a formula (leading `=`, `+`, `-`, `@`,
/// tab or CR) gets a leading `'` unless it is a number, so `-97` stays numeric; a field holding a
/// comma, quote or line break is then quoted per RFC 4180.
pub fn push_field(out: &mut String, field: &str) {
  let formula = field.starts_with(['=', '+', '-', '@', '\t', '\r']) && !is_number(field);
  let quoted = field.contains([',', '"', '\r', '\n']);
  if quoted {
    out.push('"');
  }
  if formula {
    out.push('\'');
  }
  if quoted {
    for (i, part) in field.split('"').enumerate() {
      if i > 0 {
        out.push_str("\"\"");
      }
      out.push_str(part);
    }
    out.push('"');
  } else {
    out.push_str(field);
  }
}

/// Renders points as CSV rows without the header, oldest first. Ties sort by pigeon, key and
/// value, so two exports of one range are byte-identical. A pigeon missing from `pigeon_names`
/// gets an empty name.
pub fn rows_csv(
  points: &mut [TelemetryHistoryPoint],
  pigeon_names: &HashMap<String, String>,
  flock_name: &str,
) -> String {
  points.sort_by(|a, b| row_order(a).cmp(&row_order(b)));
  let capacity = points
    .iter()
    .map(|p| {
      let name = pigeon_names.get(&p.pigeon_id).map_or(0, String::len);
      // `value` twice: once as text, once more as `value_num` when it is a number.
      REPORTED_AT_MAX
        + p.pigeon_id.len()
        + name
        + flock_name.len()
        + p.key.len()
        + 2 * p.value.len()
        + ROW_SEPARATORS
    })
    .sum();
  let mut out = String::with_capacity(capacity);
  for p in points.iter() {
    push_field(&mut out, &rfc3339_utc(p.reported_at));
    out.push(',');
    push_field(&mut out, &p.pigeon_id);
    out.push(',');
    push_field(
      &mut out,
      pigeon_names.get(&p.pigeon_id).map_or("", String::as_str),
    );
    out.push(',');
    push_field(&mut out, flock_name);
    out.push(',');
    push_field(&mut out, &p.key);
    out.push(',');
    push_field(&mut out, &p.value);
    out.push(',');
    if is_number(&p.value) {
      out.push_str(&p.value);
    }
    out.push_str("\r\n");
  }
  out
}

/// `<scope>-telemetry-<since>-<until>.csv`, safe on common file systems. The scope name keeps
/// ASCII letters, digits, `_` and `.`, every other run becomes one `-`, and `fallback` stands in
/// when nothing is left. The stamps are RFC 3339 UTC without `-` and `:` (`20260926T000000Z`),
/// since Windows refuses `:`.
pub fn file_name(
  scope_name: &str,
  fallback: &str,
  since: OffsetDateTime,
  until: OffsetDateTime,
) -> String {
  let mut safe = String::with_capacity(scope_name.len());
  for c in scope_name.chars() {
    if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
      safe.push(c);
    } else if !safe.ends_with('-') {
      safe.push('-');
    }
  }
  let mut stem = safe.trim_matches(['-', '.']);
  // ASCII only by now, so any byte index is a char boundary.
  stem = stem[..stem.len().min(NAME_MAX)].trim_end_matches(['-', '.']);
  if stem.is_empty() {
    stem = fallback;
  }

  let mut name =
    String::with_capacity(stem.len() + NAME_INFIX.len() + 2 * REPORTED_AT_MAX + ".csv".len() + 1);
  name.push_str(stem);
  name.push_str(NAME_INFIX);
  push_stamp(&mut name, since);
  name.push('-');
  push_stamp(&mut name, until);
  name.push_str(".csv");
  name
}

/// Whether `key` can be asked for by name: the history routes split `keys` on commas and trim
/// each entry, so a key holding a comma or edge whitespace is reachable only as "all keys".
pub fn filterable_key(key: &str) -> bool {
  !key.is_empty() && !key.contains(',') && key.trim() == key
}

fn row_order(p: &TelemetryHistoryPoint) -> (OffsetDateTime, &str, &str, &str) {
  (p.reported_at, &p.pigeon_id, &p.key, &p.value)
}

/// RFC 3339 in UTC, as the file writes `reported_at`.
pub fn rfc3339_utc(t: OffsetDateTime) -> String {
  t.to_offset(UtcOffset::UTC)
    .format(&Rfc3339)
    .unwrap_or_default()
}

fn push_stamp(out: &mut String, t: OffsetDateTime) {
  out.extend(rfc3339_utc(t).chars().filter(|c| *c != '-' && *c != ':'));
}

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::TELEMETRY_HISTORY_MAX_POINTS;
  use time::macros::datetime;

  fn field(s: &str) -> String {
    let mut out = String::new();
    push_field(&mut out, s);
    out
  }

  fn point(pigeon_id: &str, key: &str, value: &str, at: OffsetDateTime) -> TelemetryHistoryPoint {
    TelemetryHistoryPoint {
      pigeon_id: pigeon_id.to_string(),
      key: key.to_string(),
      value: value.to_string(),
      value_num: value.parse().ok(),
      reported_at: at,
    }
  }

  #[test]
  fn plain_fields_pass_through() {
    assert_eq!(field("rsrp_dbm"), "rsrp_dbm");
    assert_eq!(field(""), "");
  }

  #[test]
  fn commas_quotes_and_line_breaks_are_quoted() {
    assert_eq!(field("1,5"), "\"1,5\"");
    assert_eq!(field("say \"hi\", ok"), "\"say \"\"hi\"\", ok\"");
    assert_eq!(field("a\"b"), "\"a\"\"b\"");
    assert_eq!(field("line\nbreak"), "\"line\nbreak\"");
    assert_eq!(field("cr\rhere"), "\"cr\rhere\"");
  }

  #[test]
  fn formula_leads_are_neutralised() {
    assert_eq!(field("=1+1"), "'=1+1");
    assert_eq!(field("+x"), "'+x");
    assert_eq!(field("-x"), "'-x");
    assert_eq!(field("@SUM(1)"), "'@SUM(1)");
    assert_eq!(field("\tcmd"), "'\tcmd");
    assert_eq!(field("\rcmd"), "\"'\rcmd\"");
    assert_eq!(field("=A1,B1"), "\"'=A1,B1\"");
  }

  #[test]
  fn signed_numbers_stay_numbers() {
    assert_eq!(field("-97"), "-97");
    assert_eq!(field("-1.5e3"), "-1.5e3");
    assert_eq!(field("+5"), "+5");
  }

  #[test]
  fn infinities_and_nan_are_text() {
    assert_eq!(field("-inf"), "'-inf");
    assert_eq!(field("nan"), "nan");
    assert!(!is_number("inf"));
    assert!(!is_number("NaN"));
    assert!(is_number("1e3"));
  }

  #[test]
  fn rows_have_seven_fields_in_order_and_crlf() {
    let at = datetime!(2026-09-26 12:00:00 UTC);
    let mut points = vec![point("p1", "rsrp_dbm", "-97", at)];
    let names = HashMap::from([("p1".to_string(), "Holyoke 414".to_string())]);
    assert_eq!(
      rows_csv(&mut points, &names, "PVTA signs"),
      "2026-09-26T12:00:00Z,p1,Holyoke 414,PVTA signs,rsrp_dbm,-97,-97\r\n"
    );
  }

  #[test]
  fn value_num_is_the_raw_number_or_empty() {
    let at = datetime!(2026-09-26 12:00:00 UTC);
    let mut points = vec![
      point("p1", "a", "1e3", at),
      point("p1", "b", "inf", at),
      point("p1", "c", "0.13.6", at),
    ];
    let csv = rows_csv(&mut points, &HashMap::new(), "");
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(lines[0], "2026-09-26T12:00:00Z,p1,,,a,1e3,1e3");
    assert_eq!(lines[1], "2026-09-26T12:00:00Z,p1,,,b,inf,");
    assert_eq!(lines[2], "2026-09-26T12:00:00Z,p1,,,c,0.13.6,");
    assert_eq!(lines[3], "");
    assert!(!csv.starts_with("reported_at"));
  }

  #[test]
  fn rows_sort_by_time_then_pigeon_then_key() {
    let t1 = datetime!(2026-09-26 12:00:00 UTC);
    let t2 = datetime!(2026-09-26 12:00:01 UTC);
    let mut points = vec![
      point("p2", "a", "1", t2),
      point("p2", "b", "1", t1),
      point("p1", "b", "1", t1),
      point("p1", "a", "1", t1),
    ];
    let csv = rows_csv(&mut points, &HashMap::new(), "");
    let order: Vec<(&str, &str)> = csv
      .lines()
      .map(|l| {
        let f: Vec<&str> = l.split(',').collect();
        (f[1], f[4])
      })
      .collect();
    assert_eq!(
      order,
      vec![("p1", "a"), ("p1", "b"), ("p2", "b"), ("p2", "a")]
    );
  }

  #[test]
  fn sub_second_times_keep_their_fraction() {
    let at = datetime!(2026-07-17 15:34:41.389358 UTC);
    let mut points = vec![point("p1", "a", "1", at)];
    let csv = rows_csv(&mut points, &HashMap::new(), "");
    assert!(csv.starts_with("2026-07-17T15:34:41.389358Z,"), "{csv}");
  }

  fn at(secs: i64) -> OffsetDateTime {
    datetime!(2026-09-26 00:00:00 UTC) + time::Duration::seconds(secs)
  }

  #[test]
  fn a_complete_page_is_kept_whole() {
    let points = vec![point("p", "a", "1", at(0)), point("p", "a", "1", at(1))];
    assert_eq!(plan_page(&points, false, 0), (0, PageStep::Done));
  }

  #[test]
  fn a_cut_page_drops_its_oldest_second_and_continues_there() {
    let mut points: Vec<_> = ["a", "b", "c"]
      .iter()
      .map(|k| point("p", k, "1", at(0)))
      .collect();
    points.push(point("p", "a", "1", at(1)));
    points.push(point("p", "a", "1", at(2)));
    assert_eq!(plan_page(&points, true, 0), (3, PageStep::Continue(at(0))));
  }

  #[test]
  fn a_cut_page_inside_one_second_stalls() {
    let points = vec![point("p", "a", "1", at(5)), point("p", "b", "1", at(5))];
    assert_eq!(
      plan_page(&points, true, 0),
      (2, PageStep::Stalled(Some(at(5))))
    );
    assert_eq!(plan_page(&[], true, 0), (0, PageStep::Stalled(None)));
  }

  #[test]
  fn the_last_allowed_page_caps() {
    let points = vec![point("p", "a", "1", at(0)), point("p", "a", "1", at(1))];
    assert_eq!(
      plan_page(&points, true, EXPORT_MAX_PAGES - 1),
      (1, PageStep::Capped(at(0)))
    );
    assert_eq!(
      plan_page(&points, true, EXPORT_MAX_PAGES - 2),
      (1, PageStep::Continue(at(0)))
    );
  }

  /// dovecote's raw read: the newest `TELEMETRY_HISTORY_MAX_POINTS` of `[since, until]`, oldest
  /// first, with ties at the cut broken in whatever order the database likes.
  fn serve(
    table: &[TelemetryHistoryPoint],
    until: OffsetDateTime,
  ) -> (Vec<TelemetryHistoryPoint>, bool) {
    let mut rows: Vec<_> = table
      .iter()
      .filter(|p| p.reported_at <= until)
      .cloned()
      .collect();
    rows.sort_by(|a, b| {
      b.reported_at
        .cmp(&a.reported_at)
        .then_with(|| a.key.cmp(&b.key))
    });
    let truncated = rows.len() > TELEMETRY_HISTORY_MAX_POINTS;
    rows.truncate(TELEMETRY_HISTORY_MAX_POINTS);
    rows.reverse();
    (rows, truncated)
  }

  #[test]
  fn a_walk_over_a_cut_inside_a_second_exports_every_point_once() {
    // Seven keys a second, so 5,000 is never a whole number of seconds.
    let keys = ["k1", "k2", "k3", "k4", "k5", "k6", "k7"];
    let table: Vec<_> = (0..1_600)
      .flat_map(|s| keys.iter().map(move |k| point("p", k, "1", at(s))))
      .collect();
    let mut until = at(1_599);
    let mut kept = Vec::new();
    let mut pages = 0;
    loop {
      let (page, truncated) = serve(&table, until);
      let (keep_from, step) = plan_page(&page, truncated, pages);
      pages += 1;
      kept.extend_from_slice(&page[keep_from..]);
      match step {
        PageStep::Done => break,
        PageStep::Continue(t) => until = t,
        other => panic!("unexpected step {other:?}"),
      }
    }
    assert_eq!(pages, 3);
    let mut seen: Vec<(OffsetDateTime, String)> = kept
      .iter()
      .map(|p| (p.reported_at, p.key.clone()))
      .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), kept.len(), "a point was exported twice");
    assert_eq!(kept.len(), table.len(), "a point was left out");
  }

  #[test]
  fn file_names_are_safe_and_carry_the_range() {
    let since = datetime!(2026-09-26 00:00:00 UTC);
    let until = datetime!(2026-09-28 00:00:00 UTC);
    assert_eq!(
      file_name("Holyoke signs / PVTA", "flock", since, until),
      "Holyoke-signs-PVTA-telemetry-20260926T000000Z-20260928T000000Z.csv"
    );
    assert_eq!(
      file_name("Café: 12/B", "flock", since, until),
      "Caf-12-B-telemetry-20260926T000000Z-20260928T000000Z.csv"
    );
    assert_eq!(
      file_name("..-sign-414-.", "pigeon", since, until),
      "sign-414-telemetry-20260926T000000Z-20260928T000000Z.csv"
    );
  }

  #[test]
  fn file_names_fall_back_and_stay_short() {
    let since = datetime!(2026-09-26 00:00:00 UTC);
    let until = datetime!(2026-09-28 00:00:00 UTC);
    assert!(file_name("", "pigeon", since, until).starts_with("pigeon-telemetry-"));
    assert!(file_name("/// ::", "flock", since, until).starts_with("flock-telemetry-"));
    let stem = |name: &str| {
      file_name(name, "flock", since, until)
        .replace("-telemetry-20260926T000000Z-20260928T000000Z.csv", "")
    };
    assert_eq!(
      stem(&("é".repeat(10) + &"x".repeat(100))),
      "x".repeat(NAME_MAX)
    );
    // A cut that lands on a separator does not leave it dangling.
    let edge = "x".repeat(NAME_MAX - 1) + " " + &"y".repeat(10);
    assert_eq!(stem(&edge), "x".repeat(NAME_MAX - 1));
  }

  #[test]
  fn only_keys_the_routes_can_filter_by_name_are_filterable() {
    assert!(filterable_key("rsrp_dbm"));
    assert!(!filterable_key("a,b"));
    assert!(!filterable_key(" a"));
    assert!(!filterable_key("a "));
    assert!(!filterable_key(""));
  }
}
