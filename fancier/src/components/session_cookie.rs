use crate::helpers::{
  clear_return_to, remove_session_cookie, take_return_to, url_query_param,
  write_session_hint_cookie,
};
use crate::models::AuthState;
use crate::{Configuration, Create, Route, Session};
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::frontend_api::to_session;
use std::str::FromStr;

#[component]
pub fn SetSessionCookie(state: bool) -> Element {
  let mut session = use_context::<Session>();
  let nav = use_navigator();

  use_future(move || async move {
    // The address bar, not the route prop, is the source of truth for
    // `?state=`: SSG prerenders this route as `/session/local?state=false`
    // (bool Default), and hydration restores that route on the full-page
    // Kratos redirect after login/verification — so trusting the prop alone
    // ran the logout branch on every successful sign-in, tearing down the
    // session hint that had just been established. See
    // helpers::url_query_param.
    let state = url_query_param("state").map_or(state, |s| s == "true");
    if state {
      // state = true: Kratos redirect after successful login or verification.
      // We must now ask the Kratos backend to validate the secure HttpOnly cookie
      // and give us the session metadata (like expiry).
      let config = Configuration::create();

      match to_session(&config, None, None, None).await {
        Ok(kratos_session) => {
          if let Some(expires_at) = kratos_session.expires_at {
            write_session_hint_cookie(&expires_at);
            // A fresh session is no longer a lapsed one, so the login
            // view stops explaining a sign-out that has been undone.
            session.signed_out.set(false);
            session.state.set(AuthState::Authenticated);
            // Signing back in after a session ended mid-visit resumes on
            // the interrupted page. A stale or hand-edited entry can only
            // ever name an in-app route, and one that no longer resolves
            // would land on the 404 view -- worse than the dashboard, so
            // it falls back instead.
            let destination = take_return_to()
              .and_then(|path| Route::from_str(&path).ok())
              .filter(|route| !matches!(route, Route::PageNotFound { .. }))
              .unwrap_or(Route::Dashboard {});
            nav.replace(destination);
          } else {
            error!("Kratos returned a valid session, but missing expiry.");
            session.state.set(AuthState::Unauthenticated);
            nav.replace(Route::Index {});
          }
        }
        Err(err) => {
          // This handles edge cases where the redirect happened, but the
          // HttpOnly cookie was dropped or invalid.
          error!("Kratos session validation failed post-redirect: {err:?}");
          session.state.set(AuthState::Unauthenticated);
          nav.replace(Route::Index {});
        }
      }
    } else {
      // state = false: Kratos redirect after logout.
      // Tear down the UI hint and global state.
      remove_session_cookie();
      // Logging out on purpose is not being signed out, so the login form
      // must not greet a returning user with an expiry notice, and the
      // next sign-in must not resume a page they chose to leave.
      session.signed_out.set(false);
      clear_return_to();
      session.state.set(AuthState::Unauthenticated);
      nav.replace(Route::Index {});
    }
  });

  rsx! {
    div { class: "flex items-center justify-center min-h-screen",
      p { "Synchronizing secure session..." }
    }
  }
}
