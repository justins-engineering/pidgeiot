// Browser-tab titles, read from the same `page-meta.json` that
// scripts/build-release.sh bakes into each prerendered page. Sharing the file
// is the point: a prerendered page's <title> and the title a client-side
// navigation sets are otherwise written in two languages, in two files, and
// nothing would catch them disagreeing except noticing the tab.
//
// Only titles are read here. The descriptions and canonicals in that file are
// crawler-facing, and crawlers read the prerendered HTML the build script
// writes -- shipping them in the wasm bundle too would pay for them on every
// page load and change nothing.
use crate::Route;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct PageMeta {
  pages: HashMap<String, PageEntry>,
}

impl PageMeta {
  /// The name for routes with no SEO title of their own. The landing page's
  /// title is already the brand line, so it is reused rather than written
  /// down a second time and left to drift.
  fn brand_title(&self) -> &str {
    self
      .pages
      .get("/")
      .map(|page| page.title.as_str())
      // Dioxus.toml's shell title, and unreachable while the tests below
      // hold: they require an entry for the landing page.
      .unwrap_or("PidgeIoT")
  }
}

/// `description` and the file's other keys are deliberately absent: serde
/// ignores what it isn't asked for, so the build script can carry fields this
/// crate has no reason to compile in.
#[derive(Deserialize)]
struct PageEntry {
  title: String,
}

static META: LazyLock<PageMeta> = LazyLock::new(|| {
  serde_json::from_str(include_str!("../../page-meta.json"))
    .expect("page-meta.json is not valid page metadata")
});

/// A route's browser-tab title.
///
/// Public pages resolve to the exact SEO title their prerendered HTML
/// carries, keyed on the same path string the build script uses, so
/// navigating to a page and loading it cold can never show different titles.
/// Auth-gated and Kratos-flow routes are noindex and have no SEO title, so
/// they get a short name here instead -- worth having, since these are the
/// pages a user is most likely to have several of open at once.
pub fn page_title(route: &Route) -> String {
  let app_page = match route {
    Route::Dashboard {} => Some("Dashboard"),
    Route::Flocks {} => Some("Flocks"),
    Route::Pigeons { .. } => Some("Pigeons"),
    Route::PigeonView { .. } => Some("Pigeon"),
    Route::Orgs {} => Some("Organizations"),
    Route::OrgView { .. } => Some("Organization"),
    Route::SessionInfo {} => Some("Session"),
    Route::SettingsFlow { .. } => Some("Account settings"),
    Route::InviteAccept { .. } => Some("Accept invite"),
    Route::LoginFlow { .. } => Some("Sign in"),
    Route::RegisterFlow { .. } => Some("Create an account"),
    Route::VerificationFlow { .. } => Some("Verify your email"),
    Route::RecoveryFlow { .. } => Some("Recover your account"),
    Route::SetSessionCookie { .. } => Some("Signing in"),
    Route::ServerError { .. } => Some("Something went wrong"),
    Route::Unauthorized {} => Some("Not authorized"),
    Route::PageNotFound { .. } => Some("Page not found"),
    _ => None,
  };

  match app_page {
    Some(name) => format!("{name} | PidgeIoT"),
    None => META
      .pages
      .get(&route.to_string())
      .map(|page| page.title.clone())
      .unwrap_or_else(|| META.brand_title().to_string()),
  }
}

// These are the join between the JSON and the router. Both halves are easy to
// break silently: a page renamed in the router keeps its old key in the JSON
// and quietly serves the brand title, and a page added to the router without a
// JSON entry does the same.
#[cfg(test)]
mod page_meta_matches_the_router {
  use super::{META, page_title};
  use crate::Route;
  use std::str::FromStr;

  /// Every public page, listed so that adding one to the router without a
  /// title is a test failure rather than a silent fallback to the brand name.
  const PUBLIC_ROUTES: [Route; 15] = [
    Route::Index {},
    Route::FeaturesPage {},
    Route::HowItWorksPage {},
    Route::UseCasesPage {},
    Route::PricingPage {},
    Route::DocumentationPage {},
    Route::ApiReferencePage {},
    Route::Architecture {},
    Route::GettingStartedPage {},
    Route::DemoPage {},
    Route::OpenSourcePage {},
    Route::AboutUs {},
    Route::PrivacyPage {},
    Route::TermsPage {},
    Route::DepartureBoardStory {},
  ];

  #[test]
  fn every_json_key_is_a_route_that_renders_to_that_exact_path() {
    for path in META.pages.keys() {
      let route = Route::from_str(path)
        .unwrap_or_else(|_| panic!("page-meta.json has {path}, which the router does not parse"));
      assert_eq!(
        &route.to_string(),
        path,
        "page-meta.json keys {path}, but the router writes that route as {route}"
      );
    }
  }

  #[test]
  fn every_public_route_has_its_own_title() {
    for route in PUBLIC_ROUTES {
      assert!(
        META.pages.contains_key(&route.to_string()),
        "{route} has no page-meta.json entry, so its tab would read as the bare brand name"
      );
    }
  }

  #[test]
  fn public_and_app_routes_cover_the_same_ground_as_the_json() {
    assert_eq!(
      META.pages.len(),
      PUBLIC_ROUTES.len(),
      "page-meta.json and the public route list disagree on how many public pages there are"
    );
  }

  #[test]
  fn app_routes_are_named_rather_than_falling_back() {
    let title = page_title(&Route::Dashboard {});
    assert_eq!(title, "Dashboard | PidgeIoT");
    assert_ne!(page_title(&Route::Unauthorized {}), META.brand_title());
  }

  #[test]
  fn a_public_route_resolves_to_its_seo_title() {
    assert_eq!(
      page_title(&Route::PricingPage {}),
      "Pricing — Free During Early Access | PidgeIoT"
    );
  }
}
