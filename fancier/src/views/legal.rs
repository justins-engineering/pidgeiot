//! The four published legal documents, each rendered from its own markdown
//! source in `docs/legal/`.
//!
//! One component renders all four: they differ only in which file they carry,
//! which version dates them, and the id their section takes. The documents
//! themselves are the contract, so nothing here rewrites them -- the only
//! substitution is the publication date, which the deployment decides.

use crate::helpers::legal_doc;
use capsules::{PRIVACY_NOTICE_VERSION, TERMS_VERSION};
use dioxus::prelude::*;

const TERMS_MD: &str = include_str!("../../../docs/legal/terms.md");
const PRIVACY_MD: &str = include_str!("../../../docs/legal/privacy.md");
const DPA_MD: &str = include_str!("../../../docs/legal/dpa.md");
const SUBPROCESSORS_MD: &str = include_str!("../../../docs/legal/subprocessors.md");

// Scoped to this page's container so it never leaks into the rest of the
// app, same convention as the API reference's block: colors reference the
// DaisyUI --color-* custom properties directly, so the document tracks the
// theme toggle without a utility class on every element pulldown-cmark
// generates.
const MARKDOWN_STYLE: &str = r#"<style>
  #legal-md { color: var(--color-base-content); line-height: 1.7; overflow-wrap: break-word; }
  #legal-md h1 { font-size: 2.25rem; font-weight: 800; letter-spacing: -.025em; margin: 0 0 1rem; }
  #legal-md h2 { font-size: 1.5rem; font-weight: 700; margin: 2.5rem 0 1rem; color: var(--color-primary); border-bottom: 1px solid var(--color-base-300); padding-bottom: .4rem; }
  #legal-md h3 { font-size: 1.2rem; font-weight: 700; margin: 1.75rem 0 .75rem; }
  #legal-md h4 { font-size: 1.05rem; font-weight: 700; margin: 1.5rem 0 .5rem; color: var(--color-secondary); }
  #legal-md p { margin: 1rem 0; }
  #legal-md a { color: var(--color-secondary); text-decoration: underline; text-underline-offset: 2px; }
  #legal-md ul, #legal-md ol { margin: 1rem 0 1rem 1.5rem; }
  #legal-md ul { list-style: disc; }
  #legal-md ol { list-style: decimal; }
  #legal-md li { margin: .35rem 0; }
  #legal-md li > p { margin: .25rem 0; }
  #legal-md code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--color-base-300); color: var(--color-base-content); padding: .15em .4em; border-radius: .3em; font-size: .9em; overflow-wrap: anywhere; }
  #legal-md table { width: 100%; border-collapse: collapse; margin: 0; }
  /* Wide tables scroll in their own box: the sub-processor list is seven
     columns, and `main` clips rather than scrolls. */
  #legal-md .table-scroll { overflow-x: auto; margin: 1.25rem 0; }
  #legal-md th, #legal-md td { border: 1px solid var(--color-base-300); padding: .5rem .75rem; text-align: left; vertical-align: top; }
  #legal-md th { background: var(--color-base-200); font-weight: 700; }
  #legal-md blockquote { border-left: 3px solid var(--color-primary); padding-left: 1rem; margin: 1rem 0; opacity: .8; }
  #legal-md hr { border: none; border-top: 1px solid var(--color-base-300); margin: 2rem 0; }
  #legal-md strong { font-weight: 700; }
  /* Clears the sticky navbar when a link into a clause lands. */
  #legal-md :is(h1, h2, h3, h4)[id] { scroll-margin-top: 6rem; }
</style>"#;

/// One published document. The "Last updated" line is the document's own,
/// dated by `version`, so the page shell prints no second one.
#[component]
fn LegalDocument(section_id: &'static str, version: &'static str, source: &'static str) -> Element {
  // Style and document concatenated once here rather than in the
  // attribute, which would rebuild the whole document through std::fmt on
  // every render of the component.
  let body = use_memo(move || {
    let rendered = legal_doc::render(source, version);
    let mut html = String::with_capacity(MARKDOWN_STYLE.len() + rendered.len());
    html.push_str(MARKDOWN_STYLE);
    html.push_str(&rendered);
    html
  });

  rsx! {
    section { id: section_id, class: "py-16 md:py-24",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2",
          "Rendered directly from docs/legal/ in the repository"
        }
        div { id: "legal-md", dangerous_inner_html: "{body}" }
      }
    }
  }
}

#[component]
pub fn TermsPage() -> Element {
  rsx! {
    LegalDocument { section_id: "terms-of-service", version: TERMS_VERSION, source: TERMS_MD }
  }
}

#[component]
pub fn PrivacyPage() -> Element {
  rsx! {
    LegalDocument {
      section_id: "privacy-policy",
      version: PRIVACY_NOTICE_VERSION,
      source: PRIVACY_MD,
    }
  }
}

#[component]
pub fn DpaPage() -> Element {
  rsx! {
    LegalDocument {
      section_id: "data-processing-agreement",
      version: TERMS_VERSION,
      source: DPA_MD,
    }
  }
}

#[component]
pub fn SubprocessorsPage() -> Element {
  rsx! {
    LegalDocument { section_id: "subprocessors-list", version: TERMS_VERSION, source: SUBPROCESSORS_MD }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Route;
  use std::str::FromStr;

  const DOCUMENTS: [(&str, &str); 4] = [
    ("terms.md", TERMS_MD),
    ("privacy.md", PRIVACY_MD),
    ("dpa.md", DPA_MD),
    ("subprocessors.md", SUBPROCESSORS_MD),
  ];

  /// The worst outcome available in this task is publishing the working
  /// draft counsel is still marking up, so the markers it carries are the
  /// thing to test for.
  #[test]
  fn no_draft_markers_or_bracketed_flags_publish() {
    for (name, src) in DOCUMENTS {
      for marker in ["DRAFT", "[OWNER", "[LAWYER"] {
        assert!(
          !src.contains(marker),
          "docs/legal/{name} still carries {marker}"
        );
      }
    }
  }

  #[test]
  fn every_legal_document_carries_exactly_one_last_updated_token() {
    for (name, src) in DOCUMENTS {
      assert_eq!(
        src.matches("{{LAST_UPDATED}}").count(),
        1,
        "docs/legal/{name} does not carry exactly one date token"
      );
      let rendered = legal_doc::render(src, "2026-01-01");
      assert!(
        !rendered.contains("{{"),
        "docs/legal/{name} renders with an unsubstituted token"
      );
      assert!(rendered.contains("2026-01-01"), "docs/legal/{name}");
    }
  }

  /// Every `pidgeiot.com` link the documents point at, as the path a reader
  /// would be taken to. The path ends where a character no path carries
  /// does: a closing bracket, an emphasis marker, or a sentence's period.
  fn published_paths(src: &str) -> Vec<String> {
    src
      .split("https://pidgeiot.com")
      .skip(1)
      .map(|rest| {
        let path: String = rest
          .chars()
          .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '~'))
          .collect();
        path.trim_end_matches('.').to_string()
      })
      .collect()
  }

  /// A markdown-negotiable route lives in two config files besides the
  /// route table: the worker's path list decides whether the request
  /// reaches the negotiator at all, and the header file advertises the
  /// variant. A route missing from either negotiates as HTML and nothing
  /// says so, which is why this walks every page rather than the four this
  /// module adds.
  #[test]
  fn every_negotiable_route_place_agrees() {
    let meta: serde_json::Value = serde_json::from_str(include_str!("../../page-meta.json"))
      .expect("page-meta.json is not valid JSON");
    let pages = meta["pages"]
      .as_object()
      .expect("page-meta.json has no pages map");
    let wrangler = include_str!("../../wrangler.toml");
    let headers = include_str!("../../public/_headers");

    for route in pages.keys() {
      // A post is covered by one glob rather than a line apiece, so
      // publishing one touches no config. views::stories checks the globs
      // and each post's own entries.
      let post = route.starts_with("/stories/") && route != "/stories/";
      if !post {
        let mut block = String::with_capacity(route.len() * 2 + 58);
        block.push_str(route);
        block.push_str("\n  Link: <");
        block.push_str(route);
        block.push_str("index.md>; rel=\"alternate\"; type=\"text/markdown\"");
        assert!(
          headers.contains(&block),
          "public/_headers does not advertise the markdown variant of {route}"
        );
      }
      if post || route == "/stories/" {
        continue;
      }

      // Both slash forms, so an agent asking without the slash negotiates
      // immediately rather than only after the redirect. The root has one.
      let bare = route.trim_end_matches('/');
      let forms = if bare.is_empty() {
        ["/", "/"]
      } else {
        [bare, route.as_str()]
      };
      for form in forms {
        let mut entry = String::with_capacity(form.len() + 3);
        entry.push('"');
        entry.push_str(form);
        entry.push_str("\",");
        assert!(
          wrangler.contains(&entry),
          "wrangler.toml's run_worker_first lacks {entry} so {route} never reaches the negotiator"
        );
      }
    }
  }

  /// Which route publishes each document, and where the build copies it
  /// from. The order is the one `build-release.sh` uses.
  const PUBLISHED: [(&str, &str); 4] = [
    ("/terms/", "copy_legal terms terms.md"),
    ("/privacy/", "copy_legal privacy privacy.md"),
    ("/dpa/", "copy_legal dpa dpa.md"),
    (
      "/subprocessors/",
      "copy_legal subprocessors subprocessors.md",
    ),
  ];

  /// An agent asked to check what it is agreeing to should be able to fetch
  /// the document rather than scrape the page, so each one ships as its own
  /// markdown variant and llms.txt says where.
  #[test]
  fn every_legal_document_is_published_as_markdown_too() {
    let llms = include_str!("../../public/llms.txt");
    let build = include_str!("../../scripts/build-release.sh");
    for (route, copy) in PUBLISHED {
      let mut variant = String::with_capacity(28 + route.len());
      variant.push_str("https://pidgeiot.com");
      variant.push_str(route);
      variant.push_str("index.md");
      assert!(llms.contains(&variant), "llms.txt does not list {variant}");
      assert!(
        build.contains(copy),
        "scripts/build-release.sh does not copy the variant for {route}"
      );
    }
  }

  /// The Terms incorporate the DPA by pointing at its published address, so
  /// a link that 404s is a defect in the contract rather than in the page.
  #[test]
  fn published_links_point_at_routes_that_exist() {
    for (name, src) in DOCUMENTS {
      for path in published_paths(src) {
        let path = if path.is_empty() {
          "/".to_string()
        } else {
          path
        };
        let route = Route::from_str(&path)
          .unwrap_or_else(|_| panic!("docs/legal/{name} links {path}, which does not parse"));
        assert!(
          !matches!(route, Route::PageNotFound { .. }),
          "docs/legal/{name} links {path}, which is the not-found page"
        );
      }
    }
  }
}
