use dioxus::prelude::*;

#[component]
pub fn RouteStops() -> Element {
  rsx! {
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
            body: "Flash the Zephyr library. Registering the device mints its keypair and hands back a 69-byte token — the private half signs that token and is discarded, so only the public key is ever stored. From there it speaks CoAP over DTLS or plain HTTPS, whatever the modem can afford.",
            mock: rsx! {
              div { class: "rounded-xl bg-base-200 border border-base-300 p-4 font-mono text-xs leading-relaxed text-base-content/70 overflow-x-auto",
                p { class: "whitespace-nowrap", "dashboard → Register Pigeon" }
                p { class: "whitespace-nowrap text-success", "✓ keypair minted, public key stored" }
                p { class: "whitespace-nowrap text-success", "✓ token issued (69 B) — shown once" }
              }
            },
          }

          Stop {
            number: "2",
            title: "The edge",
            body: "Each device owns a small object on Cloudflare's network — its shadow, its permissions, its credentials. Nothing to provision, nothing to patch, close to wherever it wakes up.",
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
            body: "The dashboard above: graphs, GPS tracks, firmware rollouts, remote logs and alerts by email. Or bypass it — the API the dashboard uses is the API you get.",
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
