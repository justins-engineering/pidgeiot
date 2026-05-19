use crate::components::{SetSessionCookie, session_cookie_valid};
use crate::config::{KRATOS_BROWSER_URL, SESSION_COOKIE_NAME};
use capsules::{Flock, Pigeon};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use ory_kratos_client_wasm::apis::configuration::Configuration;
use std::collections::HashMap;
use unic_langid::langid;
use views::{
  AboutUs, AccountRecovery, Architecture, Dashboard, Flocks, Index, LoginFlow, PageNotFound,
  PigeonView, Pigeons, RecoveryFlow, RegisterFlow, ServerError, SessionInfo, Settings,
  SettingsFlow, SignIn, SignUp, Unauthorized, VerificationFlow, Verify, Wrapper,
};

pub mod api;
mod components;
mod config;
mod partials;
mod views;

#[derive(Clone, Copy, Debug)]
struct Session {
  state: Signal<bool>,
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
    Pigeons { flock_id: String },
    #[route("/flocks/:flock_id/pigeons/:pigeon_id")]
    PigeonView { flock_id: String, pigeon_id: String },
    #[route("/session")]
    SessionInfo {},
    #[route("/my-settings")]
    Settings {},
    #[route("/settings?:flow")]
    SettingsFlow { flow: String },
  #[end_layout]
  #[route("/")]
  Index {},
  #[route("/about")]
  AboutUs {},
  #[route("/architecture")]
  Architecture {},
  #[route("/sign-in")]
  SignIn {},
  #[route("/login?:flow")]
  LoginFlow { flow: String },
  #[route("/sign-up")]
  SignUp {},
  #[route("/registration?:flow")]
  RegisterFlow { flow: String },
  #[route("/verify")]
  Verify {},
  #[route("/verification?:flow")]
  VerificationFlow { flow: String },
  #[route("/account-recovery")]
  AccountRecovery {},
  #[route("/recovery?:flow")]
  RecoveryFlow { flow: String },
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
  if !*use_context::<Session>().state.read() {
    let nav = use_navigator();
    nav.replace(Route::Unauthorized {});
    return rsx! {};
  }

  rsx! {
    Outlet::<Route> {}
  }
}

#[derive(Clone, Copy, Debug)]
struct LocalSession {
  flocks: Signal<HashMap<String, Flock>>,
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

  use_effect(crate::components::set_lang);

  let session = use_context_provider(|| Session {
    state: Signal::new(false),
  });

  let set_state = use_resource(move || async move { session_cookie_valid().await });
  (set_state)();

  let mut local_session = use_context_provider(|| LocalSession {
    flocks: Signal::new(HashMap::new()),
    pigeons: Signal::new(HashMap::new()),
  });

  use_resource(move || async move {
    if (session.state)() {
      if let Some(flocks_data) = api::flocks::list().await {
        local_session.flocks.set(flocks_data);
      }
    }
  });

  rsx! {
    document::Link { rel: "stylesheet", href: asset!("/assets/styling/main.css") }
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
