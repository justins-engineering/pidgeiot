use crate::components::{OryLogOut, ThemeController};
use crate::{Route, Session};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdBird, LdHome, LdInfo, LdLogIn, LdMenu, LdSettings, LdUser, LdX,
};

#[component]
pub fn Navbar() -> Element {
  let mut is_menu_open: Signal<bool> = use_signal(|| false);
  let is_logged_in = use_context::<Session>().state.read().is_authenticated();

  rsx! {
    header { class: "w-full sticky top-0 z-50 backdrop-blur-md bg-base-200/90 border-b border-base-300 shadow-sm",
      nav { class: "navbar container mx-auto sm:px-4 lg:px-8",
        // --- Navbar Start: Logo & Brand ---
        div { class: "navbar-start",
          Link {
            to: if !is_logged_in { Route::Index {} } else { Route::Dashboard {} },
            class: "flex items-center gap-3 hover:opacity-80 transition-opacity",
            div { class: "size-10 rounded-full flex items-center justify-center bg-secondary/60 animate-glow",
              Icon {
                icon: LdBird,
                class: "size-6",
                title: "Logo",
              }
            }
            span { class: "text-primary text-2xl md:text-3xl font-bold tracking-tight",
              "Pidge"
              span { class: "text-secondary", "IoT" }
            }
          }
        }

        // --- Navbar Center: Desktop Links (Hidden when logged in) ---
        if !is_logged_in {
          div { class: "navbar-center hidden lg:flex",
            ul { class: "menu menu-horizontal px-1 gap-2 text-base font-medium",
              li {
                Link {
                  to: Route::Index {},
                  class: "hover:text-primary transition-colors duration-300",
                  "Home"
                }
              }
              li {
                Link {
                  to: Route::AboutUs {},
                  class: "hover:text-primary transition-colors duration-300",
                  "About Us"
                }
              }
              li {
                Link {
                  to: Route::Architecture {},
                  class: "hover:text-primary transition-colors duration-300",
                  "Architecture"
                }
              }
            }
          }
        }

        // --- Navbar End: Auth, Theme Toggle & Mobile Menu ---
        div { class: "navbar-end flex items-center gap-2 lg:gap-4",
          // Desktop Auth Actions
          div { class: "hidden lg:flex items-center gap-4",
            if is_logged_in {
              // --- User Profile Dropdown ---
              div { class: "dropdown dropdown-end",
                div {
                  tabindex: "0",
                  role: "button",
                  class: "btn btn-ghost btn-circle avatar border border-primary/20 hover:border-primary/50 transition-colors",
                  Icon {
                    icon: LdUser,
                    class: "size-6 text-primary",
                    title: "User Menu",
                  }
                }
                ul {
                  tabindex: "0",
                  class: "mt-3 z-1 p-2 shadow-xl menu menu-md dropdown-content bg-base-200 border border-base-300 rounded-box w-52 gap-1",
                  li {
                    Link {
                      to: Route::Dashboard {},
                      class: "justify-between",
                      "Dashboard"
                    }
                  }
                  li {
                    Link { to: Route::SettingsFlow { flow: None },
                      "Settings"
                    }
                  }
                  div { class: "divider my-0" } // Visual separator for logout
                  li { class: "text-error hover:bg-error/10 rounded-lg",
                    OryLogOut {}
                  }
                }
              }
            } else {
              Link {
                class: "btn btn-ghost btn-special font-semibold",
                to: Route::LoginFlow { flow: None },
                Icon {
                  icon: LdLogIn,
                  title: "Login",
                  class: "size-5 mr-1",
                }
                "Login"
              }
              Link {
                class: "btn btn-glow font-semibold",
                to: Route::RegisterFlow { flow: None },
                "Get Started"
              }
            }
          }

          // --- Theme Toggle (Visible on all breakpoints) ---
          ThemeController {}

          // --- Mobile Menu Toggle Button ---
          div { class: "lg:hidden",
            button {
              class: "btn btn-ghost btn-circle swap swap-rotate",
              class: if is_menu_open() { "swap-active" },
              aria_label: "Toggle menu",
              onclick: move |_| is_menu_open.toggle(),
              Icon { icon: LdMenu, class: "swap-off size-6" }
              Icon { icon: LdX, class: "swap-on size-6" }
            }
          }
        }
      }

      // --- Mobile Menu Dropdown Overlay ---
      if is_menu_open() {
        div { class: "lg:hidden bg-base-200/95 backdrop-blur-md border-t border-base-300 shadow-xl absolute w-full left-0",
          ul { class: "menu menu-lg w-full p-4 gap-2",
            // Marketing links (Hidden when logged in)
            if !is_logged_in {
              li {
                Link {
                  to: Route::Index {},
                  onclick: move |_| is_menu_open.set(false),
                  Icon {
                    icon: LdHome,
                    class: "size-5 mr-2 opacity-70",
                  }
                  "Home"
                }
              }
              li {
                Link {
                  to: Route::AboutUs {},
                  onclick: move |_| is_menu_open.set(false),
                  Icon {
                    icon: LdInfo,
                    class: "size-5 mr-2 opacity-70",
                  }
                  "About Us"
                }
              }
              div { class: "divider my-2" }
            }
            // Mobile Auth Actions
            if is_logged_in {
              li {
                Link {
                  class: "text-primary font-bold",
                  to: Route::Dashboard {},
                  onclick: move |_| is_menu_open.set(false),
                  Icon { icon: LdUser, class: "size-5 mr-2" }
                  "Dashboard"
                }
              }
              li {
                Link {
                  to: Route::SettingsFlow { flow: None },
                  onclick: move |_| is_menu_open.set(false),
                  Icon {
                    icon: LdSettings,
                    class: "size-5 mr-2 opacity-70",
                  }
                  "Settings"
                }
              }
              li {
                span {
                  class: "text-error",
                  onclick: move |_| is_menu_open.set(false),
                  OryLogOut {}
                }
              }
            } else {
              li {
                Link {
                  to: Route::LoginFlow { flow: None },
                  onclick: move |_| is_menu_open.set(false),
                  Icon {
                    icon: LdLogIn,
                    class: "size-5 mr-2 opacity-70",
                  }
                  "Login"
                }
              }
              li {
                Link {
                  class: "btn btn-glow font-semibold mt-4 text-center block",
                  to: Route::RegisterFlow { flow: None },
                  onclick: move |_| is_menu_open.set(false),
                  "Get Started"
                }
              }
            }
          }
        }
      }
    }
  }
}
