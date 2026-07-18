use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::fa_brands_icons::FaGithub;
use dioxus_free_icons::icons::ld_icons::{
  LdBookOpen, LdChevronRight, LdCode, LdFileText, LdPlay, LdRadio,
};

#[component]
pub fn DocumentationPage() -> Element {
  rsx! {
    section { class: "py-24 md:py-32",
      div { class: "max-w-4xl mx-auto px-4 md:px-8 text-center",
        Icon {
          icon: LdBookOpen,
          class: "w-12 h-12 mx-auto mb-8",
          title: "Open book",
        }
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "Documentation"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-3xl mx-auto text-balance",
          "This page is honest about what exists: a getting-started path through the dashboard, a full API reference, and the source for everything else."
        }
      }
    }

    // Getting started
    section { class: "pb-16 md:pb-24",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-10 tracking-tight", "Getting Started" }
        div { class: "space-y-6",
          DocStep {
            number: "1",
            title: "Create an account",
            body: "Register a dashboard account (self-hosted Ory Kratos) and sign in.",
          }
          DocStep {
            number: "2",
            title: "Create a flock",
            body: "A flock groups pigeons under one owner — think of it as a project or a fleet.",
          }
          DocStep {
            number: "3",
            title: "Create a pigeon",
            body: "Creating a pigeon mints its Ed25519 keypair and returns a one-time device token — this is the only time the token is ever shown. Copy it before dismissing the dialog.",
          }
          DocStep {
            number: "4",
            title: "Provision the device",
            body: "Bake the pigeon's endpoint and token into your device build (see the pigeon library below), or use the CoAP-over-TLS PSK fields if your device speaks CoAP instead of HTTPS.",
          }
          DocStep {
            number: "5",
            title: "Connect and confirm",
            body: "Once the device reports in, its shadow, telemetry, and logs (if wired up) start showing on the pigeon's detail page in the dashboard.",
          }
        }
        div { class: "mt-10 flex flex-col sm:flex-row gap-4",
          Link {
            class: "btn btn-primary rounded-full",
            to: Route::RegisterFlow { flow: None },
            Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
            "Create an Account"
          }
          Link {
            class: "btn btn-outline rounded-full",
            to: Route::ApiReferencePage {},
            Icon { icon: LdFileText, class: "mr-2", title: "API reference" }
            "Read the API Reference"
          }
        }
      }
    }

    // Reference & source
    section { class: "pb-24 md:pb-32",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-10 tracking-tight", "Reference & Source" }
        div { class: "grid grid-cols-1 md:grid-cols-2 gap-6",
          DocLink {
            icon: rsx! {
              Icon { icon: LdFileText, class: "size-7 stroke-primary", title: "File" }
            },
            title: "API Reference",
            body: "Every dashboard and device route, request/response shapes, and auth models — generated straight from the maintained doc in the repo.",
            href: None,
            route: Some(Route::ApiReferencePage {}),
          }
          DocLink {
            icon: rsx! {
              Icon { icon: FaGithub, class: "size-7 stroke-primary", title: "GitHub" }
            },
            title: "pidgeiot",
            body: "The platform itself — dovecote (edge router), fancier (this dashboard), and capsules (shared wire types).",
            href: Some("https://github.com/justins-engineering/pidgeiot"),
            route: None,
          }
          DocLink {
            icon: rsx! {
              Icon { icon: LdRadio, class: "size-7 stroke-primary", title: "Radio" }
            },
            title: "pigeon",
            body: "The Zephyr device library: shadow fetch/report, telemetry, dictionary log upload, and the FOTA client.",
            href: Some("https://github.com/justins-engineering/pigeon"),
            route: None,
          }
          DocLink {
            icon: rsx! {
              Icon { icon: LdCode, class: "size-7 stroke-primary", title: "Code" }
            },
            title: "pigeon-examples",
            body: "Board-level sample applications built on the pigeon library — bring-up references for real hardware targets.",
            href: Some("https://github.com/justins-engineering/pigeon-examples"),
            route: None,
          }
        }
      }
    }
  }
}

#[component]
fn DocStep(number: &'static str, title: &'static str, body: &'static str) -> Element {
  rsx! {
    div { class: "flex gap-6 items-start text-left",
      div { class: "shrink-0 size-10 rounded-full bg-primary/20 border border-primary/40 flex items-center justify-center font-bold text-primary",
        "{number}"
      }
      div {
        h3 { class: "text-xl font-bold mb-1", "{title}" }
        p { class: "text-base-content/70 leading-relaxed", "{body}" }
      }
    }
  }
}

#[component]
fn DocLink(
  icon: Element,
  title: &'static str,
  body: &'static str,
  href: Option<&'static str>,
  route: Option<Route>,
) -> Element {
  let inner = rsx! {
    div { class: "shrink-0 mt-1", {icon} }
    div {
      div { class: "flex items-center gap-2",
        h3 { class: "text-xl font-bold", "{title}" }
        Icon {
          icon: LdChevronRight,
          class: "opacity-0 group-hover:opacity-100 transition-opacity",
          title: "Chevron right",
        }
      }
      p { class: "text-base-content/70 leading-relaxed mt-1", "{body}" }
    }
  };
  rsx! {
    div { class: "p-6 rounded-2xl bg-base-300/50 border border-base-content/10 hover:border-primary/40 transition-colors",
      if let Some(r) = route {
        Link { class: "group flex gap-4 items-start", to: r, {inner} }
      } else if let Some(h) = href {
        a { class: "group flex gap-4 items-start", href: h, target: "_blank", rel: "noopener noreferrer", {inner} }
      }
    }
  }
}
