use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdCircleCheckBig, LdCode, LdFileText, LdKeyRound, LdMessagesSquare, LdPlay, LdRadio,
};

#[component]
pub fn GettingStartedPage() -> Element {
  rsx! {
    // Header matches the other public pages (eyebrow, left-aligned h1 on
    // base-200) rather than the centred hero this page used to carry, so the
    // marketing set reads as one thing.
    section { class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4",
          "Getting started"
        }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "Ten minutes to first telemetry. No hardware required."
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-3xl text-base-content/80 text-pretty",
          "Set it up in the dashboard, run a simulated device on your own machine, and swap in a real board whenever one lands on your desk."
        }
        Link {
          class: "inline-flex items-center gap-1.5 mt-6 text-sm font-semibold text-primary hover:underline",
          to: Route::DemoPage {},
          Icon { icon: LdRadio, class: "size-4", title: "Live" }
          "Just want to see it working? Live demo →"
        }
      }
    }

    section { class: "pb-16",
      div { class: "max-w-3xl mx-auto px-4 md:px-8 text-center",
        p { class: "text-sm uppercase tracking-wide text-base-content/50 font-semibold mb-4",
          "The whole flow in under a minute"
        }
        // Click-to-play <video> instead of an autoplaying GIF: as an <img>
        // the GIF was this page's LCP element, gating LCP on its full
        // download (a 1.48MB GIF costs ~9s on slow 4G). Autoplaying video
        // doesn't fix it either -- Chrome takes the first frame, not the
        // poster, as the LCP candidate. Click-to-play makes the poster image
        // the LCP candidate instead; the ~830KB webm only loads on click.
        GettingStartedRecording {}
      }
    }

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
              code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
                "native_sim"
              }
              " target, running as a plain binary on your own machine, no board or radio involved -- connected to your dashboard, fetching its shadow and reporting telemetry just like a real device would. It's the fastest way to try the whole platform before flashing anything real."
            }
          }
        }
      }
    }

    section { class: "pb-16 md:pb-20",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-10 tracking-tight",
          "Set up in the dashboard"
        }
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

    section { class: "pb-16 md:pb-20",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight",
          "Run the simulator on your machine"
        }
        p { class: "text-base-content/70 leading-relaxed mb-6",
          "This runs "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
            "wifi_init"
          }
          ", a sample from the "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
            "pigeon-examples"
          }
          " repository, built for Zephyr's "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
            "native_sim"
          }
          " board target. You don't need WiFi credentials or a board -- "
          code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
            "native_sim"
          }
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

        h3 { class: "text-lg font-bold mb-2",
          "1. Clone the repo and set up the west workspace"
        }
        GsCode { code: "git clone https://github.com/justins-engineering/pigeon-examples\ncd pigeon-examples\npython3 -m venv .venv && source .venv/bin/activate\npip install west\nwest update" }
        p { class: "text-sm text-base-content/60 mb-6",
          "\"west update\" fetches the Zephyr sources -- a few hundred MB, one time only."
        }

        h3 { class: "text-lg font-bold mb-2", "2. Add your device credentials" }
        p { class: "text-base-content/70 leading-relaxed mb-2",
          "Paste in the endpoint and token from steps 4-5 above:"
        }
        GsCode { code: "cat > samples/wifi_init/prj.local.conf <<'EOF'\nCONFIG_PIGEON_ENDPOINT=\"https://api.pidgeiot.com/device/pigeons/<pigeon-id>\"\nCONFIG_PIGEON_TOKEN=\"<device-bearer-token>\"\nEOF" }
        p { class: "text-sm text-base-content/60 mb-6",
          "This file is git-ignored -- these are real device secrets, never commit them."
        }

        h3 { class: "text-lg font-bold mb-2", "3. Build and run" }
        GsCode { code: "west build -d build_wifi_native samples/wifi_init -b native_sim/native/64\n./build_wifi_native/zephyr/zephyr.exe" }
      }
    }

    section { class: "pb-16 md:pb-20",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-4 tracking-tight",
          "What you should see"
        }
        p { class: "text-base-content/70 leading-relaxed mb-4",
          "Within about a second, the console prints something like:"
        }
        GsCode { code: "WARNING: Using a test - not safe - entropy source\n*** Pigeon v4.4.1 ***\n[00:00:00.000,000] <inf> wifi_connection_manager: Bringing network interface up\n[00:00:00.000,000] <inf> wifi_connection_manager: Connecting to the network\n[00:00:01.010,000] <inf> wifi_connection_manager: Network connectivity established and IP address assigned\n[00:00:01.010,000] <inf> pigeon: Transport mapped to secure HTTPS edge pipeline: https://api.pidgeiot.com/device/pigeons/<pigeon-id>\n[00:00:01.510,004] <inf> shadow: Shadow fetched: target_version=0 current_version=0\n[00:00:01.870,008] <inf> pigeon: Flushed shadow param: uptime_s=1\n[00:00:01.870,008] <inf> shadow: Next shadow poll in 60 s" }
        p { class: "text-sm text-base-content/60 mb-6",
          "The entropy warning is expected: native_sim has no hardware random source, so Zephyr simulates one and says so loudly. Real boards use their own TRNG."
        }
        p { class: "text-base-content/70 leading-relaxed mb-4",
          "That last line is the simulator reporting its own uptime as a telemetry value. Leave it running -- it keeps flushing on an interval."
        }
        p { class: "text-base-content/70 leading-relaxed mb-2",
          "Back in the dashboard, on the pigeon's detail page:"
        }
        ul { class: "list-disc ml-6 space-y-2 text-base-content/70 leading-relaxed",
          li {
            "The connection badge next to the pigeon's name flips to online once it's reported in."
          }
          li {
            "Try the config loop: click "
            strong { "Edit Shadow" }
            " and set "
            code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
              "{{\"telemetry_interval\": 30}}"
            }
            ". Within one poll the simulator's console reads \"Next shadow poll in 30 s\", the reports speed up, and the Shadow section's "
            strong { "Current" }
            " version catches up to "
            strong { "Target" }
            " -- the full config round trip. (A device only adopts keys its firmware understands -- this sample knows "
            code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "log" }
            ", "
            code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
              "telemetry_interval"
            }
            ", and "
            code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded", "reboot" }
            ". Anything else stays visible in Target but won't appear in Current, which is exactly how you spot a key your firmware ignored.)"
          }
          li {
            "Every numeric value the device reports becomes graphable under Telemetry Graphs. "
            code { class: "font-mono text-sm bg-base-300 px-1.5 py-0.5 rounded",
              "uptime_s"
            }
            " is the only key this sample sends -- it plots as a dutiful climbing staircase, which proves the pipeline but wins no awards. Graphs get good when you report real sensor values ("
            Link { class: "link link-secondary", to: Route::DemoPage {}, "see the live demo" }
            ")."
          }
        }
      }
    }

    section { class: "pb-24 md:pb-32",
      div { class: "max-w-4xl mx-auto px-4 md:px-8",
        h2 { class: "text-3xl md:text-4xl font-bold mb-10 tracking-tight",
          "Where to go next"
        }
        div { class: "grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-6",
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
              Icon { icon: LdMessagesSquare, class: "size-7 stroke-secondary", title: "Chat" }
            },
            title: "Stuck?",
            body: "Discord is where the maintainers actually are. Bug reports go to the issue tracker and get answered by the people who wrote the line.",
            href: Some("https://discord.gg/W2vjtpeP"),
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

/// The terminal recording, click-to-play. Renders as a still frame (the
/// prerendered/SSG state too) with a play-button overlay; the actual video
/// element -- and its ~830KB webm -- only exists after the visitor presses
/// play. `autoplay` on the swapped-in element is fine LCP-wise: LCP
/// candidates stop at the first user interaction, and mounting it fresh
/// from a click means playback starts immediately without a second tap.
#[component]
fn GettingStartedRecording() -> Element {
  let mut playing = use_signal(|| false);
  rsx! {
    div { class: "relative w-full max-w-full rounded-2xl border border-base-content/10 shadow-lg mx-auto overflow-hidden",
      if playing() {
        video {
          class: "w-full block",
          autoplay: true,
          muted: true,
          r#loop: true,
          playsinline: true,
          controls: true,
          width: 796,
          height: 564,
          aria_label: "Terminal recording: cloning pigeon-examples, building the wifi_init sample for Zephyr's native_sim target, and running it -- the console shows the simulated pigeon fetching its shadow and flushing telemetry against a real PidgeIoT backend.",
          source {
            src: asset!("/assets/images/getting-started-demo.webm"),
            r#type: "video/webm",
          }
          source {
            src: asset!("/assets/images/getting-started-demo.mp4"),
            r#type: "video/mp4",
          }
        }
      } else {
        img {
          class: "w-full block",
          // Deliberately NOT asset!(): dx's image pipeline re-encodes webp
          // assets and bloats this 60KB still to 218KB. Served verbatim
          // from fancier/public/ instead, same as og.png and favicon.ico.
          // This still is likely the page's LCP element, so its size
          // directly moves mobile LCP -- keep it small.
          src: "/getting-started-poster.webp",
          alt: "Terminal recording still: building and running the wifi_init sample for Zephyr's native_sim target from pigeon-examples.",
          width: "796",
          height: "564",
        }
        button {
          class: "absolute inset-0 flex items-center justify-center bg-base-300/20 hover:bg-base-300/30 transition-colors cursor-pointer",
          aria_label: "Play the recording",
          onclick: move |_| playing.set(true),
          span { class: "btn btn-circle btn-primary btn-lg shadow-lg",
            Icon { icon: LdPlay, class: "size-7", title: "Play" }
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
