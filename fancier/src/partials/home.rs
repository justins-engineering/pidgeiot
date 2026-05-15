use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::fa_brands_icons::FaGithub;
use dioxus_free_icons::icons::ld_icons::{LdChevronsDown, LdRocket};

#[component]
pub fn Home() -> Element {
  rsx! {
    section { id: "home", class: "front-page animate-slide-in",
      h1 { class: "text-center text-4xl md:text-6xl lg:text-7xl font-bold leading-tight",
        span { class: "italic", "The" }
        span { class: "gradient-text font-extrabold block", "No Compromise" }
        "IoT Platform"
      }
      article { class: "flex flex-col space-y-4 @min-[22rem]:space-y-8 text-xl text-pretty text-center leading-relaxed",
        p {
          "An open-source, distributed, and efficient IoT platform. Built in "
          span { class: "text-orange-700 dark:text-orange-400", "Rust" }
          ", from front to back. Designed with security, privacy, and flexibility in mind."
        }
        p {
          span { class: "text-primary", " Your data." }
          span { class: "text-secondary", " Your control." }
        }
      }
      div { class: "flex flex-col sm:flex-row sm:space-x-6 space-y-4 sm:space-y-0 sm:px-4 md:px-8 lg:px-16 xl:px-32 2xl:px-64",
        Link {
          class: "grow btn btn-xl btn-glow font-bold",
          to: Route::SignUp {},
          Icon {
            icon: LdRocket,
            class: "mr-2 animate-bounce-slow",
            title: "Rocket icon",
          }
          "Get Started Free"
        }
        a {
          class: "grow btn btn-xl btn-special font-bold",
          href: "https://github.com/justins-engineering",
          Icon { icon: FaGithub, class: "mr-2", title: "GitHub logo" }
          "View on GitHub"
        }
      }
      div { class: "mx-auto",
        a { href: "/#features",
          Icon {
            icon: LdChevronsDown,
            class: "size-15 mx-auto stroke-primary animate-pulse-slow",
            title: "Scroll for more info",
          }
        }
      }
    }
  }
}
