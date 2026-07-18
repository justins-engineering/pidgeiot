use dioxus::prelude::*;
use pulldown_cmark::{Options, Parser, html};

const API_MD: &str = include_str!("../../../docs/api.md");

// Scoped so it never leaks into the rest of the page — same convention as
// the #infra-mesh SVG and #my-svg architecture diagram: colors reference
// the DaisyUI --color-* custom properties directly, so this tracks the
// app's light/dark theme toggle without needing a Tailwind utility class
// on every element pulldown-cmark generates.
const MARKDOWN_STYLE: &str = r#"<style>
  #api-md { color: var(--color-base-content); line-height: 1.7; }
  #api-md h1 { font-size: 2.25rem; font-weight: 800; margin: 0 0 1rem; }
  #api-md h2 { font-size: 1.5rem; font-weight: 700; margin: 2.5rem 0 1rem; color: var(--color-primary); border-bottom: 1px solid var(--color-base-300); padding-bottom: .4rem; }
  #api-md h3 { font-size: 1.2rem; font-weight: 700; margin: 1.75rem 0 .75rem; }
  #api-md h4 { font-size: 1.05rem; font-weight: 700; margin: 1.5rem 0 .5rem; color: var(--color-secondary); }
  #api-md p { margin: 1rem 0; }
  #api-md a { color: var(--color-secondary); text-decoration: underline; text-underline-offset: 2px; }
  #api-md ul, #api-md ol { margin: 1rem 0 1rem 1.5rem; }
  #api-md ul { list-style: disc; }
  #api-md ol { list-style: decimal; }
  #api-md li { margin: .35rem 0; }
  #api-md li > p { margin: .25rem 0; }
  #api-md code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--color-base-300); color: var(--color-base-content); padding: .15em .4em; border-radius: .3em; font-size: .9em; }
  #api-md pre { background: var(--color-base-300); padding: 1rem 1.25rem; border-radius: .75rem; overflow-x: auto; margin: 1.25rem 0; }
  #api-md pre code { background: transparent; padding: 0; font-size: .875em; }
  #api-md table { width: 100%; border-collapse: collapse; margin: 1.25rem 0; }
  #api-md .table-scroll { overflow-x: auto; margin: 1.25rem 0; }
  #api-md .table-scroll table { margin: 0; }
  #api-md th, #api-md td { border: 1px solid var(--color-base-300); padding: .5rem .75rem; text-align: left; }
  #api-md th { background: var(--color-base-200); font-weight: 700; white-space: nowrap; }
  #api-md blockquote { border-left: 3px solid var(--color-primary); padding-left: 1rem; margin: 1rem 0; opacity: .8; }
  #api-md hr { border: none; border-top: 1px solid var(--color-base-300); margin: 2rem 0; }
  #api-md strong { font-weight: 700; }
</style>"#;

fn render_markdown(src: &str) -> String {
  let mut options = Options::empty();
  options.insert(Options::ENABLE_TABLES);
  options.insert(Options::ENABLE_STRIKETHROUGH);
  options.insert(Options::ENABLE_FOOTNOTES);
  let parser = Parser::new_ext(src, options);
  let mut body = String::new();
  html::push_html(&mut body, parser);
  // Wrap every generated <table> in a horizontally-scrollable container so
  // the wide auth-model/status-code tables never force the page itself to
  // scroll sideways.
  let body = body.replace("<table>", r#"<div class="table-scroll"><table>"#);
  let body = body.replace("</table>", "</table></div>");
  format!("{MARKDOWN_STYLE}{body}")
}

#[component]
pub fn ApiReferencePage() -> Element {
  let rendered = use_memo(|| render_markdown(API_MD));

  rsx! {
    section { class: "py-16 md:py-24",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2",
          "Rendered directly from docs/api.md in the repository"
        }
        div { id: "api-md", dangerous_inner_html: "{rendered}" }
      }
    }
  }
}
