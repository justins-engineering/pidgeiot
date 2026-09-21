//! Renders a published legal document from its in-repo markdown.
//!
//! The four documents under `docs/legal/` are what counsel and the business
//! folder both reference, and the same files are served as the markdown
//! variant under content negotiation, so nothing here may depend on markup
//! only this renderer understands.
//!
//! Slugs come from `api_doc` rather than a second copy of the rule: the same
//! document is read as rendered markdown too, and a link written in it has to
//! land in both.

use crate::helpers::api_doc::{Slugger, inline_text};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

/// Stands in each document for the date its deployment publishes it.
const LAST_UPDATED: &str = "{{LAST_UPDATED}}";

/// The heading level as the digit its tag carries.
fn level_digit(level: HeadingLevel) -> char {
  match level {
    HeadingLevel::H1 => '1',
    HeadingLevel::H2 => '2',
    HeadingLevel::H3 => '3',
    HeadingLevel::H4 => '4',
    HeadingLevel::H5 => '5',
    HeadingLevel::H6 => '6',
  }
}

/// Renders a published legal document, substituting the "Last updated"
/// token with `version`.
///
/// Headings get GitHub-rule ids so a clause can be linked to, and tables
/// scroll inside their own box. No folding and no route scraping: those are
/// API-reference affordances, and a contract clause a reader can collapse is
/// wrong.
pub fn render(src: &str, version: &str) -> String {
  let src = src.replace(LAST_UPDATED, version);

  let mut options = Options::empty();
  options.insert(Options::ENABLE_TABLES);
  options.insert(Options::ENABLE_STRIKETHROUGH);
  options.insert(Options::ENABLE_FOOTNOTES);

  let mut slugger = Slugger::default();
  let mut out: Vec<Event> = Vec::new();
  let mut heading: Option<(char, Vec<Event>)> = None;

  for event in Parser::new_ext(&src, options) {
    match event {
      Event::Start(Tag::Heading { level, .. }) => heading = Some((level_digit(level), Vec::new())),
      Event::End(TagEnd::Heading(_)) => {
        let Some((digit, inner)) = heading.take() else {
          continue;
        };
        let slug = slugger.unique(&inline_text(&inner));
        let mut open = String::with_capacity(10 + slug.len());
        open.push_str("<h");
        open.push(digit);
        open.push_str(" id=\"");
        open.push_str(&slug);
        open.push_str("\">");
        out.push(Event::Html(open.into()));
        out.extend(inner);
        let mut close = String::with_capacity(5);
        close.push_str("</h");
        close.push(digit);
        close.push('>');
        out.push(Event::Html(close.into()));
      }
      other => match heading.as_mut() {
        Some((_, inner)) => inner.push(other),
        None => out.push(other),
      },
    }
  }

  let mut body = String::new();
  html::push_html(&mut body, out.into_iter());
  // `main` clips horizontal overflow app-wide, so a wide table would be cut
  // off rather than scrolled. The sub-processor list is seven columns.
  body
    .replace("<table>", "<div class=\"table-scroll\"><table>")
    .replace("</table>", "</table></div>")
}
