use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::fa_brands_icons::FaGithub;
use dioxus_free_icons::icons::ld_icons::{
  LdActivity, LdBookOpen, LdChevronRight, LdCode, LdFileText, LdLifeBuoy, LdMail, LdPlay, LdRadio,
  LdRocket,
};

#[component]
pub fn DocumentationPage() -> Element {
  rsx! {
    section { id: "docs-hero", class: "py-24 md:py-32",
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
        Link {
          class: "btn btn-primary rounded-full mt-8",
          to: Route::GettingStartedPage {},
          Icon { icon: LdRocket, class: "mr-2", title: "Rocket" }
          "Try it now -- no hardware required"
        }
      }
    }

    // Getting started
    section { id: "docs-getting-started", class: "pb-16 md:pb-24",
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
            body: "Bake the pigeon's endpoint and token into your device build (see the pigeon library below). HTTPS, WebSocket, and CoAP (DTLS/UDP or TLS/TCP, PSK-authenticated) are all live — pick whichever transport fits the hardware.",
          }
          DocStep {
            number: "5",
            title: "Connect and confirm",
            body: "Once the device reports in, its shadow, telemetry, and logs (if wired up) start showing on the pigeon's detail page in the dashboard — and any alerts you define start evaluating against it.",
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
    section { id: "docs-reference", class: "pb-16 md:pb-24",
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

    section { id: "docs-support", class: "pb-24 md:pb-32",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight", "Support" }
        p { class: "text-base-content/70 leading-relaxed mb-10 max-w-3xl",
          "PidgeIoT is built and run by one engineer, so here is the honest version: you will get a considered reply from the person who wrote the code, and it will usually take a day, not a minute."
        }
        div { class: "grid grid-cols-1 md:grid-cols-2 gap-6 mb-10",
          DocLink {
            icon: rsx! {
              Icon { icon: LdMail, class: "size-7 stroke-primary", title: "Envelope" }
            },
            title: "support@pidgeiot.com",
            body: "Email for anything already broken, plus billing and account questions. Expect a reply within two business days.",
            href: Some("mailto:support@pidgeiot.com"),
            route: None,
          }
          DocLink {
            icon: rsx! {
              Icon { icon: LdActivity, class: "size-7 stroke-primary", title: "Activity" }
            },
            title: "Status page",
            body: "Live availability of the API, authentication and dashboard, plus any incident we are working on. Check here first.",
            href: Some("https://status.pidgeiot.com"),
            route: None,
          }
          DocLink {
            icon: rsx! {
              Icon { icon: LdLifeBuoy, class: "size-7 stroke-primary", title: "Life buoy" }
            },
            title: "Contact form",
            body: "Sales questions, feature requests, and anything with enough detail to be worth structuring. It reaches the same inbox.",
            href: Some("/contact/"),
            route: None,
          }
          DocLink {
            icon: rsx! {
              Icon { icon: FaGithub, class: "size-7 stroke-primary", title: "GitHub" }
            },
            title: "Issues and discussion",
            body: "Bugs in the open-source device library or the platform itself are best raised where the code lives.",
            href: Some("https://github.com/justins-engineering"),
            route: None,
          }
        }
        div { class: "p-6 md:p-8 rounded-2xl bg-base-300/50 border border-base-content/10",
          h3 { class: "text-xl font-bold mb-4", "What to include" }
          p { class: "text-base-content/70 leading-relaxed mb-4",
            "A report with these five things can usually be answered on the first reply instead of the third:"
          }
          ul { class: "space-y-2 text-base-content/70 leading-relaxed list-disc list-inside",
            li { "The email address on your account." }
            li { "The flock and pigeon involved, by id." }
            li { "What you expected to happen, and what happened instead." }
            li { "When it happened, in UTC, and whether it is still happening." }
            li { "The transport the device uses: HTTPS, WebSocket, or CoAP." }
          }
          p { class: "text-base-content/70 leading-relaxed mt-6",
            "Never send a device token, a pre-shared key, or any other credential. Nothing we need to diagnose a problem requires one, and a token pasted into an email should be treated as compromised and rotated."
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
