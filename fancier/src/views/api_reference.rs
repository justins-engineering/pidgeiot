use crate::helpers::api_doc::{self, API_MD, TocSection};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowUp, LdList};
use serde::Deserialize;

// Scoped so it never leaks into the rest of the page — same convention as
// the #infra-mesh SVG and #my-svg architecture diagram: colors reference
// the DaisyUI --color-* custom properties directly, so this tracks the
// app's light/dark theme toggle without needing a Tailwind utility class
// on every element pulldown-cmark generates.
const MARKDOWN_STYLE: &str = r#"<style>
  /* `main` clips horizontal overflow app-wide, and an overflow-x other
     than visible or clip makes that element the scroll box a sticky
     descendant sticks to. It never scrolls -- the document does -- so the
     contents sidebar would simply scroll away. `clip` clips identically
     without creating a scroll box. */
  main:has(#api-reference-doc) { overflow-x: clip; }
  #api-md { color: var(--color-base-content); line-height: 1.7; overflow-wrap: break-word; }
  /* A `capsules::MAX_TELEMETRY_BATCH_READINGS` is wider than a phone's
     text column, and `main` clips rather than scrolls, so an unbreakable
     token would be cut off mid-word instead of wrapping. Code inside a
     `pre` is exempt: that block scrolls on its own. */
  #api-md :not(pre) > code { overflow-wrap: anywhere; }
  #api-md h1 { font-size: 2.25rem; font-weight: 800; margin: 0 0 1rem; }
  #api-md h2 { font-size: 1.5rem; font-weight: 700; margin: 2.5rem 0 1rem; color: var(--color-primary); border-bottom: 1px solid var(--color-base-300); padding-bottom: .4rem; }
  #api-md h3 { font-size: 1.2rem; font-weight: 700; margin: 1.75rem 0 .75rem; }
  #api-md h4 { font-size: 1.05rem; font-weight: 700; margin: 1.5rem 0 .5rem; color: var(--color-secondary); }
  #api-md h5 { font-size: .95rem; font-weight: 700; margin: 1.25rem 0 .5rem; opacity: .85; }
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
  /* Clears the sticky navbar when a hash link or the contents list jumps
     to a heading, whether the browser does it or the script below does. */
  #api-md :is(h1, h2, h3, h4, h5)[id] { scroll-margin-top: 6rem; }
  #api-md .heading-anchor { color: var(--color-primary); text-decoration: none; opacity: .3; margin-left: .45rem; font-weight: 700; }
  #api-md .heading-anchor:hover, #api-md .heading-anchor:focus-visible { opacity: 1; }
  #api-md details.api-surface { margin: 2.75rem 0; }
  #api-md details.api-surface > summary { list-style: none; cursor: pointer; }
  #api-md details.api-surface > summary::-webkit-details-marker { display: none; }
  #api-md details.api-surface > summary > h2 { margin: 0 0 1rem; }
  #api-md details.api-surface > summary > h2::before { content: "\25be"; display: inline-block; width: 1em; opacity: .5; font-size: .85em; }
  #api-md details.api-surface:not([open]) > summary > h2::before { content: "\25b8"; }
</style>"#;

/// Tracks which heading the reader is currently under, and whether the page
/// has scrolled far enough to be worth offering a way back up. Runs from a
/// future rather than the first render: the prerendered HTML carries no
/// script, and hydration adopts the markup this reads.
const TOC_WATCH_JS: &str = r##"
(() => {
  const container = document.getElementById("api-md");
  if (!container) { return; }
  const heads = Array.from(container.querySelectorAll("h2[id], h3[id], h4[id]"));
  if (!heads.length) { return; }

  // A permalink inside a <summary> would otherwise fold away the very
  // surface it points at.
  container.querySelectorAll("summary .heading-anchor").forEach((link) => {
    link.addEventListener("click", (event) => event.stopPropagation());
  });

  const jumpTo = (id) => {
    const target = document.getElementById(id);
    if (!target) { return false; }
    const surface = target.closest("details");
    if (surface) { surface.open = true; }
    // Overrides the theme's smooth scrolling: this document is around
    // 70,000 pixels tall, and animating a jump across it takes seconds.
    target.scrollIntoView({ behavior: "instant", block: "start" });
    return true;
  };

  // The address bar is the source of truth for a shared link: the page was
  // prerendered without one, and the layout moves as hydration lands. Land
  // again once it has settled, unless the reader has scrolled since -- one
  // late reflow above the target is enough to leave it off screen.
  const hash = decodeURIComponent(window.location.hash.replace("#", ""));
  if (hash && jumpTo(hash)) {
    // Reflow above the target moves it out from under the landing, and
    // scroll anchoring moves the scroll position with it, so "has the
    // reader scrolled" has to be read from their own input rather than
    // from the position.
    let readerMoved = false;
    const noteInput = () => { readerMoved = true; };
    ["wheel", "touchstart", "keydown", "mousedown"].forEach((name) => {
      window.addEventListener(name, noteInput, { once: true, passive: true });
    });
    const settle = () => { if (!readerMoved) { jumpTo(hash); } };
    setTimeout(settle, 300);
    setTimeout(settle, 900);
  }

  // The router owns every same-origin anchor click, and a bare fragment
  // resolves to the route the reader is already on: it rewrites the address
  // bar, scrolls nowhere, and then restores the scroll position it saved.
  // Take these clicks before it sees them and do the whole job here, for
  // the contents list, the routes table and the heading permalinks alike.
  // replaceState rather than pushState leaves the router's own history
  // stack alone while still leaving a shareable URL in the address bar.
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey) { return; }
    const link = event.target.closest && event.target.closest('a[href^="#"]');
    if (!link) { return; }
    const id = decodeURIComponent(link.getAttribute("href").slice(1));
    if (!id || !document.getElementById(id)) { return; }
    event.preventDefault();
    event.stopPropagation();
    jumpTo(id);
    history.replaceState(history.state, "", "#" + encodeURIComponent(id));
  }, true);

  let sent = { active: "", scrolled: false };
  const report = () => {
    let active = heads[0].id;
    for (const head of heads) {
      const box = head.getBoundingClientRect();
      // A folded surface reports a zero box for everything inside it.
      if (box.height === 0) { continue; }
      if (box.top <= 140) { active = head.id; } else { break; }
    }
    const scrolled = (window.scrollY || document.documentElement.scrollTop || 0) > 800;
    if (active !== sent.active || scrolled !== sent.scrolled) {
      sent = { active: active, scrolled: scrolled };
      dioxus.send(sent);
    }
  };
  let queued = false;
  const schedule = () => {
    if (queued) { return; }
    queued = true;
    requestAnimationFrame(() => { queued = false; report(); });
  };
  const observer = new IntersectionObserver(schedule, {
    threshold: 0,
    rootMargin: "-140px 0px -55% 0px",
  });
  heads.forEach((head) => observer.observe(head));
  window.addEventListener("scroll", schedule, { passive: true });
  window.addEventListener("resize", schedule, { passive: true });
  report();
})();
"##;

#[derive(Deserialize)]
struct ScrollReport {
  active: String,
  scrolled: bool,
}

fn toc_class(base: &str, is_active: bool) -> String {
  let state = if is_active {
    "text-primary font-semibold"
  } else {
    "text-base-content/70 hover:text-primary"
  };
  format!("{base} {state}")
}

/// The contents tree. Surfaces and resource groups are always listed; a
/// group's routes appear once the reader is inside it, which keeps a
/// 66-route document navigable from one column.
#[component]
fn ApiContents(toc: Vec<TocSection>, active: String) -> Element {
  rsx! {
    ul { class: "space-y-1 text-sm",
      for section in toc.iter() {
        li { key: "{section.slug}",
          a {
            href: "#{section.slug}",
            class: toc_class("block py-1", active == section.slug),
            "{section.text}"
          }
          if !section.groups.is_empty() {
            ul { class: "ml-2 border-l border-base-300 pl-3 space-y-0.5",
              for group in section.groups.iter() {
                li { key: "{group.slug}",
                  a {
                    href: "#{group.slug}",
                    class: toc_class("block py-0.5", active == group.slug),
                    "{group.text}"
                  }
                  if group.slug == active || group.leaves.iter().any(|leaf| leaf.slug == active) {
                    ul { class: "ml-2 border-l border-base-300 pl-3 py-0.5 space-y-0.5",
                      for leaf in group.leaves.iter() {
                        li { key: "{leaf.slug}",
                          a {
                            href: "#{leaf.slug}",
                            class: toc_class("block py-0.5 font-mono text-xs break-all", active == leaf.slug),
                            "{leaf.text}"
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
      }
    }
  }
}

#[component]
pub fn ApiReferencePage() -> Element {
  let doc = use_signal(|| api_doc::render(API_MD));
  let mut active = use_signal(String::new);
  let mut scrolled = use_signal(|| false);

  use_future(move || async move {
    let mut watcher = document::eval(TOC_WATCH_JS);
    while let Ok(report) = watcher.recv::<ScrollReport>().await {
      active.set(report.active);
      scrolled.set(report.scrolled);
    }
  });

  let rendered = doc.read();

  rsx! {
    section { id: "api-reference-doc", class: "py-16 md:py-24",
      div { class: "max-w-7xl mx-auto px-4 md:px-8",
        div { class: "flex gap-10",
          nav {
            id: "api-reference-toc",
            class: "hidden lg:block w-64 shrink-0",
            "aria-label": "API reference contents",
            div { class: "sticky top-20 max-h-[calc(100vh-7rem)] overflow-y-auto pr-2",
              p { class: "text-xs font-bold uppercase tracking-wide text-base-content/50 mb-3",
                "Contents · {rendered.routes.len()} routes"
              }
              ApiContents { toc: rendered.toc.clone(), active: active() }
            }
          }
          div { class: "min-w-0 flex-1",
            p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2",
              "Rendered directly from docs/api.md in the repository"
            }
            details {
              id: "api-reference-contents",
              class: "lg:hidden border border-base-300 rounded-box p-4 mb-8",
              summary { class: "cursor-pointer font-semibold flex items-center gap-2",
                Icon { width: 16, height: 16, icon: LdList }
                "Contents"
              }
              div { class: "pt-3",
                ApiContents { toc: rendered.toc.clone(), active: active() }
              }
            }
            div { id: "api-md", dangerous_inner_html: "{MARKDOWN_STYLE}{rendered.html}" }
          }
        }
      }
      a {
        href: "#api-reference-doc",
        class: if scrolled() {
          "btn btn-sm btn-primary fixed bottom-6 right-6 z-40 shadow-lg"
        } else {
          "hidden"
        },
        "aria-label": "Back to top",
        Icon { width: 14, height: 14, icon: LdArrowUp }
        "Top"
      }
    }
  }
}
