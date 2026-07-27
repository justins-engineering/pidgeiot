use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCircleCheckBig, LdCode, LdFileText, LdKeyRound, LdPlay, LdRadio, LdRocket,
};

#[component]
pub fn GettingStartedPage() -> Element {
  rsx! {
    section { class: "py-24 md:py-32",
      div { class: "max-w-4xl mx-auto px-4 md:px-8 text-center",
        Icon { icon: LdRocket, class: "w-12 h-12 mx-auto mb-8", title: "Rocket" }
        h1 { class: "text-5xl md:text-6xl font-extrabold tracking-tighter mb-6 text-balance",
          "Getting Started"
        }
        p { class: "text-xl md:text-2xl text-base-content/70 leading-relaxed max-w-3xl mx-auto text-balance",
          "See the whole platform work end to end before you touch any hardware. In about ten minutes you'll have a simulated device reporting real telemetry to your dashboard."
        }
        Link {
          class: "inline-flex items-center gap-1.5 mt-6 text-sm font-semibold text-primary hover:underline",
          to: Route::DemoPage {},
          Icon { icon: LdRadio, class: "size-4", title: "Live" }
          "Just want to see it working? Live demo →"
        }
      }
    }

    // The whole flow, recorded
    section { class: "pb-16",
      div { class: "max-w-3xl mx-auto px-4 md:px-8 text-center",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 font-semibold mb-4",
          "The whole flow in under a minute"
        }
        img {
          class: "w-full max-w-full rounded-2xl border border-base-content/10 shadow-lg mx-auto",
          src: asset!("/assets/images/getting-started-demo.gif"),
          alt: "Terminal recording: cloning pigeon-examples, building the wifi_init sample for Zephyr's native_sim target, and running it -- the console shows the simulated pigeon fetching its shadow and flushing telemetry against a real PidgeIoT backend.",
          width: "796",
          height: "564",
        }
      }
    }

    // The whole flow, recorded
    section { class: "pb-16",
      div { class: "max-w-3xl mx-auto px-4 md:px-8 text-center",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 font-semibold mb-4",
          "The whole flow in under a minute"
        }
        img {
          class: "w-full max-w-full rounded-2xl border border-base-content/10 shadow-lg mx-auto",
          src: asset!("/assets/images/getting-started-demo.gif"),
          alt: "Terminal recording: cloning pigeon-examples, building the wifi_init sample for Zephyr's native_sim target, and running it -- the console shows the simulated pigeon fetching its shadow and flushing telemetry against a real PidgeIoT backend.",
          width: "796",
          height: "564",
        }
      }
    }

    // What you'll have at the end
    section { class: "pb-16",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        div { class: "flex gap-4 items-start bg-primary/10 border border-primary/30 rounded-2xl p-6",
          Icon {
            icon: LdCircleCheckBig,
            class: "size-7 text-primary shrink-0 mt-1",
            title: "Check",
          }
          div {
            h2 { class: "text-xl font-bold mb-2", "What you'll have at the end" }
            p { class: "text-base-content/70 leading-relaxed",
              "A simulated pigeon -- Zephyr's "
              code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "native_sim" }
              " target, running as a plain binary on your own machine, no board or radio involved -- connected to your dashboard, fetching its shadow and reporting telemetry just like a real device would. It's the fastest way to try the whole platform before flashing anything real."
            }
          }
        }
      }
    }

    // Steps 1-5: account through device credentials
    section { class: "pb-16 md:pb-20",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-10 tracking-tight", "Set up in the dashboard" }
        div { class: "space-y-6",
          GsStep {
            number: "1",
            title: "Create your account",
            body: "Register a dashboard account and verify your email. Once it's confirmed you land signed in.",
          }
          GsStep {
            number: "2",
            title: "Create a flock",
            body: "On the Flocks page, click \"Create Flock\" and give it a name. A flock just groups pigeons under one owner.",
          }
          GsStep {
            number: "3",
            title: "Register a pigeon",
            body: "Open the flock and click \"Register Pigeon.\" Give it a name (e.g. \"Simulated Pigeon\"), leave Serial and Board blank, and leave Protocol on its default, HTTPS (REST API) -- then click \"Register Device.\"",
          }
          GsStep {
            number: "4",
            title: "Save the one-time device token",
            body: "A Device Token dialog appears immediately -- this is the only time the token is ever shown, and it can't be retrieved later (refreshing it mints a new keypair and revokes this one). Copy it with the clipboard button, then click \"I've Saved the Token.\"",
          }
          GsStep {
            number: "5",
            title: "Copy the device endpoint",
            body: "Dismissing the token dialog takes you to the pigeon's own detail page. Under \"Connector,\" the Endpoint field (with its own copy button) is the URL you'll bake into the simulator -- it looks like https://api.pidgeiot.com/device/pigeons/<pigeon-id>.",
          }
        }
        div { class: "mt-10",
          Link {
            class: "btn btn-primary rounded-full",
            to: Route::RegisterFlow { flow: None },
            Icon { icon: LdPlay, class: "mr-2", title: "Start now" }
            "Create an Account"
          }
        }
      }
    }

    // Run the simulator
    section { class: "pb-16 md:pb-20",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight",
          "Run the simulator on your machine"
        }
        p { class: "text-base-content/70 leading-relaxed mb-6",
          "This runs "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "wifi_init" }
          ", a sample from the "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "pigeon-examples" }
          " repository, built for Zephyr's "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "native_sim" }
          " board target. You don't need WiFi credentials or a board -- "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "native_sim" }
          " swaps in host-socket networking, and compiles as a plain native binary, so a working host C compiler is the only real prerequisite (no Zephyr SDK cross-toolchain needed for this target). See the "
          a {
            class: "link link-secondary",
            href: "https://github.com/justins-engineering/pigeon-examples",
            target: "_blank",
            rel: "noopener noreferrer",
            "pigeon-examples README"
          }
          " for the full west workspace walkthrough this reuses."
        }

        h3 { class: "text-lg font-bold mb-2", "1. Clone the repo and set up the west workspace" }
        GsCode {
          code: "git clone https://github.com/justins-engineering/pigeon-examples\ncd pigeon-examples\npython3 -m venv .venv && source .venv/bin/activate\npip install west\nwest update",
        }
        p { class: "text-sm text-base-content/60 mb-6",
          "\"west update\" fetches the Zephyr sources -- a few hundred MB, one time only."
        }

        h3 { class: "text-lg font-bold mb-2", "2. Add your device credentials" }
        p { class: "text-base-content/70 leading-relaxed mb-2",
          "Paste in the endpoint and token from steps 4-5 above:"
        }
        GsCode {
          code: "cat > samples/wifi_init/prj.local.conf <<'EOF'\nCONFIG_PIGEON_ENDPOINT=\"https://api.pidgeiot.com/device/pigeons/<pigeon-id>\"\nCONFIG_PIGEON_TOKEN=\"<device-bearer-token>\"\nEOF",
        }
        p { class: "text-sm text-base-content/60 mb-6",
          "This file is git-ignored -- these are real device secrets, never commit them."
        }

        h3 { class: "text-lg font-bold mb-2", "3. Build and run" }
        GsCode {
          code: "west build -d build_wifi_native samples/wifi_init -b native_sim/native/64\n./build_wifi_native/zephyr/zephyr.exe",
        }
      }
    }

    // What you should see
    section { class: "pb-16 md:pb-20",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight", "What you should see" }
        p { class: "text-base-content/70 leading-relaxed mb-4",
          "Within about a second, the console prints something like:"
        }
        GsCode {
          code: "Network connectivity established\nShadow fetched: target_version=0 current_version=0\nFlushed shadow param: uptime_s=1",
        }
        p { class: "text-base-content/70 leading-relaxed mb-4",
          "That last line is the simulator reporting its own uptime as a telemetry value. Leave it running -- it keeps flushing on an interval."
        }
        p { class: "text-base-content/70 leading-relaxed mb-2",
          "Back in the dashboard, on the pigeon's detail page:"
        }
        ul { class: "list-disc ml-6 space-y-2 text-base-content/70 leading-relaxed",
          li { "The connection badge next to the pigeon's name flips to online once it's reported in." }
          li {
            "The Shadow section's "
            strong { "Current Config" }
            " version catches up to "
            strong { "Target Config" }
            " -- confirmation the device applied it."
          }
          li {
            "Under Telemetry Graphs, click \"Add Graph\" and pick the "
            code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "uptime_s" }
            " key to watch it climb live."
          }
        }
      }
    }

    // Where to go next
    section { class: "pb-24 md:pb-32",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-10 tracking-tight", "Where to go next" }
        div { class: "grid grid-cols-1 md:grid-cols-3 gap-6",
          GsLink {
            icon: rsx! {
              Icon { icon: LdCode, class: "size-7 stroke-primary", title: "Code" }
            },
            title: "Flash real hardware",
            body: "Board-level samples for ESP32-C6 and Nordic nRF91 boards in the same pigeon-examples repository.",
            href: Some("https://github.com/justins-engineering/pigeon-examples"),
            route: None,
          }
          GsLink {
            icon: rsx! {
              Icon { icon: LdKeyRound, class: "size-7 stroke-secondary", title: "Key" }
            },
            title: "Explore shadows & alerts",
            body: "Push configuration to a device and define alerts that evaluate against it -- see the full Documentation page.",
            href: None,
            route: Some(Route::DocumentationPage {}),
          }
          GsLink {
            icon: rsx! {
              Icon { icon: LdFileText, class: "size-7 stroke-primary", title: "File" }
            },
            title: "API Reference",
            body: "Every dashboard and device route this walkthrough touched, and everything it didn't.",
            href: None,
            route: Some(Route::ApiReferencePage {}),
          }
        }
      }
    }
  }
}

#[component]
fn GsStep(number: &'static str, title: &'static str, body: &'static str) -> Element {
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

// Plain rsx pre/code rather than dangerous_inner_html + pulldown-cmark (see
// api_reference.rs/open_source.rs) -- this page's code blocks are real,
// hand-written commands, not rendered markdown, so there's no HTML source to
// parse. Styling deliberately matches those pages' `#api-md pre`/`code`
// rules (bg-base-300, rounded, horizontal-only scroll) for visual
// consistency with the rest of the docs surface.
#[component]
fn GsCode(code: &'static str) -> Element {
  rsx! {
    pre { class: "bg-base-300 text-base-content rounded-xl p-4 md:p-5 overflow-x-auto text-xs md:text-sm font-mono leading-relaxed mb-2",
      code { "{code}" }
    }
  }
}

#[component]
fn GsLink(
  icon: Element,
  title: &'static str,
  body: &'static str,
  href: Option<&'static str>,
  route: Option<Route>,
) -> Element {
  let inner = rsx! {
    div { class: "shrink-0 mt-1", {icon} }
    div {
      h3 { class: "text-lg font-bold", "{title}" }
      p { class: "text-base-content/70 leading-relaxed mt-1 text-sm", "{body}" }
    }
  };
  rsx! {
    div { class: "p-6 rounded-2xl bg-base-300/50 border border-base-content/10 hover:border-primary/40 transition-colors",
      if let Some(r) = route {
        Link { class: "flex gap-4 items-start", to: r, {inner} }
      } else if let Some(h) = href {
        a {
          class: "flex gap-4 items-start",
          href: h,
          target: "_blank",
          rel: "noopener noreferrer",
          {inner}
        }
      }
    }
  }
}
