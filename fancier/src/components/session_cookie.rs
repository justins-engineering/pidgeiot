use crate::helpers::{
  adopt_kratos_session, clear_return_to, remove_session_cookie, take_return_to, url_query_param,
};
use crate::models::AuthState;
use crate::{Route, Session};
use dioxus::prelude::*;
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
      if adopt_kratos_session(session).await {
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
        // The redirect landed but no session came back with it.
        session.state.set(AuthState::Unauthenticated);
        nav.replace(Route::Index {});
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
