//! The stories index and the post pages under it.
//!
//! A post is two files under `assets/stories/`: the owner's prose as markdown,
//! kept verbatim, and a JSON sidecar carrying the metadata and the figure
//! plan. The headline is read from the prose's own H1, the reading time is
//! counted from its words, and each figure follows the paragraph its anchor
//! phrase is found in, so a revision is a file copy and a rebuild with no
//! code edit. Adding a post is the two files, one line in `STORIES`, and one
//! `page-meta.json` entry; the tests below fail on anything else it needs.
use crate::Route;
use crate::views::PageNotFound;
use dioxus::prelude::*;
use pulldown_cmark::{Options, Parser, html};
use serde::Deserialize;
use std::sync::LazyLock;

/// A post as committed: the prose and the sidecar that lays it out.
pub struct Story {
  /// The segment under `/stories/`, and the media directory under
  /// `public/stories/`.
  pub slug: &'static str,
  md: &'static str,
  sidecar: &'static str,
}

/// One manifest entry, read from `assets/stories/<slug>.md` and `.json`.
// Unused while the manifest is empty.
#[allow(unused_macros)]
macro_rules! story {
  ($slug:literal) => {
    Story {
      slug: $slug,
      md: include_str!(concat!("../../assets/stories/", $slug, ".md")),
      sidecar: include_str!(concat!("../../assets/stories/", $slug, ".json")),
    }
  };
}

/// Every published post. Array order is not publication order: the index
/// sorts by the sidecar's date.
pub const STORIES: &[Story] = &[];

/// The sidecar. `date` is ISO `YYYY-MM-DD`, which is what lets posts sort as
/// strings; `source` is one markdown sentence for the box under the story,
/// and the box is left out without it.
#[derive(Deserialize)]
struct Sidecar {
  date: String,
  blurb: String,
  byline: String,
  #[serde(default)]
  source: Option<String>,
  hero: Hero,
  figures: Vec<Figure>,
}

/// The picture above the story. `src_800` is the same picture at 800px on
/// its long edge: the index card's image and the phone-width candidate.
#[derive(Deserialize)]
struct Hero {
  src: String,
  #[serde(default)]
  src_800: Option<String>,
  width: u32,
  height: u32,
  alt: String,
  caption: String,
}

/// A figure placed after the paragraph its `anchor` phrase is found in.
/// `files` holds one served path, or two for a pair, all `width` by
/// `height`; `poster` is the still a video shows before it plays.
#[derive(Deserialize)]
struct Figure {
  slot: String,
  kind: FigureKind,
  anchor: String,
  files: Vec<String>,
  width: u32,
  height: u32,
  alt: String,
  caption: String,
  #[serde(default)]
  poster: Option<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
enum FigureKind {
  Photo,
  PhotoPair,
  Video,
}

/// A story with its files read: what the two pages render.
struct Post {
  slug: &'static str,
  headline: &'static str,
  body: &'static str,
  meta: Sidecar,
}

impl Post {
  fn read(story: &Story) -> Result<Post, serde_json::Error> {
    let meta = serde_json::from_str(story.sidecar)?;
    let (headline, body) = split_headline(story.md);
    Ok(Post {
      slug: story.slug,
      headline,
      body,
      meta,
    })
  }

  /// `src_800` first so a phone downloads the smaller file; the width
  /// descriptors are what lets the browser pick.
  fn hero_srcset(&self) -> Option<String> {
    let hero = &self.meta.hero;
    let src_800 = hero.src_800.as_deref()?;
    let (w800, _) = size_at_800(hero.width, hero.height);
    let w800 = w800.to_string();
    let width = hero.width.to_string();
    let mut srcset =
      String::with_capacity(src_800.len() + w800.len() + hero.src.len() + width.len() + 6);
    srcset.push_str(src_800);
    srcset.push(' ');
    srcset.push_str(&w800);
    srcset.push_str("w, ");
    srcset.push_str(&hero.src);
    srcset.push(' ');
    srcset.push_str(&width);
    srcset.push('w');
    Some(srcset)
  }
}

/// The manifest with its files read, newest first.
static POSTS: LazyLock<Vec<Post>> = LazyLock::new(|| {
  let mut posts: Vec<Post> = STORIES
    .iter()
    .map(|story| Post::read(story).expect("a story sidecar is not valid"))
    .collect();
  posts.sort_by(|a, b| b.meta.date.cmp(&a.meta.date));
  posts
});

// Served verbatim out of fancier/public/ rather than through asset!(): dx's
// image pipeline re-encodes images it tracks (see the same decision for the
// getting-started poster), and these are photographs whose orientation and
// byte size both matter. They are stored with the rotation baked into the
// pixels and no EXIF at all, so no stage of the pipeline can misread an
// orientation tag.
//
// The box is capped by WIDTH and the height follows from the intrinsic
// ratio the width/height attributes declare. Capping the height instead
// (an `h-auto`/`max-h-*` pair) leaves the width indeterminate until the
// bitmap arrives, which collapses the figure to nothing while it is still
// loading: no space is reserved, the layout shifts when it lands, and a
// `loading=lazy` image sitting in a zero-height box may never come near
// enough to the viewport to be fetched at all. Both were observed before
// the widths below replaced the height caps.
const STORY_PHOTO_CLASS: &str =
  "mx-auto block w-full max-w-md h-auto rounded-xl border border-base-300 shadow-sm";
const STORY_PHOTO_PAIR_CLASS: &str =
  "mx-auto block w-full max-w-72 h-auto rounded-xl border border-base-300 shadow-sm";
const CAPTION_CLASS: &str = "mt-3 text-center text-sm text-base-content/70 leading-relaxed";

// Scoped so it never leaks into the rest of the page, same convention as
// api_reference.rs: colors reference the DaisyUI --color-* custom properties
// directly, so the story tracks the app's own light/dark toggle without a
// Tailwind class on every element pulldown-cmark generates.
const MARKDOWN_STYLE: &str = r#"<style>
  #story-md { color: var(--color-base-content); line-height: 1.75; font-size: 1.0625rem; }
  #story-md p { margin: 1.4rem 0; }
  #story-md h2 { font-size: 1.5rem; font-weight: 700; margin: 2.5rem 0 1rem; letter-spacing: -.015em; }
  #story-md h3 { font-size: 1.2rem; font-weight: 700; margin: 2rem 0 .75rem; }
  #story-md a, #story-source-md a { color: var(--color-secondary); text-decoration: underline; text-underline-offset: 2px; }
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
  #story-source-md p { margin: 0; }
</style>"#;

/// The story's leading `# ` line, and the body that follows it.
///
/// Splitting here rather than letting pulldown-cmark emit the `<h1>` lets the
/// headline sit in the page's own header chrome, above the photo, while
/// still coming from the source file.
fn split_headline(md: &str) -> (&str, &str) {
  let Some(rest) = md.trim_start().strip_prefix("# ") else {
    return ("", md);
  };
  match rest.split_once('\n') {
    Some((headline, body)) => (headline.trim(), body.trim_start()),
    None => (rest.trim(), ""),
  }
}

/// Byte offset just past the paragraph an anchor phrase falls in, or the
/// body's end when nothing matches, which puts that figure after the story
/// rather than dropping it.
fn cut_after(body: &str, anchor: &str) -> usize {
  body
    .find(anchor)
    .and_then(|at| body[at..].find("\n\n").map(|end| at + end))
    .unwrap_or(body.len())
}

/// The figures in the order their anchors fall in the prose, each with the
/// offset its block ends at. Sorting here frees the sidecar from listing
/// them in document order; the sort is stable, so two figures anchored to
/// one paragraph keep the sidecar's order.
fn place_figures<'a>(body: &str, figures: &'a [Figure]) -> Vec<(usize, &'a Figure)> {
  let mut placed: Vec<(usize, &Figure)> = figures
    .iter()
    .map(|figure| (cut_after(body, &figure.anchor), figure))
    .collect();
  placed.sort_by_key(|(cut, _)| *cut);
  placed
}

/// The body cut at ascending offsets into `cuts.len() + 1` blocks, one
/// figure after each block but the last.
fn blocks_between<'a>(body: &'a str, cuts: &[usize]) -> Vec<&'a str> {
  let mut blocks = Vec::with_capacity(cuts.len() + 1);
  let mut start = 0;
  for &cut in cuts {
    blocks.push(&body[start..cut]);
    start = cut;
  }
  blocks.push(&body[start..]);
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

fn month_name(month: u8) -> Option<&'static str> {
  Some(match month {
    1 => "January",
    2 => "February",
    3 => "March",
    4 => "April",
    5 => "May",
    6 => "June",
    7 => "July",
    8 => "August",
    9 => "September",
    10 => "October",
    11 => "November",
    12 => "December",
    _ => return None,
  })
}

/// "September 2026" from an ISO `YYYY-MM-DD` date, or the date itself when
/// it is not one.
fn month_year(date: &str) -> String {
  let name = date
    .get(5..7)
    .and_then(|month| month.parse::<u8>().ok())
    .and_then(month_name);
  let Some((name, year)) = name.zip(date.get(..4)) else {
    return date.to_string();
  };
  let mut out = String::with_capacity(name.len() + 1 + year.len());
  out.push_str(name);
  out.push(' ');
  out.push_str(year);
  out
}

/// A picture's pixel size at 800 on its long edge, the size an `src_800`
/// file is made at.
fn size_at_800(width: u32, height: u32) -> (u32, u32) {
  let long = width.max(height).max(1);
  (width * 800 / long, height * 800 / long)
}

fn figure_element(figure: &Figure) -> Element {
  let file = figure.files.first().map(String::as_str).unwrap_or("");
  match figure.kind {
    FigureKind::Photo => rsx! {
      figure { id: "story-figure-{figure.slot}", class: "my-10",
        img {
          class: STORY_PHOTO_CLASS,
          src: "{file}",
          width: "{figure.width}",
          height: "{figure.height}",
          loading: "lazy",
          decoding: "async",
          alt: "{figure.alt}",
        }
        figcaption { class: CAPTION_CLASS, "{figure.caption}" }
      }
    },
    // One description covers the pair, so it names the group and the
    // images themselves are marked decorative rather than read twice.
    FigureKind::PhotoPair => rsx! {
      figure { id: "story-figure-{figure.slot}", class: "my-10",
        div {
          class: "grid grid-cols-1 sm:grid-cols-2 gap-4",
          role: "group",
          aria_label: "{figure.alt}",
          for file in figure.files.iter() {
            img {
              class: STORY_PHOTO_PAIR_CLASS,
              src: "{file}",
              width: "{figure.width}",
              height: "{figure.height}",
              loading: "lazy",
              decoding: "async",
              alt: "",
            }
          }
        }
        figcaption { class: CAPTION_CLASS, "{figure.caption}" }
      }
    },
    FigureKind::Video => rsx! {
      figure { id: "story-figure-{figure.slot}", class: "my-10",
        video {
          class: STORY_PHOTO_CLASS,
          src: "{file}",
          poster: figure.poster.as_deref(),
          width: "{figure.width}",
          height: "{figure.height}",
          muted: true,
          r#loop: true,
          playsinline: true,
          controls: true,
          preload: "metadata",
          aria_label: "{figure.alt}",
        }
        figcaption { class: CAPTION_CLASS, "{figure.caption}" }
      }
    },
  }
}

#[component]
pub fn StoriesIndex() -> Element {
  rsx! {
    section { id: "stories-header", class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-4xl mx-auto",
        p { class: "text-xs font-bold uppercase tracking-[0.14em] text-secondary mb-3",
          "Field notes"
        }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight text-pretty", "Stories" }
      }
    }

    section { id: "stories-index", class: "px-4 md:px-10 py-14",
      div { class: "max-w-4xl mx-auto",
        if POSTS.is_empty() {
          p { class: "text-lg text-base-content/70",
            "Field stories from the bench and the fleet. The first one is on its way."
          }
        } else {
          ul { class: "flex flex-col gap-8",
            for (i , post) in POSTS.iter().enumerate() {
              li { key: "{post.slug}", {story_card(post, i == 0)} }
            }
          }
        }
      }
    }
  }
}

/// One index card. The first card's picture is above the fold, so only the
/// ones below it load lazily.
fn story_card(post: &Post, first: bool) -> Element {
  let hero = &post.meta.hero;
  let (src, (width, height)) = match hero.src_800.as_deref() {
    Some(src_800) => (src_800, size_at_800(hero.width, hero.height)),
    None => (hero.src.as_str(), (hero.width, hero.height)),
  };
  let when = month_year(&post.meta.date);
  let minutes = reading_minutes(post.body);
  rsx! {
    Link {
      to: Route::StoryPage { slug: post.slug.to_string() },
      class: "card lg:card-side bg-base-100 border border-base-300 shadow-sm hover:border-secondary transition-colors",
      figure { class: "lg:w-64 shrink-0 bg-base-200",
        img {
          class: "w-full h-64 lg:h-full object-cover",
          src: "{src}",
          width: "{width}",
          height: "{height}",
          loading: if first { "eager" } else { "lazy" },
          decoding: "async",
          alt: "{hero.alt}",
        }
      }
      // grow-0: daisyUI stretches every paragraph in a card body, which
      // would float the blurb away from the meta line beside a tall picture.
      div { class: "card-body",
        h2 { class: "card-title text-2xl font-bold text-balance", "{post.headline}" }
        p { class: "grow-0 flex flex-wrap gap-x-4 text-sm text-base-content/70",
          span { "{when}" }
          span { "{minutes} min read" }
        }
        p { class: "grow-0 leading-relaxed", "{post.meta.blurb}" }
        span { class: "link link-secondary text-sm font-semibold mt-2", "Read the story" }
      }
    }
  }
}

#[component]
pub fn StoryPage(slug: String) -> Element {
  let Some(post) = POSTS.iter().find(|post| post.slug == slug) else {
    return rsx! {
      PageNotFound { route: vec!["stories".to_string(), slug] }
    };
  };
  let hero = &post.meta.hero;
  let when = month_year(&post.meta.date);
  let minutes = reading_minutes(post.body);
  let placed = place_figures(post.body, &post.meta.figures);
  let cuts: Vec<usize> = placed.iter().map(|(cut, _)| *cut).collect();
  let mut blocks = blocks_between(post.body, &cuts)
    .into_iter()
    .map(render_markdown);
  // The scoped style rides in the first block, ahead of everything it styles.
  let first = blocks.next().unwrap_or_default();
  let mut opening = String::with_capacity(MARKDOWN_STYLE.len() + first.len());
  opening.push_str(MARKDOWN_STYLE);
  opening.push_str(&first);
  let tail: Vec<(&Figure, String)> = placed
    .iter()
    .map(|(_, figure)| *figure)
    .zip(blocks)
    .collect();
  let source = post.meta.source.as_deref().map(render_markdown);

  rsx! {
    article { class: "w-full flex-1",

      section { id: "story-header", class: "pt-12 md:pt-16",
        div { class: "max-w-3xl mx-auto px-4 md:px-8",
          p { class: "text-xs font-bold uppercase tracking-[0.14em] text-secondary mb-3",
            "Field notes"
          }
          h1 { class: "text-3xl md:text-5xl font-extrabold tracking-tight leading-tight text-balance",
            "{post.headline}"
          }
          div { class: "mt-6 pb-6 border-b border-base-300 flex flex-wrap items-center gap-x-5 gap-y-2 text-sm text-base-content/70",
            span { "{post.meta.byline}" }
            span { "{when}" }
            span { "{minutes} min read" }
          }
        }
      }

      section { id: "story-hero", class: "pt-8",
        div { class: "max-w-3xl mx-auto px-4 md:px-8",
          figure {
            img {
              class: STORY_PHOTO_CLASS,
              src: "{hero.src}",
              srcset: post.hero_srcset(),
              sizes: "(max-width: 767px) calc(100vw - 2rem), 448px",
              width: "{hero.width}",
              height: "{hero.height}",
              alt: "{hero.alt}",
            }
            figcaption { class: CAPTION_CLASS, "{hero.caption}" }
          }
        }
      }

      section { id: "story-body", class: "pb-4",
        div { id: "story-md", class: "max-w-3xl mx-auto px-4 md:px-8",
          div { dangerous_inner_html: "{opening}" }
          for (figure , block) in tail.iter() {
            {figure_element(figure)}
            div { dangerous_inner_html: "{block}" }
          }
        }
      }

      if let Some(source) = source {
        section { id: "story-source", class: "pb-16 md:pb-24",
          div { class: "max-w-3xl mx-auto px-4 md:px-8",
            div { class: "rounded-box border border-base-300 border-s-4 border-s-secondary bg-base-200 p-6",
              span { class: "block text-xs font-bold uppercase tracking-[0.1em] text-secondary mb-2",
                "Source"
              }
              div {
                id: "story-source-md",
                class: "text-base leading-relaxed",
                dangerous_inner_html: "{source}",
              }
            }
          }
        }
      } else {
        div { class: "pb-16 md:pb-24" }
      }
    }
  }
}

// These are the join between a story's files and the code that lays it out.
// Both halves fail silently otherwise: a re-sync that drops the H1 leaves
// the page with an empty headline, and one that reworks a paragraph an
// anchor points at slides its figure to the end of the story with nothing
// to report it.
#[cfg(test)]
mod the_story_files_and_the_layout_agree {
  use super::{
    FigureKind, POSTS, Post, STORIES, Story, blocks_between, month_year, place_figures,
    reading_minutes, size_at_800, split_headline,
  };

  const FIXTURE: Story = Story {
    slug: "fixture",
    md: "# A headline\n\nOne.\n\nTwo, cap raised.\n\nThree, fleet complete.\n\nFour.\n",
    sidecar: r#"{
      "date": "2026-09-01", "blurb": "b", "byline": "y",
      "hero": {"src": "/stories/fixture/hero.jpg", "src_800": "/stories/fixture/hero-800.jpg",
               "width": 1205, "height": 1600, "alt": "a", "caption": "c"},
      "figures": [
        {"slot": "later", "kind": "photo", "anchor": "fleet complete",
         "files": ["/stories/fixture/later.jpg"], "width": 1, "height": 1, "alt": "", "caption": ""},
        {"slot": "earlier", "kind": "photo-pair", "anchor": "cap raised",
         "files": ["/a.jpg", "/b.jpg"], "width": 1, "height": 1, "alt": "", "caption": ""},
        {"slot": "lost", "kind": "video", "anchor": "nowhere in the story",
         "files": ["/v.mp4"], "poster": "/p.jpg", "width": 1, "height": 1, "alt": "", "caption": ""}
      ]
    }"#,
  };

  fn is_iso_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    bytes.len() == 10
      && bytes[4] == b'-'
      && bytes[7] == b'-'
      && bytes
        .iter()
        .enumerate()
        .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
      && month_year(date) != date
  }

  fn served_file_exists(path: &str) -> bool {
    let mut on_disk = String::from(concat!(env!("CARGO_MANIFEST_DIR"), "/public"));
    on_disk.push_str(path);
    std::fs::metadata(on_disk).is_ok()
  }

  #[test]
  fn the_headline_comes_from_the_h1_and_the_sidecar_parses() {
    let post = Post::read(&FIXTURE).expect("fixture sidecar");
    assert_eq!(post.headline, "A headline");
    assert!(post.body.starts_with("One."));
    assert_eq!(post.meta.figures[1].kind, FigureKind::PhotoPair);
    assert_eq!(split_headline("no headline here"), ("", "no headline here"));
  }

  #[test]
  fn figures_follow_their_anchors_in_document_order_whatever_the_sidecar_order() {
    let post = Post::read(&FIXTURE).expect("fixture sidecar");
    let placed = place_figures(post.body, &post.meta.figures);
    let slots: Vec<&str> = placed
      .iter()
      .map(|(_, figure)| figure.slot.as_str())
      .collect();
    assert_eq!(slots, ["earlier", "later", "lost"]);
    let cuts: Vec<usize> = placed.iter().map(|(cut, _)| *cut).collect();
    let blocks = blocks_between(post.body, &cuts);
    assert_eq!(blocks.len(), 4);
    assert!(blocks[0].ends_with("Two, cap raised."));
    assert!(blocks[1].ends_with("Three, fleet complete."));
    assert_eq!(
      blocks[3], "",
      "a figure whose anchor matches nothing goes last"
    );
    assert_eq!(
      blocks.concat(),
      post.body,
      "splitting dropped or duplicated prose"
    );
  }

  #[test]
  fn reading_time_is_at_least_a_minute() {
    assert_eq!(reading_minutes(""), 1);
    assert_eq!(reading_minutes(&"word ".repeat(200)), 1);
    assert_eq!(reading_minutes(&"word ".repeat(201)), 2);
  }

  #[test]
  fn a_date_reads_as_month_and_year() {
    assert_eq!(month_year("2026-09-01"), "September 2026");
    assert_eq!(month_year("2026-13-01"), "2026-13-01");
    assert_eq!(month_year("soon"), "soon");
    assert!(is_iso_date("2026-09-01"));
    assert!(!is_iso_date("2026-9-1"));
  }

  #[test]
  fn the_800_variant_keeps_the_ratio() {
    assert_eq!(size_at_800(1205, 1600), (602, 800));
    assert_eq!(size_at_800(1600, 1200), (800, 600));
  }

  #[test]
  fn every_published_story_is_complete() {
    for story in STORIES {
      let post = Post::read(story)
        .unwrap_or_else(|err| panic!("{}: sidecar does not parse: {err}", story.slug));
      let slug = story.slug;
      assert!(
        !post.headline.is_empty(),
        "{slug}: the story must open with a '# ' headline"
      );
      assert!(
        !post.body.is_empty(),
        "{slug}: the story is a headline and nothing else"
      );
      assert!(
        is_iso_date(&post.meta.date),
        "{slug}: date must be YYYY-MM-DD"
      );
      assert!(
        !post.meta.blurb.is_empty(),
        "{slug}: the index card needs a blurb"
      );
      assert!(!post.meta.byline.is_empty(), "{slug}: the byline is empty");
      let hero = &post.meta.hero;
      assert!(
        served_file_exists(&hero.src),
        "{slug}: hero {} is not under public/",
        hero.src
      );
      if let Some(src_800) = &hero.src_800 {
        assert!(
          served_file_exists(src_800),
          "{slug}: {src_800} is not under public/"
        );
      }
      for figure in &post.meta.figures {
        let slot = &figure.slot;
        assert!(
          post.body.contains(&figure.anchor),
          "{slug}: no paragraph contains figure {slot}'s anchor phrase any more, so it would \
           render at the end of the story instead of beside the passage it illustrates"
        );
        let expected_files = match figure.kind {
          FigureKind::PhotoPair => 2,
          FigureKind::Photo | FigureKind::Video => 1,
        };
        assert_eq!(
          figure.files.len(),
          expected_files,
          "{slug}: figure {slot}'s file count"
        );
        for file in &figure.files {
          assert!(
            served_file_exists(file),
            "{slug}: {file} is not under public/"
          );
        }
        match (figure.kind, &figure.poster) {
          (FigureKind::Video, Some(poster)) => {
            assert!(
              served_file_exists(poster),
              "{slug}: {poster} is not under public/"
            );
          }
          (FigureKind::Video, None) => panic!("{slug}: video figure {slot} has no poster"),
          (_, Some(_)) => panic!("{slug}: only a video figure takes a poster ({slot})"),
          (_, None) => {}
        }
        assert!(
          !figure.alt.is_empty(),
          "{slug}: figure {slot} has no alt text"
        );
      }
      // A figure may close the story, so only the last block may be empty.
      let placed = place_figures(post.body, &post.meta.figures);
      let cuts: Vec<usize> = placed.iter().map(|(cut, _)| *cut).collect();
      let blocks = blocks_between(post.body, &cuts);
      assert!(
        !blocks[0].trim().is_empty(),
        "{slug}: a figure sits before the first paragraph"
      );
      assert!(
        blocks[1..blocks.len().max(2) - 1]
          .iter()
          .all(|block| !block.trim().is_empty()),
        "{slug}: two figures share a paragraph"
      );
    }
  }

  // A markdown-negotiable route lives in three files besides the router.
  // The worker's path list and the header file take one glob for every
  // post, so what a post itself needs is its page-meta entry; the globs
  // are checked so nobody replaces them with per-post lines that the next
  // post then forgets.
  #[test]
  fn every_negotiable_route_place_knows_the_stories() {
    let meta: serde_json::Value = serde_json::from_str(include_str!("../../page-meta.json"))
      .expect("page-meta.json is not valid JSON");
    let pages = meta["pages"]
      .as_object()
      .expect("page-meta.json has no pages map");
    assert!(
      pages.contains_key("/stories/"),
      "page-meta.json lacks the stories index"
    );
    for story in STORIES {
      let mut key = String::with_capacity(10 + story.slug.len());
      key.push_str("/stories/");
      key.push_str(story.slug);
      key.push('/');
      assert!(pages.contains_key(&key), "page-meta.json lacks {key}");
    }

    let wrangler = include_str!("../../wrangler.toml");
    for entry in ["\"/stories\",", "\"/stories/*\",", "\"!/stories/*.*\","] {
      assert!(
        wrangler.contains(entry),
        "wrangler.toml's run_worker_first lacks {entry}"
      );
    }

    let headers = include_str!("../../public/_headers");
    for rule in [
      "/stories/\n  Link: </stories/index.md>; rel=\"alternate\"; type=\"text/markdown\"",
      "/stories/:slug/\n  Link: </stories/:slug/index.md>; rel=\"alternate\"; type=\"text/markdown\"",
    ] {
      assert!(
        headers.contains(rule),
        "public/_headers lacks the rule:\n{rule}"
      );
    }
  }

  #[test]
  fn the_index_runs_newest_first() {
    assert!(
      POSTS
        .windows(2)
        .all(|pair| pair[0].meta.date >= pair[1].meta.date)
    );
  }
}
