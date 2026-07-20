use crate::components::SetSessionCookie;
use crate::config::{KRATOS_BROWSER_URL, SESSION_COOKIE_NAME};
use crate::helpers::session_cookie_valid;
use crate::models::AuthState;
use capsules::{Flock, Pigeon};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use ory_kratos_client_wasm::apis::configuration::Configuration;
use std::collections::HashMap;
use unic_langid::langid;
use uuid::Uuid;
use views::{
  AboutUs, ApiReferencePage, Architecture, Dashboard, DocumentationPage, FeaturesPage, Flocks,
  Index, LoginFlow, PageNotFound, PigeonView, Pigeons, PricingPage, RecoveryFlow, RegisterFlow,
  ServerError, SessionInfo, SettingsFlow, Unauthorized, VerificationFlow, Wrapper,
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
    #[route("/session")]
    SessionInfo {},
    #[route("/settings?:flow")]
    SettingsFlow { flow: Option<String> },
  #[end_layout]
  #[route("/")]
  Index {},
  #[route("/about")]
  AboutUs {},
  #[route("/architecture")]
  Architecture {},
  #[route("/features")]
  FeaturesPage {},
  #[route("/documentation")]
  DocumentationPage {},
  #[route("/pricing")]
  PricingPage {},
  #[route("/api-reference")]
  ApiReferencePage {},
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

#[component]
fn AuthGuard() -> Element {
  let session = use_context::<Session>();

  match (session.state)() {
    AuthState::Authenticated => {
      rsx! {
        Outlet::<Route> {}
      }
    }
    AuthState::Unauthenticated => {
      let nav = use_navigator();
      nav.replace(Route::Unauthorized {});
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
  });

  use_resource(move || async move {
    if (session.state)() == AuthState::Authenticated {
      api::flocks::list().await;
    }
  });

  rsx! {
    document::Meta {
      name: "description",
      content: "PidgeIoT is an edge-native IoT device management platform: provision devices, push configuration, and collect telemetry from a Cloudflare Workers + Durable Objects backend.",
    }
    // Release builds get main.css from a static <link> in index.html instead
    // (Dioxus.toml's [web.resource], populated by scripts/build-release.sh) —
    // it loads in parallel with app.js/wasm rather than only after this
    // component mounts post-WASM-boot, which was the FOUC/CLS root cause
    // (task #9 design review, ~0.10 layout shift on every page load). Dev
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
