//! Renders `docs/api.md` into the API reference page: heading ids anything
//! can link to, the contents tree the sidebar draws, and the rows of the
//! document's own routes table.
//!
//! Slugs follow the rule GitHub uses, because the same file is also read as
//! rendered markdown (`/api-reference/index.md`, and the repository view),
//! and a link written in it has to land in all three.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
use std::collections::HashMap;

/// The single source of truth this page renders. `docs/api.md` is also
/// copied verbatim as the markdown variant served under content
/// negotiation, so nothing here may depend on markup only this renderer
/// understands.
pub const API_MD: &str = include_str!("../../../docs/api.md");

/// One route as the document's leading table lists it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct RouteRow {
  /// Method and path, as the heading spells them.
  pub route: String,
  /// The route's own `Auth:` line, or empty when it has none.
  pub auth: String,
  /// Anchor of the heading this row points at.
  pub slug: String,
}

/// A single route under a resource group.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TocLeaf {
  pub text: String,
  pub slug: String,
}

/// A resource group (H3) and the routes under it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TocGroup {
  pub text: String,
  pub slug: String,
  pub leaves: Vec<TocLeaf>,
}

/// A surface (H2) and its resource groups.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TocSection {
  pub text: String,
  pub slug: String,
  pub groups: Vec<TocGroup>,
}

pub struct RenderedDoc {
  pub html: String,
  pub toc: Vec<TocSection>,
  pub routes: Vec<RouteRow>,
}

/// GitHub's heading-slug rule: lowercase, drop everything that is not
/// alphanumeric, `-` or `_`, and hyphenate spaces.
pub fn slugify(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for ch in text.chars() {
    if ch.is_alphanumeric() {
      out.extend(ch.to_lowercase());
    } else if ch == '-' || ch == '_' {
      out.push(ch);
    } else if ch == ' ' {
      out.push('-');
    }
  }
  out
}

/// Hands out one id per heading, numbering repeats the way GitHub does so
/// the first heading with a given text keeps the bare slug.
#[derive(Default)]
struct Slugger {
  seen: HashMap<String, usize>,
}

impl Slugger {
  fn unique(&mut self, text: &str) -> String {
    let base = slugify(text);
    match self.seen.get_mut(&base) {
      Some(count) => {
        *count += 1;
        format!("{base}-{count}")
      }
      None => {
        self.seen.insert(base.clone(), 0);
        base
      }
    }
  }
}

fn level_number(level: HeadingLevel) -> u8 {
  match level {
    HeadingLevel::H1 => 1,
    HeadingLevel::H2 => 2,
    HeadingLevel::H3 => 3,
    HeadingLevel::H4 => 4,
    HeadingLevel::H5 => 5,
    HeadingLevel::H6 => 6,
  }
}

/// Plain text of a heading's inline events: a code span contributes its
/// literal, a link its label, which is what a slug is built from.
fn inline_text(events: &[Event<'_>]) -> String {
  let mut text = String::new();
  for event in events {
    match event {
      Event::Text(t) | Event::Code(t) => text.push_str(t),
      Event::SoftBreak | Event::HardBreak => text.push(' '),
      _ => {}
    }
  }
  text
}

/// Method and path if this heading names a route, e.g. `GET /flocks`.
fn route_of(text: &str) -> Option<&str> {
  let (method, path) = text.split_once(' ')?;
  let known = method
    .split('|')
    .all(|m| matches!(m, "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE"));
  if known && !method.is_empty() && path.starts_with('/') && !path.contains(' ') {
    Some(text)
  } else {
    None
  }
}

/// The `**Auth:** ...` line each route heading is followed by, keyed by the
/// route it belongs to. Read off the source rather than the event stream so
/// the rule stays the one a reader of the markdown sees.
fn auth_lines(md: &str) -> HashMap<String, String> {
  let mut found = HashMap::new();
  let mut fenced = false;
  let mut route: Option<String> = None;
  for line in md.lines() {
    if line.starts_with("```") {
      fenced = !fenced;
      continue;
    }
    if fenced {
      continue;
    }
    if let Some(rest) = line.strip_prefix("#### ") {
      route = rest
        .strip_prefix('`')
        .and_then(|r| r.strip_suffix('`'))
        .and_then(route_of)
        .map(str::to_string);
      continue;
    }
    if let (Some(current), Some(auth)) = (route.as_ref(), line.strip_prefix("**Auth:** ")) {
      found.insert(current.clone(), auth.trim().to_string());
      route = None;
    }
  }
  found
}

/// Escapes what a markdown table cell cannot carry literally.
#[cfg(test)]
fn escape_cell(text: &str) -> String {
  text.replace('|', "\\|")
}

/// Wraps each surface in a `details` so a reader can fold one away. It ships
/// open: a closed one would hide its text from find-in-page and from anything
/// reading the page without running it.
const SURFACE_OPEN: &str =
  "<details open class=\"api-surface\"><summary class=\"api-surface-head\">";
const SURFACE_MID: &str = "</summary><div class=\"api-surface-body\">";
const SURFACE_CLOSE: &str = "</div></details>\n";

pub fn render(md: &str) -> RenderedDoc {
  let mut options = Options::empty();
  options.insert(Options::ENABLE_TABLES);
  options.insert(Options::ENABLE_STRIKETHROUGH);
  options.insert(Options::ENABLE_FOOTNOTES);

  let auth = auth_lines(md);
  let mut slugger = Slugger::default();
  let mut toc: Vec<TocSection> = Vec::new();
  let mut routes: Vec<RouteRow> = Vec::new();
  let mut out: Vec<Event> = Vec::new();
  let mut heading: Option<(u8, Vec<Event>)> = None;
  let mut in_surface = false;

  for event in Parser::new_ext(md, options) {
    match event {
      Event::Start(Tag::Heading { level, .. }) => heading = Some((level_number(level), Vec::new())),
      Event::End(TagEnd::Heading(_)) => {
        let Some((level, inner)) = heading.take() else {
          continue;
        };
        let text = inline_text(&inner);
        let slug = slugger.unique(&text);

        if level == 2 {
          if in_surface {
            out.push(Event::Html(SURFACE_CLOSE.into()));
          }
          in_surface = true;
          out.push(Event::Html(SURFACE_OPEN.into()));
          toc.push(TocSection {
            text: text.clone(),
            slug: slug.clone(),
            groups: Vec::new(),
          });
        } else if level == 3 {
          if let Some(section) = toc.last_mut() {
            section.groups.push(TocGroup {
              text: text.clone(),
              slug: slug.clone(),
              leaves: Vec::new(),
            });
          }
        } else if level == 4 {
          if let Some(group) = toc.last_mut().and_then(|s| s.groups.last_mut()) {
            group.leaves.push(TocLeaf {
              text: text.clone(),
              slug: slug.clone(),
            });
          }
          if let Some(route) = route_of(&text) {
            routes.push(RouteRow {
              route: route.to_string(),
              auth: auth.get(route).cloned().unwrap_or_default(),
              slug: slug.clone(),
            });
          }
        }

        out.push(Event::Html(format!("<h{level} id=\"{slug}\">").into()));
        out.extend(inner);
        out.push(Event::Html(
          format!(
            "<a class=\"heading-anchor\" href=\"#{slug}\" \
             aria-label=\"Link to this heading\">#</a></h{level}>"
          )
          .into(),
        ));
        if level == 2 {
          out.push(Event::Html(SURFACE_MID.into()));
        }
      }
      other => match heading.as_mut() {
        Some((_, inner)) => inner.push(other),
        None => out.push(other),
      },
    }
  }
  if in_surface {
    out.push(Event::Html(SURFACE_CLOSE.into()));
  }

  let mut body = String::new();
  html::push_html(&mut body, out.into_iter());
  // Wide tables scroll inside their own box rather than pushing the page
  // sideways -- `main` clips horizontal overflow app-wide, so a table that
  // overflowed would be silently cut off instead.
  let body = body
    .replace("<table>", "<div class=\"table-scroll\"><table>")
    .replace("</table>", "</table></div>");

  RenderedDoc {
    html: body,
    toc,
    routes,
  }
}

/// The body rows of the document's own "Routes at a glance" table, verbatim.
#[cfg(test)]
fn glance_lines(md: &str) -> Vec<&str> {
  let mut lines = Vec::new();
  let mut inside = false;
  for line in md.lines() {
    if let Some(rest) = line.strip_prefix("## ") {
      inside = rest.trim() == "Routes at a glance";
      continue;
    }
    if inside && line.starts_with("| [") {
      lines.push(line);
    }
  }
  lines
}

/// The rows of the document's own "Routes at a glance" table, as committed.
/// Compared against [`render`]'s route list so the table cannot fall behind
/// the headings it indexes.
#[cfg(test)]
pub fn glance_rows(md: &str) -> Vec<RouteRow> {
  let mut rows = Vec::new();
  for line in glance_lines(md) {
    let cells: Vec<&str> = line.trim_matches('|').split(" | ").collect();
    let Some(link) = cells.first() else { continue };
    let Some((route, target)) = link.split_once("](#") else {
      continue;
    };
    rows.push(RouteRow {
      route: route
        .trim()
        .trim_start_matches('[')
        .trim_matches('`')
        .replace("\\|", "|"),
      auth: cells
        .get(1)
        .map(|c| c.trim().to_string())
        .unwrap_or_default(),
      slug: target.trim_end_matches(')').to_string(),
    });
  }
  rows
}

/// The table body [`glance_rows`] expects, rebuilt from the headings. The
/// "what it does" column is prose and is left to the author; everything a
/// reader navigates by is generated here.
#[cfg(test)]
pub fn glance_table(routes: &[RouteRow]) -> Vec<String> {
  routes
    .iter()
    .map(|r| {
      format!(
        "| [`{}`](#{}) | {} |",
        escape_cell(&r.route),
        r.slug,
        r.auth
      )
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn slugs_follow_the_github_rule() {
    assert_eq!(slugify("Routes at a glance"), "routes-at-a-glance");
    assert_eq!(slugify("Per-tier limits"), "per-tier-limits");
    assert_eq!(
      slugify("GET /pigeons/:pigeon_id/shadow"),
      "get-pigeonspigeon_idshadow"
    );
    assert_eq!(
      slugify("GET|HEAD /.well-known/api-catalog"),
      "gethead-well-knownapi-catalog"
    );
    assert_eq!(
      slugify("CoAP device surface (via the loft terminator)"),
      "coap-device-surface-via-the-loft-terminator"
    );
  }

  #[test]
  fn repeated_headings_get_numbered_ids() {
    let mut slugger = Slugger::default();
    assert_eq!(slugger.unique("Shadow"), "shadow");
    assert_eq!(slugger.unique("Shadow"), "shadow-1");
    assert_eq!(slugger.unique("Shadow"), "shadow-2");
  }

  #[test]
  fn route_headings_are_told_apart_from_prose_headings() {
    assert_eq!(route_of("GET /flocks"), Some("GET /flocks"));
    assert_eq!(
      route_of("GET|HEAD /.well-known/api-catalog"),
      Some("GET|HEAD /.well-known/api-catalog")
    );
    assert_eq!(route_of("Per-tier limits"), None);
    assert_eq!(route_of("Raw mode"), None);
    assert_eq!(route_of("GET the picture"), None);
  }

  /// Every ATX heading outside a code fence, which is what the renderer
  /// sees too.
  fn heading_count(md: &str) -> usize {
    let mut fenced = false;
    let mut count = 0;
    for line in md.lines() {
      if line.starts_with("```") {
        fenced = !fenced;
      } else if !fenced && line.starts_with('#') && line.contains("# ") {
        count += 1;
      }
    }
    count
  }

  #[test]
  fn every_heading_gets_an_id_and_an_anchor() {
    let doc = render(API_MD);
    let headings = heading_count(API_MD);
    assert!(headings > 100, "only {headings} headings");
    assert_eq!(
      doc.html.matches("class=\"heading-anchor\"").count(),
      headings
    );
    assert_eq!(doc.html.matches(" id=\"").count(), headings);
    assert!(doc.html.contains("<h4 id=\"get-pigeonspigeon_idshadow\">"));
  }

  #[test]
  fn surfaces_are_wrapped_open_so_no_text_is_hidden() {
    let doc = render(API_MD);
    let opened = doc
      .html
      .matches("<details open class=\"api-surface\">")
      .count();
    assert_eq!(opened, doc.toc.len());
    assert_eq!(doc.html.matches("</details>").count(), opened);
    assert!(!doc.html.contains("<details class="));
  }

  #[test]
  fn contents_tree_nests_surfaces_groups_and_routes() {
    let doc = render(API_MD);
    let dashboard = doc
      .toc
      .iter()
      .find(|s| s.text == "Dashboard API")
      .expect("dashboard surface");
    let shadow = dashboard
      .groups
      .iter()
      .find(|g| g.text == "Shadow")
      .expect("shadow group");
    assert!(
      shadow
        .leaves
        .iter()
        .any(|l| l.text == "PUT /pigeons/:pigeon_id/shadow")
    );
    assert!(doc.toc.iter().all(|s| !s.slug.is_empty()));
  }

  #[test]
  fn the_routes_table_matches_the_headings_it_indexes() {
    let doc = render(API_MD);
    let committed = glance_rows(API_MD);
    assert_eq!(
      committed.len(),
      doc.routes.len(),
      "the routes table and the route headings disagree on how many routes exist"
    );
    for (row, heading) in committed.iter().zip(doc.routes.iter()) {
      assert_eq!(row, heading, "routes table row is stale");
    }
  }

  #[test]
  fn the_routes_table_is_what_the_generator_would_write() {
    let doc = render(API_MD);
    let generated = glance_table(&doc.routes);
    let committed: Vec<String> = glance_lines(API_MD)
      .iter()
      .map(|l| {
        let cut = l.rfind(" | ").expect("purpose column");
        format!("{} |", &l[..cut])
      })
      .collect();
    assert_eq!(committed, generated);
  }

  #[test]
  fn every_route_states_its_auth_and_every_row_says_what_it_does() {
    let doc = render(API_MD);
    assert!(doc.routes.iter().all(|r| !r.auth.is_empty()));
    for line in glance_lines(API_MD) {
      let purpose = line
        .trim_end_matches('|')
        .rsplit(" | ")
        .next()
        .unwrap_or("");
      assert!(purpose.trim().len() > 10, "empty purpose: {line}");
    }
  }

  #[test]
  fn every_in_document_link_lands_on_a_heading() {
    let doc = render(API_MD);
    let mut ids: Vec<&str> = doc.toc.iter().map(|s| s.slug.as_str()).collect();
    for section in &doc.toc {
      for group in &section.groups {
        ids.push(&group.slug);
        ids.extend(group.leaves.iter().map(|l| l.slug.as_str()));
      }
    }
    for target in API_MD.split("](#").skip(1) {
      let anchor = target.split(')').next().unwrap_or_default();
      assert!(
        ids.contains(&anchor) || doc.html.contains(&format!("id=\"{anchor}\"")),
        "link to #{anchor} has no heading"
      );
    }
  }
}
