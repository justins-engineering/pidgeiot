use crate::Route;
use crate::components::FeedbackForm;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdDatabase, LdHardDriveDownload, LdMapPin, LdPlay, LdScale, LdThermometer,
};

#[component]
pub fn UseCasesPage() -> Element {
  let mut feedback = use_context::<FeedbackForm>();

  rsx! {
    section { class: "py-24 md:py-32 text-center",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "What People Actually Build On It"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-3xl mx-auto text-balance",
          "Five shapes of fleet, all served by the same shadow model and the same device library. These are example builds — illustrations of what the platform supports, not customer case studies."
        }
      }
    }

    section { class: "pb-16 md:pb-24",
      div { class: "max-w-6xl mx-auto px-4 md:px-8 space-y-20",

        UseCase {
          icon: rsx! {
            Icon { icon: LdMapPin, class: "size-8 stroke-primary", title: "Map pin icon" }
          },
          eyebrow: "Asset tracking",
          title: "Things That Don't Stay Put",
          body: "Report GPS fixes as ordinary telemetry keys and the pigeon's detail page draws the track for any time range you pick — start marker, pulsing live position, nearest-point hover readout, rendered as self-contained SVG with no map-tile service in the loop. The key names matter here: the track widget draws on gps_lat and gps_lon specifically, with gps_speed_mps alongside them. Nothing else about the device is special; it's the same flat key/value report every other pigeon sends.",
          keys: Some("gps_lat · gps_lon · gps_speed_mps"),
        }

        UseCase {
          icon: rsx! {
            Icon { icon: LdThermometer, class: "size-8 stroke-primary", title: "Thermometer icon" }
          },
          eyebrow: "Environmental monitoring",
          title: "Readings You Need to Hear About at 3am",
          body: "Cold chains, greenhouses, server rooms — anywhere the reading matters less than the excursion. Define a condition once and the platform emails you when it trips: a key crossing a threshold, or a value jumping further between two reports than physics reasonably allows. Threshold conditions are evaluated the moment a report lands. The one that catches the worst failures — a device that has simply gone quiet — runs on a scheduled sweep instead, because nothing arriving is not an event you can observe at ingest.",
          keys: None,
        }

        UseCase {
          icon: rsx! {
            Icon {
              icon: LdHardDriveDownload,
              class: "size-8 stroke-primary",
              title: "Download icon",
            }
          },
          eyebrow: "Deployed equipment",
          title: "Hardware You Can't Drive Out To",
          body: "Firmware images are content-addressed by SHA-256, catalogued per flock, and assigned to a device through the same shadow you use for config. Downloads use Range requests straight into the MCUboot secondary slot, so a device on a marginal cellular link resumes a large image instead of starting it over. Every image and every pigeon carries a board tag, and an assignment is refused outright unless the two match — a fail-closed check that exists because of a real incident, not a hypothetical one.",
          keys: None,
        }

        UseCase {
          icon: rsx! {
            Icon { icon: LdDatabase, class: "size-8 stroke-primary", title: "Database icon" }
          },
          eyebrow: "Bring your own database",
          title: "When the Time Series Has to Live Somewhere Else",
          body: "Point a pigeon's telemetry endpoint at your own InfluxDB-line-protocol-compatible database — InfluxDB, GreptimeDB, and friends — and every report is forwarded straight there as line protocol instead of into our history store. Worth being precise about what that does and doesn't mean: we still keep the latest value of each key, because that's what the dashboard renders and what alerts evaluate against. What we stop keeping is the history. The time series accumulates in your database, not ours.",
          keys: None,
        }

        UseCase {
          icon: rsx! {
            Icon { icon: LdScale, class: "size-8 stroke-primary", title: "Scale icon" }
          },
          eyebrow: "Small fleets and self-hosting",
          title: "Five Devices Shouldn't Cost Like Five Thousand",
          body: "Plenty of platforms price and shape themselves around fleets far larger than the one you have. PidgeIoT is one product with one feature set — there is no tier where shadows or alerts or OTA get switched off. And because the whole stack is AGPL-3.0 and developed in the open, the edge router, the dashboard and the Zephyr device library are all there to read, audit, or run yourself.",
          keys: None,
        }
      }
    }

    section { class: "pb-24 md:pb-32 text-center",
      div { class: "max-w-3xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight",
          "Something here close to what you're building?"
        }
        p { class: "text-lg text-base-content/70 mb-8 leading-relaxed",
          "Start with a simulated device on your own machine, or tell us what you're working on — it genuinely shapes what gets built next."
        }
        div { class: "flex flex-col sm:flex-row gap-4 justify-center items-center",
          Link {
            class: "btn btn-primary btn-lg px-10 rounded-full",
            to: Route::GettingStartedPage {},
            Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
            "Try It Free"
          }
          button {
            r#type: "button",
            class: "btn btn-ghost btn-lg px-10 rounded-full",
            onclick: move |_| feedback.0.set(true),
            "Talk to us"
          }
        }
      }
    }
  }
}

#[component]
fn UseCase(
  icon: Element,
  eyebrow: &'static str,
  title: &'static str,
  body: &'static str,
  keys: Option<&'static str>,
) -> Element {
  rsx! {
    div { class: "flex flex-col md:flex-row gap-8 items-start",
      div { class: "shrink-0 p-4 rounded-2xl bg-base-300 border border-base-content/10",
        {icon}
      }
      div { class: "min-w-0",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 mb-2", "{eyebrow}" }
        h2 { class: "text-2xl md:text-3xl font-bold mb-3", "{title}" }
        p { class: "text-lg text-base-content/70 leading-relaxed max-w-3xl", "{body}" }
        if let Some(k) = keys {
          div { class: "mt-4 overflow-x-auto",
            code { class: "text-sm px-3 py-2 rounded-lg bg-base-300 border border-base-content/10 whitespace-nowrap inline-block",
              "{k}"
            }
          }
        }
      }
    }
  }
}
