use crate::components::SetSessionCookie;
use crate::config::{KRATOS_BROWSER_URL, SESSION_COOKIE_NAME};
use crate::helpers::session_cookie_valid;
use crate::models::AuthState;
use capsules::{AlertDefinition, Flock, Pigeon};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use ory_kratos_client_wasm::apis::configuration::Configuration;
use std::collections::HashMap;
use unic_langid::langid;
use uuid::Uuid;
use views::{
  AboutUs, ApiReferencePage, Architecture, Dashboard, DemoPage, DocumentationPage, FeaturesPage,
  Flocks, GettingStartedPage, HowItWorksPage, Index, InviteAccept, LoginFlow, OpenSourcePage,
  OrgView, Orgs, PageNotFound, PigeonView, Pigeons, PricingPage, PrivacyPage, RecoveryFlow,
  RegisterFlow, ServerError, SessionInfo, SettingsFlow, TermsPage, Unauthorized, UseCasesPage,
  VerificationFlow, Wrapper,
};

pub mod api;
mod components;
mod config;
mod helpers;
mod local_storage;
mod models;
mod partials;
mod views;

#[derive(Clone, Copy)]
struct Session {
  state: Signal<AuthState>,
  // True only when an established session ended on its own -- expired, or
  // revoked server-side -- as opposed to a deliberate logout or a visitor
  // who was never signed in. The login view reads it to explain why it is
  // being shown instead of the page that was asked for.
  signed_out: Signal<bool>,
}

trait Create {
  fn create() -> Configuration;
}

impl Create for Configuration {
  fn create() -> Configuration {
    Configuration {
      base_path: KRATOS_BROWSER_URL.to_owned(),
      user_agent: None,
      basic_auth: None,
      oauth_access_token: None,
      bearer_access_token: None,
      api_key: None,
    }
  }
}

#[derive(Routable, Clone, PartialEq)]
#[rustfmt::skip]
enum Route {
#[layout(Wrapper)]
  #[layout(AuthGuard)]
    #[route("/dashboard")]
    Dashboard {},
    #[route("/flocks")]
    Flocks {},
    #[route("/flocks/:flock_id/pigeons")]
    Pigeons { flock_id: Uuid },
    #[route("/flocks/:flock_id/pigeons/:pigeon_id")]
    PigeonView { flock_id: Uuid, pigeon_id: String },
    #[route("/orgs")]
    Orgs {},
    #[route("/orgs/:org_id")]
    OrgView { org_id: Uuid },
    #[route("/session")]
    SessionInfo {},
    #[route("/settings?:flow")]
    SettingsFlow { flow: Option<String> },
  #[end_layout]
  // Public pages use trailing-slash paths so generated <a href>s are already
  // wrangler's canonical form (it 307s /features -> /features/) -- saves
  // crawlers a redirect hop. The router still accepts both forms (see the
  // public_route_trailing_slash tests below); only Display/href changes.
  // Auth-gated and Kratos-flow routes stay non-slash: noindex, and the flow
  // routes carry query-param props with SSG-hydration sensitivity (see
  // helpers::url_query_param).
  #[route("/")]
  Index {},
  #[route("/about/")]
  AboutUs {},
  #[route("/architecture/")]
  Architecture {},
  #[route("/features/")]
  FeaturesPage {},
  #[route("/how-it-works/")]
  HowItWorksPage {},
  #[route("/use-cases/")]
  UseCasesPage {},
  #[route("/documentation/")]
  DocumentationPage {},
  #[route("/getting-started/")]
  GettingStartedPage {},
  #[route("/pricing/")]
  PricingPage {},
  #[route("/demo/")]
  DemoPage {},
  #[route("/api-reference/")]
  ApiReferencePage {},
  #[route("/privacy/")]
  PrivacyPage {},
  #[route("/open-source/")]
  OpenSourcePage {},
  #[route("/terms/")]
  TermsPage {},
  // Org invite landing page -- public (NOT AuthGuard'd, see
  // views/invite.rs's module comment) and non-trailing-slash like the
  // Kratos flow routes, since it carries a query-param prop with
  // SSG-hydration sensitivity (read via url_query_param).
  #[route("/invite?:token")]
  InviteAccept { token: Option<String> },
  #[route("/login?:flow")]
  LoginFlow { flow: Option<String> },
  #[route("/registration?:flow")]
  RegisterFlow { flow: Option<String> },
  #[route("/verification?:flow")]
  VerificationFlow { flow: Option<String> },
  #[route("/recovery?:flow")]
  RecoveryFlow { flow: Option<String> },
  #[route("/session/local?:state")]
  SetSessionCookie { state: bool },
  #[route("/error?:id")]
  ServerError { id: String },
  #[route("/unauthorized")]
  Unauthorized {},
  #[route("/:..route")]
  PageNotFound { route: Vec<String> },
}

// `dx build --ssg` calls this endpoint (must be named exactly
// "static_routes") to discover which routes to prerender. Dioxus router's
// `Route::static_routes()` already filters out any route with a dynamic
// (`:flock_id`) or catch-all (`:..route`) segment, so this returns every
// public marketing page plus the small number of statically-routable
// AuthGuard'd pages (`/dashboard`, `/flocks`, `/session`, `/settings`) --
// those prerender AuthGuard's logged-out redirect state, not real content
// (harmless: no private data leaks into the prerendered HTML).
#[server(endpoint = "static_routes", output = server_fn::codec::Json)]
async fn static_routes() -> Result<Vec<String>, ServerFnError> {
  Ok(
    Route::static_routes()
      .iter()
      .map(ToString::to_string)
      .collect(),
  )
}

#[component]
fn AuthGuard() -> Element {
  let session = use_context::<Session>();
  // Hoisted above the match: a hook called from only one arm would shift
  // this scope's hook indices as the auth state resolves.
  let nav = use_navigator();

  match (session.state)() {
    AuthState::Authenticated => {
      rsx! {
        Outlet::<Route> {}
      }
    }
    AuthState::Unauthenticated => {
      if (session.signed_out)() {
        // A session that lapsed mid-visit lands on the login form, which
        // says so, carrying the page it interrupted so signing back in
        // resumes there. The bare 401 page is the honest answer only for
        // someone who was never signed in at all -- shown after an
        // expiry it reads as a bug rather than as a session ending.
        crate::helpers::stash_return_to();
        nav.replace(Route::LoginFlow { flow: None });
      } else {
        nav.replace(Route::Unauthorized {});
      }
      rsx! {}
    }
    AuthState::Pending => {
      rsx! {
        div { "Verifying session..." }
      }
    }
  }
}

#[derive(Clone, Copy, Debug)]
struct LocalSession {
  flocks: Signal<HashMap<Uuid, Flock>>,
  pigeons: Signal<HashMap<String, Pigeon>>,
  // Keyed by alert id, same shared/additive cache convention as
  // `flocks`/`pigeons` above: `api::alerts::list_pigeon`/`list_flock` extend
  // this map, never prune it. `AlertDefinition::scope` already carries
  // either the owning pigeon_id or flock_id, so callers filter this map by
  // `scope` locally (see `components::AlertsPanel`) instead of needing a
  // second, scope-keyed cache.
  alerts: Signal<HashMap<Uuid, AlertDefinition>>,
}

#[component]
pub fn App() -> Element {
  use_init_i18n(|| {
    I18nConfig::new(langid!("en-US")).with_locale(Locale::new_static(
      langid!("en-US"),
      include_str!("../locales/en-US.ftl"),
    ))
  });

  use_effect(crate::helpers::set_lang);

  // 1. Initialize context with the Pending state
  let mut session = use_context_provider(|| Session {
    state: Signal::new(AuthState::Pending),
    signed_out: Signal::new(false),
  });

  // 2. Fire the async check. This future runs automatically on mount.
  use_future(move || async move {
    let is_valid = session_cookie_valid().await;

    session.state.set(if is_valid {
      AuthState::Authenticated
    } else {
      AuthState::Unauthenticated
    });
  });

  let _local_session = use_context_provider(|| LocalSession {
    flocks: Signal::new(HashMap::new()),
    pigeons: Signal::new(HashMap::new()),
    alerts: Signal::new(HashMap::new()),
  });

  use_resource(move || async move {
    if (session.state)() == AuthState::Authenticated {
      api::flocks::list().await;
    }
  });

  // Without this an idle tab keeps its signed-in chrome until the user
  // clicks something, which is the first request that gets a 401. Reading
  // `session.state` first is what re-arms the watch on each sign-in and
  // drops it on each sign-out -- see `watch_session_expiry` for why it
  // costs no network and what it deliberately does not detect.
  use_resource(move || async move {
    if (session.state)() == AuthState::Authenticated {
      crate::helpers::watch_session_expiry().await;
    }
  });

  rsx! {
    document::Meta {
      name: "description",
      content: "Open-source IoT device management: config push, OTA updates, telemetry graphs, GPS tracks, and email alerts for ESP32 and nRF91 fleets. Free while in beta.",
    }
    // Release builds get main.css from a static <link> in index.html instead
    // (Dioxus.toml's [web.resource], populated by scripts/build-release.sh) —
    // it loads in parallel with app.js/wasm rather than only after this
    // component mounts post-WASM-boot, which causes a FOUC/layout shift. Dev
    // keeps this runtime injection since `[web.resource.dev]` is
    // deliberately left empty — see that config's comment for why.
    if cfg!(debug_assertions) {
      document::Link { rel: "stylesheet", href: asset!("/assets/styling/main.css") }
    }
    document::Link {
      rel: "icon",
      href: asset!("/assets/images/icon-light.ico"),
      sizes: "32x32",
      media: "(prefers-color-scheme: light)",
    }
    document::Link {
      rel: "icon",
      href: asset!("/assets/images/icon-dark.ico"),
      sizes: "32x32",
      media: "(prefers-color-scheme: dark)",
    }
    document::Link {
      rel: "icon",
      r#type: "image/svg+xml",
      href: asset!("/assets/images/icon-light.svg"),
    }
    document::Link {
      rel: "icon",
      r#type: "image/svg+xml",
      href: asset!("/assets/images/icon-light.svg"),
      media: "(prefers-color-scheme: light)",
    }
    document::Link {
      rel: "icon",
      r#type: "image/svg+xml",
      href: asset!("/assets/images/icon-dark.svg"),
      media: "(prefers-color-scheme: dark)",
    }
    Router::<Route> {}
  }
}

// These prove the ROUTER parses `?flow=` correctly — including the
// trailing-slash form that wrangler's `html_handling` 307 produces for every
// prerendered route (`/registration?flow=X` → `/registration/?flow=X`).
// What drops the flow id on a real page load is SSG hydration restoring the
// prerendered `flow: None` route instead of re-parsing `window.location`;
// see helpers::url_query_param for the fix.
#[cfg(test)]
mod route_query_param_parsing {
  use super::Route;
  use std::str::FromStr;

  #[test]
  fn flow_param_no_trailing_slash() {
    let r = Route::from_str("/registration?flow=abc123").unwrap();
    assert!(
      matches!(r, Route::RegisterFlow { flow: Some(ref f) } if f == "abc123"),
      "got {}",
      r
    );
  }

  #[test]
  fn flow_param_with_trailing_slash() {
    let r = Route::from_str("/registration/?flow=abc123").unwrap();
    assert!(
      matches!(r, Route::RegisterFlow { flow: Some(ref f) } if f == "abc123"),
      "got {}",
      r
    );
  }

  #[test]
  fn session_local_state_param() {
    let r = Route::from_str("/session/local?state=true").unwrap();
    assert!(
      matches!(r, Route::SetSessionCookie { state: true }),
      "got {}",
      r
    );
  }
}

// Public routes are annotated with trailing-slash paths so generated hrefs
// are wrangler's canonical form directly (no 307 hop for crawlers). These
// prove the router still accepts BOTH forms — a deep load of /features (no
// slash, e.g. a stale external link) and /features/ must resolve to the
// same component, never fall through to the PageNotFound catch-all — and
// that Display emits the trailing-slash canonical.
#[cfg(test)]
mod public_route_trailing_slash {
  use super::Route;
  use std::str::FromStr;

  macro_rules! both_forms {
    ($name:ident, $path:literal, $variant:ident) => {
      #[test]
      fn $name() {
        let with_slash = Route::from_str(concat!($path, "/")).unwrap();
        assert!(
          matches!(with_slash, Route::$variant {}),
          "got {}",
          with_slash
        );
        let without_slash = Route::from_str($path).unwrap();
        assert!(
          matches!(without_slash, Route::$variant {}),
          "got {}",
          without_slash
        );
        // Rendered <a href> values are the canonical trailing-slash URL.
        assert_eq!(with_slash.to_string(), concat!($path, "/"));
      }
    };
  }

  both_forms!(about, "/about", AboutUs);
  both_forms!(architecture, "/architecture", Architecture);
  both_forms!(features, "/features", FeaturesPage);
  both_forms!(how_it_works, "/how-it-works", HowItWorksPage);
  both_forms!(use_cases, "/use-cases", UseCasesPage);
  both_forms!(documentation, "/documentation", DocumentationPage);
  both_forms!(getting_started, "/getting-started", GettingStartedPage);
  both_forms!(pricing, "/pricing", PricingPage);
  both_forms!(demo, "/demo", DemoPage);
  both_forms!(api_reference, "/api-reference", ApiReferencePage);
  both_forms!(privacy, "/privacy", PrivacyPage);
  both_forms!(open_source, "/open-source", OpenSourcePage);
  both_forms!(terms, "/terms", TermsPage);

  #[test]
  fn root_unchanged() {
    assert!(matches!(Route::from_str("/").unwrap(), Route::Index {}));
    assert_eq!(Route::Index {}.to_string(), "/");
  }
}
