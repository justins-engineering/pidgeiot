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
            body: "A flock groups pigeons under one owner: think of it as a project or a fleet.",
          }
          DocStep {
            number: "3",
            title: "Create a pigeon",
            body: "Creating a pigeon mints its Ed25519 keypair and returns a one-time device token. This is the only time the token is ever shown, so copy it before dismissing the dialog.",
          }
          DocStep {
            number: "4",
            title: "Provision the device",
            body: "Build the pigeon's endpoint and token into your device firmware as CONFIG_PIGEON_ENDPOINT and CONFIG_PIGEON_TOKEN (a pre-shared-key transport adds its key); the pigeon-examples samples read them from a git-ignored prj.local.conf. HTTPS, WebSocket, CoAP (DTLS/UDP or TLS/TCP, PSK-authenticated) and MQTT over TLS on port 8883 (certificate or per-device pre-shared key) are all live, so pick whichever transport fits the hardware. ",
            Link { class: "link link-secondary", to: Route::GettingStartedPage {},
              "The getting-started guide has the exact commands."
            }
          }
          DocStep {
            number: "5",
            title: "Connect and confirm",
            body: "Once the device reports in, its shadow, telemetry, and logs (if wired up) start showing on the pigeon's detail page in the dashboard, and any alerts you define start evaluating against it.",
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

    section { id: "docs-device-to-dashboard", class: "pb-16 md:pb-24",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight",
          "From a device to a customer dashboard"
        }
        p { class: "text-base-content/70 leading-relaxed mb-10 max-w-3xl",
          "If you build devices for other businesses, this is how the pieces fit together, from a board on your bench to a page your customer signs in to."
        }
        div { class: "space-y-10",
          div {
            h3 { class: "text-xl font-bold mb-2", "Organizations, roles and invites" }
            p { class: "text-base-content/70 leading-relaxed mb-4",
              "An organization is a team of dashboard accounts that share flocks and one plan. Each person in it is an owner, an admin or a member. Owners and admins invite people, register and manage the organization's devices, set up alerts and firmware, and handle billing. Only an owner changes someone's role or deletes the organization, and an organization always keeps at least one owner. Members see everything the organization owns, edit shadows, and read telemetry, logs and alerts, but cannot register, delete or re-key a device."
            }
            p { class: "text-base-content/70 leading-relaxed",
              "An invitation is a link emailed to one address at one role. It works once and expires after seven days. Removing someone from the organization removes their access to everything it owns within about 75 seconds. Seats are counted per organization: one on the free tier, three on Builder, unlimited from Growth up, and a pending invitation holds a seat."
            }
          }
          div {
            h3 { class: "text-xl font-bold mb-2", "Flocks" }
            p { class: "text-base-content/70 leading-relaxed",
              "A flock is a group of pigeons: a site, a product line, a deployment. You create a flock on your own account and move it into an organization with Transfer to org on the flock's page, which needs you to be an owner or admin of that organization. A flock that belongs to an organization stays there; it cannot move to another one."
            }
          }
          div {
            h3 { class: "text-xl font-bold mb-2", "Pigeons" }
            p { class: "text-base-content/70 leading-relaxed",
              "A pigeon is one device. Register one pigeon per physical board: each has its own token, its own shadow and its own telemetry, so two boards sharing a pigeon would overwrite each other's state."
            }
          }
          div {
            h3 { class: "text-xl font-bold mb-4", "What the dashboard shows" }
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-6 mb-4",
              div { class: "p-6 rounded-2xl bg-base-300/50 border border-base-content/10",
                h4 { class: "font-bold mb-3", "For each pigeon" }
                ul { class: "space-y-2 text-base-content/70 leading-relaxed list-disc ml-5",
                  li {
                    "Its connection state, online, stale or offline, judged against the reporting interval the device runs."
                  }
                  li {
                    "The shadow: the config you set for the device, and the config it reports back once it has applied it."
                  }
                  li {
                    "The latest value of every telemetry key, graphs of any numeric key over a range you pick, and a CSV export of its history."
                  }
                  li { "The logs the device uploaded." }
                  li { "Its alerts and whether each one is firing." }
                }
              }
              div { class: "p-6 rounded-2xl bg-base-300/50 border border-base-content/10",
                h4 { class: "font-bold mb-3", "For each flock" }
                ul { class: "space-y-2 text-base-content/70 leading-relaxed list-disc ml-5",
                  li { "Its pigeons and their connection state." }
                  li { "Graphs across the flock's pigeons, and a CSV export of their history." }
                  li { "Alerts that apply to every pigeon in the flock." }
                }
              }
            }
            p { class: "text-base-content/70 leading-relaxed",
              "The Dashboard page sums up every flock you can see: how many devices are online, which flocks they sit in, and which flock alerts are firing. Alerts send email when a value crosses a threshold or changes too fast, when a device goes stale or offline, or when its reports stop."
            }
          }
          div {
            h3 { class: "text-xl font-bold mb-2", "Custom dashboards are not here yet" }
            p { class: "text-base-content/70 leading-relaxed mb-4",
              "There is no dashboard builder and no branded page to hand a customer. Your customer signs in to the same dashboard you use and sees the flocks and pigeons of their organization. What exists instead:"
            }
            ul { class: "space-y-2 text-base-content/70 leading-relaxed list-disc ml-5",
              li {
                "Graphs on every pigeon and flock page, saved to each person's own account, so two members of one organization keep their own."
              }
              li { "CSV export of telemetry history from every pigeon and flock page." }
              li {
                "The telemetry history routes in the "
                Link { class: "link link-secondary", to: Route::ApiReferencePage {}, "API reference" }
                ", to pull readings into your own tools."
              }
              li {
                "Forwarding: set a pigeon's Telemetry Endpoint and every report goes to a time-series database you run, as InfluxDB line protocol, in place of our history store. A numeric value arrives as a number and any other value as a string. Threshold and rate-of-change alerts need the history we keep, so they stop for that pigeon; offline, stale and missing-report alerts keep working."
              }
            }
          }
          div {
            h3 { class: "text-xl font-bold mb-2", "One organization per customer" }
            p { class: "text-base-content/70 leading-relaxed mb-4",
              "A small shop serving several customers can give each one an organization of its own:"
            }
            ol { class: "space-y-2 text-base-content/70 leading-relaxed list-decimal ml-5 mb-4",
              li {
                "Create the customer's organization on the Organizations page and choose its plan. Billing belongs to an organization, so each customer has its own plan and its own bill. An account can own one organization on the free tier; each further one needs a paid plan."
              }
              li {
                "Create the customer's flocks, register their devices as pigeons, and move the flocks into the organization with Transfer to org."
              }
              li {
                "Invite the customer's people by email: member for someone who watches devices and edits their config, admin for someone who will register devices or manage billing. The free tier's one seat is yours, so inviting anyone needs Builder or above."
              }
              li { "Keep yourself an owner, so you can still look after the devices." }
            }
            p { class: "text-base-content/70 leading-relaxed",
              "Each organization sees only its own flocks, so one customer never sees another's devices, while your own Flocks and Dashboard pages show the flocks of every organization you belong to. There is no parent account above the organizations: each one is set up and billed on its own. See "
              Link { class: "link link-secondary", to: Route::PricingPage {}, "pricing" }
              " for what each plan includes."
            }
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
            body: "Every dashboard and device route, request/response shapes, and auth models, generated straight from the maintained doc in the repo.",
            href: None,
            route: Some(Route::ApiReferencePage {}),
          }
          DocLink {
            icon: rsx! {
              Icon { icon: FaGithub, class: "size-7 stroke-primary", title: "GitHub" }
            },
            title: "pidgeiot",
            body: "The platform itself: dovecote (edge router), fancier (this dashboard), and capsules (shared wire types).",
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
            body: "Board-level sample applications built on the pigeon library: bring-up references for real hardware targets.",
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
            href: None,
            route: Some(Route::ContactPage {}),
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
            li { "The transport the device uses: HTTPS, WebSocket, CoAP, or MQTT." }
          }
          p { class: "text-base-content/70 leading-relaxed mt-6",
            "Never send a device token, a pre-shared key, or any other credential. Nothing we need to diagnose a problem requires one, and a token pasted into an email should be treated as compromised and rotated."
          }
        }
      }
    }
  }
}

/// `children` continue the step's paragraph, for a step that ends in a link.
#[component]
fn DocStep(
  number: &'static str,
  title: &'static str,
  body: &'static str,
  children: Element,
) -> Element {
  rsx! {
    div { class: "flex gap-6 items-start text-left",
      div { class: "shrink-0 size-10 rounded-full bg-primary/20 border border-primary/40 flex items-center justify-center font-bold text-primary",
        "{number}"
      }
      div {
        h3 { class: "text-xl font-bold mb-1", "{title}" }
        p { class: "text-base-content/70 leading-relaxed", "{body}" {children} }
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
