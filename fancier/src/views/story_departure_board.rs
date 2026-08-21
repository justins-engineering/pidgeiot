use dioxus::prelude::*;
use pulldown_cmark::{Options, Parser, html};

// The prose is the owner's, and he is still revising it. This file is a
// verbatim copy of ~/pidgeiot-business/drafts/departure-story-board-focused.md
// with that draft's leading criteria comment removed, and it is the only
// thing a revision has to touch: the headline is read out of the copy's own
// H1, the reading time is counted from its words, and the photos below are
// placed by anchor phrases rather than by paragraph number. Re-syncing is a
// file copy and a rebuild, with no code edit.
const STORY_MD: &str = include_str!("../../assets/stories/departure-board.md");

// A distinctive phrase from the paragraph each figure follows. Anchoring to
// the prose rather than to a paragraph index means paragraphs can be added,
// cut, or reordered without a photo silently drifting away from the passage
// it illustrates, and a phrase that stops matching sends its figure to the
// end of the story instead of dropping it. The unit tests below fail on a
// re-sync that leaves an anchor behind.
const PLACARD_ANCHOR: &str = "I raised the cap to 8";
const MILL_LOCUST_ANCHOR: &str = "By August 2025 the fleet was complete";

// Served verbatim out of fancier/public/ rather than through asset!(): dx's
// image pipeline re-encodes images it tracks (see the same decision for the
// getting-started poster), and these are photographs whose orientation and
// byte size both matter. They are stored with the rotation baked into the
// pixels and no EXIF at all, so no stage of the pipeline can misread an
// orientation tag.
//
// The hero is a stand-in until the owner shoots a current one. Replacing it
// is a drop-in at these two paths, at 1600px and 800px on the long edge; no
// markup below needs to change beyond the caption and the intrinsic size
// attributes.
const HERO_1600: &str = "/stories/departure-board/hero.jpg";
const HERO_800: &str = "/stories/departure-board/hero-800.jpg";

// Every photo here is a tall portrait, so the box is capped by WIDTH and the
// height follows from the intrinsic ratio. Capping the height instead (an
// `h-auto`/`max-h-*` pair) leaves the width indeterminate until the bitmap
// arrives, which collapses the figure to nothing while it is still loading:
// no space is reserved, the layout shifts when it lands, and a `loading=lazy`
// image sitting in a zero-height box may never come near enough to the
// viewport to be fetched at all. Both were observed on this page before the
// widths below replaced the height caps.
const STORY_PHOTO_CLASS: &str =
  "mx-auto block w-full max-w-md h-auto rounded-xl border border-base-300 shadow-sm";
const STORY_PHOTO_PAIR_CLASS: &str =
  "mx-auto block w-full max-w-72 h-auto rounded-xl border border-base-300 shadow-sm";

// Scoped so it never leaks into the rest of the page, same convention as
// api_reference.rs: colors reference the DaisyUI --color-* custom properties
// directly, so the story tracks the app's own light/dark toggle without a
// Tailwind class on every element pulldown-cmark generates.
const MARKDOWN_STYLE: &str = r#"<style>
  #story-md { color: var(--color-base-content); line-height: 1.75; font-size: 1.0625rem; }
  #story-md p { margin: 1.4rem 0; }
  #story-md h2 { font-size: 1.5rem; font-weight: 700; margin: 2.5rem 0 1rem; letter-spacing: -.015em; }
  #story-md h3 { font-size: 1.2rem; font-weight: 700; margin: 2rem 0 .75rem; }
  #story-md a { color: var(--color-secondary); text-decoration: underline; text-underline-offset: 2px; }
  #story-md ul, #story-md ol { margin: 1.4rem 0 1.4rem 1.5rem; }
  #story-md ul { list-style: disc; }
  #story-md ol { list-style: decimal; }
  #story-md li { margin: .4rem 0; }
  #story-md blockquote { border-inline-start: 3px solid var(--color-primary); padding-inline-start: 1rem; margin: 1.5rem 0; opacity: .85; }
  #story-md code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--color-base-300); padding: .15em .4em; border-radius: .3em; font-size: .9em; }
  #story-md pre { background: var(--color-base-300); padding: 1rem 1.25rem; border-radius: .75rem; overflow-x: auto; margin: 1.5rem 0; }
  #story-md pre code { background: transparent; padding: 0; font-size: .875em; }
  #story-md hr { border: none; border-top: 1px solid var(--color-base-300); margin: 2.5rem 0; }
  #story-md strong { font-weight: 700; }
</style>"#;

/// The story's leading `# ` line, and the body that follows it.
///
/// Splitting here rather than letting pulldown-cmark emit the `<h1>` lets the
/// headline sit in the page's own hero chrome, above the photo, while still
/// coming from the source file.
fn split_headline(md: &str) -> (&str, &str) {
  let Some(rest) = md.trim_start().strip_prefix("# ") else {
    return ("", md);
  };
  match rest.split_once('\n') {
    Some((headline, body)) => (headline.trim(), body.trim_start()),
    None => (rest.trim(), ""),
  }
}

/// The body cut into the blocks that render between the page's photos: one
/// block per anchor, ending with the paragraph that anchor falls in, plus
/// whatever remains. Always returns `anchors.len() + 1` blocks, so callers can
/// lay figures out positionally; an anchor that matches nothing yields the
/// rest of the story and leaves the blocks after it empty, which puts its
/// figure at the end rather than losing it.
fn blocks_between_photos<'a>(body: &'a str, anchors: &[&str]) -> Vec<&'a str> {
  let mut blocks = Vec::with_capacity(anchors.len() + 1);
  let mut rest = body;
  for anchor in anchors {
    let cut = rest
      .find(anchor)
      .and_then(|at| rest[at..].find("\n\n").map(|end| at + end))
      .unwrap_or(rest.len());
    let (block, tail) = rest.split_at(cut);
    blocks.push(block);
    rest = tail;
  }
  blocks.push(rest);
  blocks
}

/// Reading time in whole minutes at 200 words per minute, counted from the
/// story itself so a revision cannot leave a stale figure in the byline.
fn reading_minutes(body: &str) -> usize {
  body.split_whitespace().count().div_ceil(200).max(1)
}

fn render_markdown(src: &str) -> String {
  let mut options = Options::empty();
  options.insert(Options::ENABLE_TABLES);
  options.insert(Options::ENABLE_STRIKETHROUGH);
  let parser = Parser::new_ext(src, options);
  let mut body = String::new();
  html::push_html(&mut body, parser);
  body
}

#[component]
pub fn DepartureBoardStory() -> Element {
  let (headline, body) = split_headline(STORY_MD);
  let minutes = reading_minutes(body);
  let mut blocks = blocks_between_photos(body, &[PLACARD_ANCHOR, MILL_LOCUST_ANCHOR])
    .into_iter()
    .map(render_markdown);
  let opening = format!("{MARKDOWN_STYLE}{}", blocks.next().unwrap_or_default());
  let after_placards = blocks.next().unwrap_or_default();
  let closing = blocks.next().unwrap_or_default();
  let hero_srcset = format!("{HERO_800} 669w, {HERO_1600} 1339w");

  rsx! {
    article { class: "w-full flex-1",

      section { id: "story-header", class: "pt-12 md:pt-16",
        div { class: "max-w-3xl mx-auto px-4 md:px-8",
          p { class: "text-xs font-bold uppercase tracking-[0.14em] text-secondary mb-3",
            "Field notes"
          }
          h1 { class: "text-3xl md:text-5xl font-extrabold tracking-tight leading-tight text-balance",
            "{headline}"
          }
          div { class: "mt-6 pb-6 border-b border-base-300 flex flex-wrap items-center gap-x-5 gap-y-2 text-sm text-base-content/70",
            span { "Justin, Justin's Engineering Services" }
            span { "August 2026" }
            span { "{minutes} min read" }
          }
        }
      }

      section { id: "story-hero", class: "pt-8",
        div { class: "max-w-3xl mx-auto px-4 md:px-8",
          figure {
            img {
              class: STORY_PHOTO_CLASS,
              src: HERO_1600,
              srcset: hero_srcset,
              sizes: "(max-width: 767px) calc(100vw - 2rem), 448px",
              width: "1339",
              height: "1600",
              alt: "A solar-powered departure sign on a pole against a cloudy sky, its six rows each showing a countdown in minutes beside a route number, with a solar panel angled above it and fall foliage behind.",
            }
            figcaption { class: "mt-3 text-center text-sm text-base-content/70 leading-relaxed",
              "Holyoke Public Library, stop 414, October 2024. Six departure times, the 55W solar panel that was never replaced, and a route placard held on with tape."
            }
          }
        }
      }

      section { id: "story-body", class: "pb-4",
        div { id: "story-md", class: "max-w-3xl mx-auto px-4 md:px-8",
          div { dangerous_inner_html: "{opening}" }

          figure { class: "my-10",
            div { class: "grid grid-cols-1 sm:grid-cols-2 gap-4",
              img {
                class: STORY_PHOTO_PAIR_CLASS,
                src: "/stories/departure-board/placard-x90.jpg",
                width: "652",
                height: "1000",
                loading: "lazy",
                decoding: "async",
                alt: "Close view of the same sign earlier that morning. The bottom row's route badge is a printed placard reading X90.",
              }
              img {
                class: STORY_PHOTO_PAIR_CLASS,
                src: "/stories/departure-board/placard-r22.jpg",
                width: "652",
                height: "1000",
                loading: "lazy",
                decoding: "async",
                alt: "The same bottom row 24 minutes later. The printed X90 badge is covered by white tape with R22 written on it by hand.",
              }
            }
            figcaption { class: "mt-3 text-center text-sm text-base-content/70 leading-relaxed",
              "Stop 414 on one October morning, 24 minutes apart. The bottom row's printed X90 placard, then the hand-lettered R22 taped over it. Route changes arrive faster than printed placards do."
            }
          }

          div { dangerous_inner_html: "{after_placards}" }

          figure { class: "my-10",
            img {
              class: STORY_PHOTO_CLASS,
              src: "/stories/departure-board/mill-locust.jpg",
              width: "896",
              height: "1000",
              loading: "lazy",
              decoding: "async",
              alt: "A three-row departure sign against a clear blue sky, showing two green countdowns for routes G1 and G2 and one white countdown for route X92, with its solar panel angled above and a residential street below.",
            }
            figcaption { class: "mt-3 text-center text-sm text-base-content/70 leading-relaxed",
              "Mill and Locust in Springfield, stop 1670, November 2024. The three-position sign, the second of the two models in the fleet. The per-route digit colors here are the original scheme."
            }
          }

          div { dangerous_inner_html: "{closing}" }
        }
      }

      section { id: "story-source", class: "pb-16 md:pb-24",
        div { class: "max-w-3xl mx-auto px-4 md:px-8",
          div { class: "rounded-box border border-base-300 border-s-4 border-s-secondary bg-base-200 p-6",
            span { class: "block text-xs font-bold uppercase tracking-[0.1em] text-secondary mb-2",
              "Source"
            }
            p { class: "text-base leading-relaxed",
              "The firmware and the platform in this story are both open source: "
              a {
                class: "link link-secondary",
                href: "https://github.com/justins-engineering/pigeon",
                "pigeon"
              }
              ", the Zephyr RTOS module the signs run, and "
              a {
                class: "link link-secondary",
                href: "https://github.com/justins-engineering/pidgeiot",
                "the platform they report to"
              }
              "."
            }
          }
        }
      }
    }
  }
}

// These are the join between the published copy of the story and the code
// that lays it out. Both halves fail silently otherwise: a re-sync that
// drops the H1 leaves the page with an empty headline, and one that reworks
// a paragraph an anchor points at slides both photos to the end of the
// story with nothing to report it.
#[cfg(test)]
mod the_story_file_and_the_layout_agree {
  use super::{
    MILL_LOCUST_ANCHOR, PLACARD_ANCHOR, STORY_MD, blocks_between_photos, reading_minutes,
    split_headline,
  };

  #[test]
  fn the_story_leads_with_the_headline_the_page_shows() {
    let (headline, body) = split_headline(STORY_MD);
    assert!(
      !headline.is_empty(),
      "the story file must open with a '# ' headline; the page has no other source for one"
    );
    assert!(
      !body.is_empty(),
      "the story file is a headline and nothing else"
    );
  }

  #[test]
  fn both_photo_anchors_still_match_a_paragraph() {
    for anchor in [PLACARD_ANCHOR, MILL_LOCUST_ANCHOR] {
      assert!(
        STORY_MD.contains(anchor),
        "no paragraph contains this photo's anchor phrase any more, so the photo would \
         render at the end of the story instead of beside the passage it illustrates: {anchor}"
      );
    }
  }

  #[test]
  fn the_photos_split_the_story_in_document_order() {
    let (_, body) = split_headline(STORY_MD);
    let blocks = blocks_between_photos(body, &[PLACARD_ANCHOR, MILL_LOCUST_ANCHOR]);
    assert_eq!(blocks.len(), 3);
    assert!(
      blocks.iter().all(|block| !block.trim().is_empty()),
      "a photo landed at the start or end of the story rather than between paragraphs, \
       which means an anchor is out of document order"
    );
    assert_eq!(
      blocks.concat(),
      body,
      "splitting the story dropped or duplicated prose"
    );
  }

  #[test]
  fn a_missing_anchor_puts_its_photo_at_the_end_rather_than_dropping_prose() {
    let body = "one\n\ntwo\n\nthree";
    let blocks = blocks_between_photos(body, &["two", "nowhere in the story"]);
    assert_eq!(blocks, vec!["one\n\ntwo", "\n\nthree", ""]);
  }

  #[test]
  fn reading_time_is_at_least_a_minute() {
    assert_eq!(reading_minutes(""), 1);
    assert_eq!(reading_minutes(&"word ".repeat(200)), 1);
    assert_eq!(reading_minutes(&"word ".repeat(201)), 2);
  }
}
