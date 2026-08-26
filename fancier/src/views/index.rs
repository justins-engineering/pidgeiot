// Section order follows the chosen homepage design: hook, then the product
// itself, then how it gets there, then who it's for, then the licence. The
// `why` block is the quieter investors/incubators section and must stay
// below all of the user-facing sections, just above the closing CTA.
//
// These seven sections lived in their own files under partials/ until each
// turned out to be used exactly once, here. Inlined so the whole page, and
// every section id on it, is visible by reading one file.

use super::dashboard::{FlockConnStats, device_card_theme, flock_status_summary};
use crate::Route;
use crate::components::{ConnectorBadge, Maturity, MaturityBadge};
use crate::helpers::connection_state::{ConnectionState, ConnectionStateStyle};
use capsules::{CoapConfig, Connector};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdBird, LdKeyRound, LdLayoutGrid, LdMail, LdScrollText, LdServer, LdTriangleAlert, LdWifi,
};

// The heading over this depiction claims the picture is the product, so it
// has to stay one: `DashboardMock` mirrors `views::dashboard` panel for
// panel, and wherever that page has presentation logic this borrows it
// rather than restating it -- `ConnectionStateStyle` for what a state is
// called and coloured, `ConnectorBadge` for the transport chip,
// `flock_status_summary` for which single status a flock shows,
// `device_card_theme` for a card's tint. The two can drift in layout and
// wording; they cannot drift in what the product calls things. Anything the
// real dashboard grows belongs here too.

/// One flock in the illustrative fleet. Only its per-state counts are
/// written down: the flock's own pigeon count, and every headline number
/// above it, are summed back out of these, so no two parts of the picture
/// can disagree about how big the fleet is.
struct MockFlock {
  name: &'static str,
  stats: FlockConnStats,
}

impl MockFlock {
  fn pigeons(&self) -> usize {
    self.stats.online + self.stats.stale + self.stats.offline + self.stats.unknown
  }
}

const FLOCKS: [MockFlock; 4] = [
  MockFlock {
    name: "West Fleet",
    stats: FlockConnStats {
      online: 17,
      stale: 0,
      offline: 1,
      unknown: 0,
    },
  },
  MockFlock {
    name: "North Barn",
    stats: FlockConnStats {
      online: 10,
      stale: 2,
      offline: 0,
      unknown: 0,
    },
  },
  MockFlock {
    name: "Riverside",
    stats: FlockConnStats {
      online: 10,
      stale: 0,
      offline: 0,
      unknown: 0,
    },
  },
  MockFlock {
    name: "Bench",
    stats: FlockConnStats {
      online: 0,
      stale: 0,
      offline: 0,
      unknown: 2,
    },
  },
];

/// The head of the device grid. Ordered worst-first, the way the real page
/// sorts it, and covering all four states -- including the device that has
/// never reported at all, which is the one a three-state picture always
/// leaves out.
struct MockDevice {
  id: &'static str,
  flock: &'static str,
  state: ConnectionState,
  coap: bool,
  seen: &'static str,
}

const DEVICES: [MockDevice; 4] = [
  MockDevice {
    id: "pigeon-0440",
    flock: "West Fleet",
    state: ConnectionState::Offline,
    coap: true,
    seen: "4h ago",
  },
  MockDevice {
    id: "pigeon-0421",
    flock: "North Barn",
    state: ConnectionState::Stale,
    coap: false,
    seen: "6m ago",
  },
  MockDevice {
    id: "pigeon-0433",
    flock: "Bench",
    state: ConnectionState::Unknown,
    coap: false,
    seen: "Never seen",
  },
  MockDevice {
    id: "pigeon-0417",
    flock: "West Fleet",
    state: ConnectionState::Online,
    coap: true,
    seen: "just now",
  },
];

/// `ConnectorBadge` renders from a real `Connector`, so the picture gets a
/// real one. The configs are empty because only the variant reaches the
/// badge, and an illustration has no endpoint or credential to name.
fn mock_connector(coap: bool) -> Connector {
  if coap {
    Connector::Coap(CoapConfig::default())
  } else {
    Connector::default()
  }
}

/// Every control here is a `span`, never a `Link`: a visitor reading this is
/// not signed in, so a real button would only bounce them off to the login
/// page from inside what is meant to be a picture.
#[component]
fn DashboardMock() -> Element {
  let online: usize = FLOCKS.iter().map(|f| f.stats.online).sum();
  let stale: usize = FLOCKS.iter().map(|f| f.stats.stale).sum();
  let offline: usize = FLOCKS.iter().map(|f| f.stats.offline).sum();
  let unknown: usize = FLOCKS.iter().map(|f| f.stats.unknown).sum();
  let total_pigeons = online + stale + offline + unknown;
  let total_flocks = FLOCKS.len();
  let needs_attention = stale + offline;
  let pct = |n: usize| (n as f64 / total_pigeons as f64) * 100.0;

  rsx! {
    section { id: "home-dashboard", class: "pt-14 pb-16",
      div { class: "max-w-6xl mx-auto",
        div { class: "text-center mb-9",
          h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
            "This is the whole dashboard"
          }
          p { class: "text-lg text-base-content/70 mt-2",
            "No modules to buy, no widgets to assemble. Every device shows up here the moment it checks in."
          }
        }

        div { class: "rounded-2xl border border-base-300 bg-base-200 p-3",
          div { class: "flex items-center gap-2 px-2 py-2",
            span { class: "size-2.5 rounded-full bg-error" }
            span { class: "size-2.5 rounded-full bg-warning" }
            span { class: "size-2.5 rounded-full bg-success" }
            span { class: "ml-3 text-xs font-mono text-base-content/50 truncate",
              "pidgeiot.com/dashboard"
            }
          }

          div { class: "rounded-xl bg-base-100 border border-base-300 p-4 md:p-6",

            header { class: "flex flex-col md:flex-row items-start md:items-center justify-between gap-4 mb-8",
              div { class: "min-w-0",
                h3 { class: "text-2xl font-bold", "Fleet Overview" }
                p { class: "text-base-content/60 text-sm mt-1",
                  "{total_pigeons} pigeons across {total_flocks} flocks"
                }
              }
              span { class: "btn btn-outline btn-primary sm:px-6 pointer-events-none",
                "Manage Flocks"
              }
            }

            div { class: "stats shadow-sm bg-base-100 border border-base-content/10 w-full grid grid-flow-row grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 mb-8",
              div { class: "stat",
                div { class: "stat-figure text-secondary",
                  Icon { width: 28, height: 28, icon: LdLayoutGrid, title: "Flocks" }
                }
                div { class: "stat-title", "Flocks" }
                div { class: "stat-value text-secondary", "{total_flocks}" }
                div { class: "stat-desc", "Groups you own" }
              }
              div { class: "stat",
                div { class: "stat-figure text-primary",
                  Icon { width: 28, height: 28, icon: LdBird, title: "Pigeons" }
                }
                div { class: "stat-title", "Pigeons" }
                div { class: "stat-value text-primary", "{total_pigeons}" }
                div { class: "stat-desc", "Registered devices" }
              }
              div { class: "stat",
                div { class: "stat-figure text-success",
                  Icon { width: 26, height: 26, icon: LdWifi, title: "Online" }
                }
                div { class: "stat-title", "Online" }
                div { class: "stat-value text-success", "{online}" }
                div { class: "stat-desc", "Seen within cadence" }
              }
              div { class: "stat",
                div { class: "stat-figure text-warning",
                  Icon {
                    width: 26,
                    height: 26,
                    icon: LdTriangleAlert,
                    title: "Needs attention",
                  }
                }
                div { class: "stat-title", "Needs Attention" }
                div { class: "stat-value text-warning", "{needs_attention}" }
                div { class: "stat-desc", "Stale or offline" }
              }
            }

            div { class: "grid grid-cols-1 lg:grid-cols-3 gap-6",
              div { class: "lg:col-span-2 flex flex-col gap-6 min-w-0",

                div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
                  h4 { class: "text-lg font-bold mb-4", "Fleet Health" }
                  div { class: "w-full h-3 rounded-full overflow-hidden flex bg-base-300",
                    div { class: "h-full bg-success", style: "width: {pct(online)}%" }
                    div { class: "h-full bg-warning", style: "width: {pct(stale)}%" }
                    div { class: "h-full bg-error", style: "width: {pct(offline)}%" }
                    div { class: "h-full bg-base-content/20", style: "width: {pct(unknown)}%" }
                  }
                  div { class: "flex flex-wrap gap-x-6 gap-y-2 mt-4 text-sm",
                    div { class: "flex items-center gap-2",
                      span { class: "status status-success" }
                      "Online "
                      span { class: "font-semibold", "{online}" }
                    }
                    div { class: "flex items-center gap-2",
                      span { class: "status status-warning" }
                      "Stale "
                      span { class: "font-semibold", "{stale}" }
                    }
                    div { class: "flex items-center gap-2",
                      span { class: "status status-error" }
                      "Offline "
                      span { class: "font-semibold", "{offline}" }
                    }
                    div { class: "flex items-center gap-2",
                      span { class: "status" }
                      "Unknown "
                      span { class: "font-semibold", "{unknown}" }
                    }
                  }
                }

                div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
                  div { class: "flex items-center justify-between gap-3 mb-4",
                    h4 { class: "text-lg font-bold", "Devices" }
                    span { class: "text-xs text-base-content/50 text-right",
                      "Sorted by status · showing {DEVICES.len()} of {total_pigeons}"
                    }
                  }
                  div { class: "grid grid-cols-1 sm:grid-cols-2 gap-3",
                    for device in DEVICES.iter() {
                      div {
                        key: "{device.id}",
                        class: "border {device_card_theme(device.state)} rounded-box p-4 flex flex-col gap-2 min-w-0",
                        div { class: "flex items-center justify-between gap-2",
                          span { class: "font-semibold text-primary truncate", "{device.id}" }
                          div { class: "badge {device.state.badge_class()} gap-1.5 shrink-0",
                            span { class: "{device.state.status_class()}" }
                            "{device.state.label()}"
                          }
                        }
                        div { class: "flex items-center gap-2 text-xs text-base-content/60 min-w-0",
                          ConnectorBadge { connector: mock_connector(device.coap) }
                          span { class: "truncate", "{device.flock}" }
                        }
                        div { class: "flex items-center justify-between gap-2 text-xs",
                          span { class: "text-base-content/50", "{device.seen}" }
                          span { class: "text-base-content/60", "View →" }
                        }
                      }
                    }
                  }
                  div { class: "flex justify-end mt-4",
                    span { class: "btn btn-ghost btn-sm text-base-content/60 pointer-events-none",
                      "View all pigeons by flock →"
                    }
                  }
                }
              }

              div { class: "flex flex-col gap-6 min-w-0",

                div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
                  div { class: "flex items-center justify-between gap-3 mb-4",
                    h4 { class: "text-lg font-bold", "Flocks" }
                    span { class: "text-xs text-base-content/60", "View all →" }
                  }
                  div { class: "flex flex-col gap-3",
                    for flock in FLOCKS.iter() {
                      div {
                        key: "{flock.name}",
                        class: "flex items-center justify-between gap-3 rounded-box border border-base-content/10 p-3",
                        div { class: "min-w-0",
                          div { class: "font-semibold text-secondary text-sm truncate", "{flock.name}" }
                          div { class: "text-xs text-base-content/50 mt-0.5",
                            "{flock.pigeons()} pigeons"
                          }
                        }
                        if let Some((dot_class, label)) = flock_status_summary(flock.stats) {
                          div { class: "flex items-center gap-1 shrink-0",
                            span { class: "status {dot_class}" }
                            span { class: "text-xs text-base-content/60", "{label}" }
                          }
                        }
                      }
                    }
                  }
                }

                div { class: "bg-base-100 border border-base-content/10 rounded-box shadow-sm p-6",
                  div { class: "flex items-center justify-between gap-3 mb-3",
                    h4 { class: "text-lg font-bold", "Alerts" }
                  }
                  div { class: "flex flex-col gap-2",
                    div { class: "flex items-center justify-between gap-3 text-sm",
                      span { class: "text-base-content/70", "Flock-level alerts firing" }
                      span { class: "font-semibold", "1" }
                    }
                    p { class: "text-xs text-base-content/50",
                      "Per-pigeon alerts aren't counted here. Open a pigeon's own page to see those."
                    }
                    span { class: "text-xs text-base-content/60 mt-2",
                      "Manage alerts in a flock →"
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}

#[component]
fn Stop(number: &'static str, title: &'static str, body: &'static str, mock: Element) -> Element {
  rsx! {
    div { class: "rounded-2xl border border-base-300 bg-base-100 p-6 md:p-7 flex flex-col gap-4 min-w-0",
      div { class: "flex items-center gap-3",
        span {
          class: "size-9 rounded-full bg-primary font-bold flex items-center justify-center shrink-0",
          style: "color:var(--color-primary-content)",
          "{number}"
        }
        h3 { class: "text-xl md:text-2xl font-bold", "{title}" }
      }
      p { class: "leading-relaxed text-base-content/80", "{body}" }
      {mock}
    }
  }
}

struct Card {
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
}

const CARDS: [Card; 5] = [
  Card {
    eyebrow: "Fleet",
    title: "Vehicle & asset tracking",
    body: "Tracks drawn from plain GPS telemetry. Alerts when an asset moves, or stops reporting.",
  },
  Card {
    eyebrow: "Farm",
    title: "Irrigation & soil",
    body: "Moisture per block, valves as config, a season of battery between visits.",
  },
  Card {
    eyebrow: "Factory",
    title: "Machine monitoring",
    body: "Vibration trends, rate-of-change alarms, remote logs when something's off.",
  },
  Card {
    eyebrow: "Utility",
    title: "Water metering",
    body: "Small payloads, sent reliably, with history in your own database if you prefer.",
  },
  // The design promised "same shape at 5 units or 50,000". The
  // architecture holds, but the dashboard has no paginated device list
  // yet, so the claim is made about the edge model rather than a fleet
  // size we can't render.
  Card {
    eyebrow: "City",
    title: "Smart parking",
    body: "Bay occupancy served from the edge nearest each sensor: one object per device, however many there are.",
  },
];

/// The investors/incubators section — deliberately placed after every
/// user-facing section and deliberately quieter than them (no gradient
/// boxes, no animation): the page sells to individual builders first, and
/// this section only states architecture and position that the rest of the
/// page has already demonstrated. Hard rule: no invented traction,
/// customers, or numbers here, ever. The roadmap item carries a `Planned`
/// badge for the same reason.

#[component]
pub fn Index() -> Element {
  rsx! {
    section { id: "home-hero", class: "pt-20 pb-14 text-center",
      div { class: "max-w-5xl mx-auto",
        h1 { class: "text-5xl md:text-7xl font-extrabold tracking-tight max-w-4xl mx-auto text-pretty",
          "Carrier pigeons for your sensors."
          br {}
          span { class: "text-primary", "Considerably faster." }
        }
        p { class: "mt-7 text-xl md:text-2xl leading-relaxed max-w-2xl mx-auto text-base-content/80 text-pretty",
          "An open-source platform that provisions your devices, keeps their config and firmware current, and brings their readings home."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-9",
          Link { class: "btn btn-primary btn-lg font-bold", to: Route::DemoPage {},
            "Try the live demo"
          }
          a {
            class: "btn btn-outline btn-lg font-bold",
            href: "https://github.com/justins-engineering",
            target: "_blank",
            rel: "noopener noreferrer",
            "Read the source"
          }
        }
        p { class: "mt-5 text-sm text-base-content/60 font-mono",
          "no signup · no hardware · AGPL-3.0"
        }
      }
    }

    DashboardMock {}

    section { id: "home-route", class: "py-16",
      div { class: "max-w-6xl mx-auto",
        h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight text-center",
          "The whole trip, three stops"
        }
        div { class: "grid grid-cols-1 md:grid-cols-3 gap-6 mt-10",

          // The design had the device mint its own key, via a `pidge`
          // CLI. Neither is real: the keypair is minted server-side when
          // the pigeon is registered, and there is no CLI to install.
          Stop {
            number: "1",
            title: "Your device",
            body: "Flash the Zephyr library. Registering the device mints its keypair and hands back a 69-byte token; the private half signs that token and is discarded, so only the public key is ever stored. From there it speaks CoAP over DTLS or plain HTTPS, whatever the modem can afford.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/70 overflow-x-auto",
                p { class: "whitespace-nowrap", "dashboard → Register Pigeon" }
                p { class: "whitespace-nowrap text-success", "✓ keypair minted, public key stored" }
                p { class: "whitespace-nowrap text-success", "✓ token issued (69 B), shown once" }
              }
            },
          }

          Stop {
            number: "2",
            title: "The edge",
            body: "Each device owns a small object on Cloudflare's network: its shadow, its permissions, its credentials. Nothing to provision, nothing to patch, close to wherever it wakes up.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 flex flex-col gap-2",
                div { class: "flex items-center justify-between text-sm gap-3",
                  span { class: "text-base-content/60", "desired" }
                  span { class: "font-mono", "interval: 60s" }
                }
                div { class: "flex items-center justify-between text-sm gap-3",
                  span { class: "text-base-content/60", "reported" }
                  span { class: "font-mono text-success", "interval: 60s ✓" }
                }
                progress { class: "progress progress-primary", value: "100", max: "100" }
              }
            },
          }

          // The design's sample used a /v1 prefix we don't have and a flat
          // float map; the real route returns one row per key with string
          // values and the timestamp they were reported at.
          Stop {
            number: "3",
            title: "You",
            body: "The dashboard above: graphs, GPS tracks, firmware rollouts, remote logs and alerts by email. Or bypass it: the API the dashboard uses is the API you get.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/70 overflow-x-auto",
                p { class: "whitespace-nowrap", "GET /pigeons/0417/telemetry" }
                p { class: "whitespace-nowrap",
                  span { class: "text-success", "200" }
                  " · [{{\"key\":\"temp_c\",\"value\":\"21.4\","
                }
                p { class: "whitespace-nowrap", "        \"reported_at\":\"…\"}}, …]" }
              }
            },
          }
        }
      }
    }

    section { id: "home-use-cases", class: "py-16",
      div { class: "max-w-6xl mx-auto",
        h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
          "Where it earns its keep"
        }
        p { class: "text-lg text-base-content/70 mt-2 mb-9",
          "Examples, not case studies. We're in beta and we're not going to pretend otherwise."
        }
        div { class: "grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4",
          for c in CARDS.iter() {
            div {
              key: "{c.title}",
              class: "rounded-2xl bg-base-200 border border-base-300 p-6 flex flex-col gap-3 min-w-0",
              span { class: "text-xs font-mono uppercase tracking-widest text-primary", "{c.eyebrow}" }
              h3 { class: "text-lg font-bold leading-snug", "{c.title}" }
              p { class: "text-sm leading-relaxed text-base-content/75", "{c.body}" }
            }
          }
        }
        div { class: "mt-8",
          Link { class: "link link-primary font-semibold", to: Route::UseCasesPage {},
            "See what each one uses →"
          }
        }
      }
    }

    section { id: "home-open-source", class: "py-16",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 md:grid-cols-3 gap-8",

        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Open, all of it" }
          p { class: "leading-relaxed text-base-content/80",
            "AGPL-3.0 across the edge router, the dashboard and the device library. No open core with the useful parts behind a sales call."
          }
          a {
            class: "link link-primary font-semibold text-sm",
            href: "https://github.com/justins-engineering",
            target: "_blank",
            rel: "noopener noreferrer",
            "Browse the repos →"
          }
        }

        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Secure by shape" }
          p { class: "leading-relaxed text-base-content/80",
            "Per-device Ed25519 keys, encrypted transports only, and a token so small it costs nothing to send. One compromised device stays one compromised device."
          }
          Link { class: "link link-primary font-semibold text-sm", to: Route::HowItWorksPage {},
            "Security model →"
          }
        }

        // The design said telemetry sent to your own endpoint is never
        // stored. The latest value per key is always kept — it's what the
        // dashboard renders and what alerts evaluate against — so the
        // claim is scoped to the history, which is the part that's true.
        div { class: "flex flex-col gap-3 min-w-0",
          h3 { class: "text-xl md:text-2xl font-bold", "Private by default" }
          p { class: "leading-relaxed text-base-content/80",
            "Send telemetry to your own endpoint and the history accumulates there, not here: we keep only the latest value per key. Dashboard identity is self-hosted, so your credentials don't visit a third party."
          }
          Link { class: "link link-primary font-semibold text-sm", to: Route::HowItWorksPage {},
            "How data flows →"
          }
        }
      }
    }

    section { id: "platform", class: "front-page",
      div { class: "max-w-4xl mx-auto",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2 text-center",
          "The long view"
        }
        h2 { class: "text-3xl md:text-4xl font-bold mb-6 text-center", "Why This Platform" }
        p { class: "text-lg leading-relaxed mb-12 text-center text-pretty",
          "Open-source IoT today makes builders choose: assemble a pile of primitives yourself, or pay enterprise prices for the pre-assembled version. PidgeIoT's bet is that one coherent, AGPL-licensed product (identity, config, firmware, telemetry, and alerts designed together in a single codebase) wins the individual developers that the incumbents price out or wear down. Those developers become the small fleets, and the small fleets become the large ones."
        }
        div { class: "space-y-8",
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdServer,
                class: "size-7 stroke-primary",
                title: "Server icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Serverless economics, edge-native by default" }
              p { class: "leading-relaxed text-base-content/80",
                "The backend runs on Cloudflare Workers and Durable Objects, and each device owns its own SQLite-backed object at the edge. No idle servers to pay for, no capacity planning: a fleet of five costs almost nothing to serve, and the same architecture serves a fleet of thousands without a re-platform."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdKeyRound,
                class: "size-7 stroke-primary",
                title: "Key icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Cryptographic identity per device" }
              p { class: "leading-relaxed text-base-content/80",
                "Every device authenticates with its own Ed25519 keypair and a 69-byte binary token: no shared secrets, no JWT overhead, and refreshing a token is revocation, because it overwrites the only key the old one could verify against. Dashboard identity is self-hosted Ory Kratos: user credentials never leave infrastructure we control."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-base-content/10 pt-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdScrollText,
                class: "size-7 stroke-primary",
                title: "Scroll icon",
              }
            }
            div {
              h3 { class: "text-xl font-bold mb-2", "Rust and WebAssembly, end to end" }
              p { class: "leading-relaxed text-base-content/80",
                "The edge router, this dashboard, and the wire types between them are one Rust workspace: the backend compiles to a Worker, the frontend to WebAssembly, and shared structs mean the two cannot drift apart. The protocol itself is the product surface: everything the dashboard does rides the same documented API a device or a script can use."
              }
            }
          }
          div { class: "flex items-start gap-5 border-t border-b border-base-content/10 py-8",
            div { class: "shrink-0 p-3 rounded-2xl bg-base-300 border border-base-content/10",
              Icon {
                icon: LdServer,
                class: "size-7 stroke-secondary",
                title: "Server icon",
              }
            }
            div {
              div { class: "flex items-center gap-3 flex-wrap mb-2",
                h3 { class: "text-xl font-bold", "Next: a user-authored rule engine" }
                MaturityBadge { maturity: Maturity::Planned }
              }
              p { class: "leading-relaxed text-base-content/80",
                "Designed, not yet built: user-written logic running against incoming telemetry at the edge, on Cloudflare Workers for Platforms, the step from device management to a programmable platform."
              }
            }
          }
        }
        div { class: "mt-12 text-center",
          p { class: "leading-relaxed text-base-content/70 max-w-2xl mx-auto mb-6",
            "PidgeIoT is in beta and pre-revenue, and this page says so. There are no customer logos here because we haven't earned them yet; the public repos, the commit history, and the running product are the evidence. If you're evaluating us, read the code."
          }
          div { class: "flex flex-col sm:flex-row justify-center gap-4",
            Link {
              class: "btn btn-outline rounded-full font-bold",
              to: Route::Architecture {},
              "Read the Architecture"
            }
            Link {
              class: "btn btn-outline rounded-full font-bold",
              to: Route::ContactPage {},
              Icon { icon: LdMail, class: "mr-2", title: "Email" }
              "Talk to Us"
            }
          }
        }
      }
    }

    section { id: "home-cta", class: "my-16",
      div {
        class: "max-w-6xl mx-auto rounded-3xl bg-primary px-6 md:px-12 py-16 text-center",
        style: "color:var(--color-primary-content)",
        h2 { class: "text-4xl md:text-5xl font-extrabold tracking-tight", "Send up your first bird" }
        // The design said "the demo flock is already flying"; the public
        // demo is a single allowlisted device.
        p { class: "text-lg md:text-xl mt-4",
          "A real device is already reporting. Ten minutes, no hardware, no card."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-3 mt-8",
          Link {
            class: "btn btn-lg font-bold border-0",
            style: "background:var(--color-primary-content);color:var(--color-primary)",
            to: Route::DemoPage {},
            "Try the live demo"
          }
          Link {
            class: "btn btn-lg btn-outline font-bold",
            style: "background:transparent;border-color:var(--color-primary-content);color:var(--color-primary-content)",
            to: Route::DocumentationPage {},
            "Read the docs"
          }
        }
      }
    }
  }
}
