// The public "talk to us" form, which every Contact / Talk to us link on
// the site opens.
//
// The route is `/contact/` with NO query-param prop, deliberately. A
// `?:about` prop would make the route Display as `/contact/?`, and that
// string is the key page-meta.json, the sitemap, `_headers` and
// `run_worker_first` all join on -- so the funnel context is read from the
// address bar via `url_query_param` instead, which is what a prerendered
// page has to do anyway (see helpers/url_query.rs: SSG bakes the route a
// page was rendered AS into its hydration payload, query string and all
// dropped). Links that carry context are therefore plain `a` hrefs rather
// than `Link`s; the destination is prerendered, so the full-page load
// paints immediately.
use crate::Route;
use crate::api::contact::ContactSendError;
use crate::config::TURNSTILE_SITE_KEY;
use crate::helpers::url_query_param;
use capsules::{
  ContactFleetSize, ContactRequest, MAX_CONTACT_COMPANY_BYTES, MAX_CONTACT_EMAIL_BYTES,
  MAX_CONTACT_MESSAGE_BYTES, MAX_CONTACT_NAME_BYTES,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{
  LdBookOpen, LdCircleCheck, LdGithub, LdMail, LdMessageSquare,
};
use serde::Deserialize;

/// Loads Turnstile's script once and renders the widget into the form's
/// container, reporting back through `dioxus.send`. Explicit rendering
/// (`?render=explicit`) rather than the script's default DOM scan: the
/// container only exists once the form is on screen, and nothing here may
/// touch the prerendered markup before hydration has adopted it.
const TURNSTILE_MOUNT_JS: &str = r#"
(async () => {
  const container = document.getElementById("contact_turnstile");
  if (!container) { return; }
  const loaded = new Promise((resolve, reject) => {
    if (window.turnstile) { resolve(); return; }
    let script = document.getElementById("cf-turnstile-api");
    if (!script) {
      script = document.createElement("script");
      script.id = "cf-turnstile-api";
      script.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
      script.async = true;
      document.head.appendChild(script);
    }
    script.addEventListener("load", () => resolve());
    script.addEventListener("error", () => reject(new Error("turnstile script failed to load")));
  });
  try { await loaded; } catch (e) { dioxus.send({ kind: "error" }); return; }
  if (!window.turnstile) { dioxus.send({ kind: "error" }); return; }
  const theme = document.documentElement.getAttribute("data-theme");
  try {
    window.__pidgeiotTurnstile = turnstile.render(container, {
      sitekey: __SITE_KEY__,
      theme: theme === "dark" || theme === "light" ? theme : "auto",
      callback: (token) => dioxus.send({ kind: "token", token: token }),
      "expired-callback": () => dioxus.send({ kind: "expired" }),
      "error-callback": () => { dioxus.send({ kind: "error" }); return true; },
    });
  } catch (e) {
    dioxus.send({ kind: "error" });
  }
})();
"#;

/// Asks the widget for a fresh token. A token is single-use, so this
/// follows any send that may have spent the one in hand.
const TURNSTILE_RESET_JS: &str = r#"
if (window.turnstile && window.__pidgeiotTurnstile != null) {
  try { turnstile.reset(window.__pidgeiotTurnstile); } catch (e) {}
}
"#;

/// Tears the widget down with the page, so a client-side return to the
/// form renders a new one instead of leaking the old iframe.
const TURNSTILE_REMOVE_JS: &str = r#"
if (window.turnstile && window.__pidgeiotTurnstile != null) {
  try { turnstile.remove(window.__pidgeiotTurnstile); } catch (e) {}
  window.__pidgeiotTurnstile = null;
}
"#;

/// What the widget reported: `token` carries one, `expired` and `error`
/// withdraw whatever was in hand.
#[derive(Deserialize)]
struct TurnstileEvent {
  kind: String,
  #[serde(default)]
  token: Option<String>,
}

/// Whether a failed send may have spent the Turnstile token it carried.
///
/// The route checks the body (400/413) and the limiter (429) before it
/// asks Cloudflare, so those leave the token unspent and the visitor can
/// fix the field and resend without another challenge. Anything else -- a
/// 403, a 5xx from after verification, or no response at all -- may have
/// consumed it, and resending a spent token can only fail again.
fn challenge_spent(status: Option<u16>) -> bool {
  !matches!(status, Some(400 | 413 | 429))
}

/// Wall-clock milliseconds, or zero where there is no clock to read.
///
/// The SSG prerender compiles this component for a native target, where
/// `js_sys`'s imports are stubs that panic if called. Nothing here runs
/// during the prerender -- the only caller is an event handler -- but the
/// guard makes that a property of the code rather than of the render
/// order.
fn now_ms() -> f64 {
  #[cfg(target_arch = "wasm32")]
  {
    js_sys::Date::now()
  }
  #[cfg(not(target_arch = "wasm32"))]
  {
    0.0
  }
}

/// Which link opened the form, if it said so. `fleet` comes from the
/// pricing page's Fleet tier, the one CTA whose sender is self-selecting
/// for a fleet-sized conversation.
fn preselected_fleet_size(about: Option<&str>) -> Option<ContactFleetSize> {
  match about {
    Some("fleet") => Some(ContactFleetSize::From1500To10000),
    _ => None,
  }
}

#[component]
pub fn ContactPage() -> Element {
  let mut name = use_signal(String::new);
  let mut email = use_signal(String::new);
  let mut company = use_signal(String::new);
  let mut message = use_signal(String::new);
  let mut fleet_size = use_signal(String::new);

  // The funnel context arrives as a state change AFTER mount, never as
  // part of the first render. A prerendered page hydrates by adopting the
  // markup it was served: a value that differs only on the client changes
  // nothing on screen, because the initial render is not re-run against
  // it: the address bar can plainly say `?about=fleet` while the page goes
  // on rendering the generic copy. A `use_future` does not run during the
  // synchronous prerender, so what it sets is a real reactive update that
  // patches the DOM.
  let mut about = use_signal(|| None::<String>);
  use_future(move || async move {
    let Some(value) = url_query_param("about") else {
      return;
    };
    // A default, not an override: `peek` rather than `read` so this never
    // subscribes, and an answer the visitor already picked wins over the
    // link's guess.
    if let Some(size) = preselected_fleet_size(Some(value.as_str()))
      && fleet_size.peek().is_empty()
    {
      fleet_size.set(size.wire().to_string());
    }
    about.set(Some(value));
  });
  let about_is_fleet = about().as_deref() == Some("fleet");
  // The honeypot's bound value. Always empty from a real browser, since
  // the field is off-screen, aria-hidden and out of the tab order.
  let mut website = use_signal(String::new);

  // Stamped by the form's own onmounted, which never fires during the
  // prerender -- so a submission that somehow arrives without it reads as
  // zero elapsed and the server refuses it, which is the safe direction.
  let mut mounted_at = use_signal(|| 0.0_f64);

  let mut is_sending = use_signal(|| false);
  let mut sent = use_signal(|| false);
  let mut error_msg = use_signal(|| None::<String>);

  // The widget's one-time token. `None` until Turnstile's callback fires
  // (which keeps the send button disabled) and again whenever the token
  // in hand may have been spent or has expired.
  let mut turnstile_token = use_signal(|| None::<String>);
  let mut turnstile_notice = use_signal(|| None::<&'static str>);

  // Mounted from a future, never from the first render: the prerendered
  // page must carry no third-party script, and hydration adopts the served
  // markup, so the container ships empty and is filled here.
  use_future(move || async move {
    let site_key = serde_json::to_string(TURNSTILE_SITE_KEY).unwrap_or_else(|_| "\"\"".to_string());
    let mut widget = document::eval(&TURNSTILE_MOUNT_JS.replace("__SITE_KEY__", &site_key));
    while let Ok(event) = widget.recv::<TurnstileEvent>().await {
      match event.kind.as_str() {
        "token" => {
          turnstile_notice.set(None);
          turnstile_token.set(event.token);
        }
        "expired" => turnstile_token.set(None),
        _ => {
          turnstile_token.set(None);
          turnstile_notice.set(Some(
            "The verification widget didn't load. Reload the page, or email us at code@jes.contact.",
          ));
        }
      }
    }
  });
  use_drop(|| {
    document::eval(TURNSTILE_REMOVE_JS);
  });

  let can_submit = !is_sending()
    && !name.read().trim().is_empty()
    && !email.read().trim().is_empty()
    && !message.read().trim().is_empty()
    && turnstile_token.read().is_some();

  let submit = move |_| {
    if !can_submit {
      return;
    }
    let elapsed = (now_ms() - mounted_at()).max(0.0) as u32;
    let request = ContactRequest {
      name: name.read().trim().to_string(),
      email: email.read().trim().to_string(),
      message: message.read().trim().to_string(),
      company: {
        let value = company.read().trim().to_string();
        (!value.is_empty()).then_some(value)
      },
      fleet_size: ContactFleetSize::from_wire(&fleet_size.read()),
      about: about(),
      website: Some(website.read().clone()),
      elapsed_ms: Some(elapsed),
      turnstile_token: turnstile_token(),
    };

    spawn(async move {
      is_sending.set(true);
      error_msg.set(None);
      // Validated here as well as server-side, against the same shared
      // function, so a field error is a rendered sentence instead of a
      // round trip. The server's copy is what the user reads either way.
      let outcome = match capsules::contact::validate(&request) {
        Err(rejection) if rejection.status() >= 400 => Err(ContactSendError {
          status: Some(rejection.status()),
          message: rejection.message().to_string(),
        }),
        _ => crate::api::contact::send(&request).await,
      };
      match outcome {
        Ok(()) => sent.set(true),
        Err(err) => {
          if challenge_spent(err.status) {
            turnstile_token.set(None);
            document::eval(TURNSTILE_RESET_JS);
          }
          error_msg.set(Some(err.message));
        }
      }
      is_sending.set(false);
    });
  };

  rsx! {
    section {
      id: "contact-hero",
      class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Contact" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          if about_is_fleet {
            "Let's scope your fleet."
          } else {
            "Tell us what you're building."
          }
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-2xl text-base-content/80 text-pretty",
          if about_is_fleet {
            "Fleet pricing is a conversation, not a checkout button. Tell us roughly how many devices, what they report, and what has to be true before you could ship. You'll get a straight answer about whether we fit."
          } else {
            "Questions about the platform, a fleet too big for the pricing page, or something that should work and doesn't: it all reaches the same small team."
          }
        }
      }
    }

    section { id: "contact-form", class: "px-4 md:px-10 py-14",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 lg:grid-cols-5 gap-10 lg:gap-16",
        div { class: "lg:col-span-3",
          if sent() {
            // Replaces the form entirely rather than clearing it: the
            // message is gone from the page because it is somewhere else
            // now, which is the reassurance being offered.
            div {
              class: "rounded-2xl border border-base-300 bg-base-100 p-8 md:p-10 flex flex-col items-start gap-4",
              role: "status",
              Icon { icon: LdCircleCheck, class: "size-12 text-success", title: "Sent" }
              h2 { class: "text-2xl font-bold", "Message sent" }
              p { class: "text-base-content/80 leading-relaxed",
                "It's with us. We're a small team in Massachusetts, so a reply comes from a person and usually inside a business day."
              }
              div { class: "flex flex-col sm:flex-row gap-3 mt-2",
                Link { class: "btn btn-primary font-bold", to: Route::Index {}, "Back to the site" }
                Link { class: "btn btn-outline font-bold", to: Route::DemoPage {}, "Try the live demo" }
              }
            }
          } else {
            form {
              class: "rounded-2xl border border-base-300 bg-base-100 p-6 md:p-8",
              // Timing starts when the form is on screen, not when the
              // page is. Never fires during the prerender.
              onmounted: move |_| mounted_at.set(now_ms()),
              onsubmit: move |e| {
                  e.prevent_default();
                  submit(());
              },
              fieldset { class: "fieldset",
                label { class: "fieldset-legend text-xs font-semibold", r#for: "contact_name",
                  "Your name"
                }
                input {
                  class: "input input-bordered w-full",
                  id: "contact_name",
                  r#type: "text",
                  autocomplete: "name",
                  required: true,
                  placeholder: "Dana Okafor",
                  // Chars, not bytes -- multibyte input can still exceed
                  // the server's byte cap, which then answers with its own
                  // copy, so this is a convenience rail, not enforcement.
                  maxlength: MAX_CONTACT_NAME_BYTES as i64,
                  disabled: is_sending(),
                  value: "{name}",
                  oninput: move |e| name.set(e.value()),
                }

                label { class: "fieldset-legend text-xs font-semibold", r#for: "contact_email",
                  "Work email"
                }
                input {
                  class: "input input-bordered w-full",
                  id: "contact_email",
                  r#type: "email",
                  autocomplete: "email",
                  required: true,
                  placeholder: "you@example.com",
                  maxlength: MAX_CONTACT_EMAIL_BYTES as i64,
                  disabled: is_sending(),
                  value: "{email}",
                  oninput: move |e| email.set(e.value()),
                }

                label { class: "fieldset-legend text-xs font-semibold", r#for: "contact_company",
                  "Company "
                  span { class: "font-normal opacity-60", "(optional)" }
                }
                input {
                  class: "input input-bordered w-full",
                  id: "contact_company",
                  r#type: "text",
                  autocomplete: "organization",
                  placeholder: "Meterworks",
                  maxlength: MAX_CONTACT_COMPANY_BYTES as i64,
                  disabled: is_sending(),
                  value: "{company}",
                  oninput: move |e| company.set(e.value()),
                }

                label { class: "fieldset-legend text-xs font-semibold", r#for: "contact_fleet_size",
                  "Fleet size "
                  span { class: "font-normal opacity-60", "(optional)" }
                }
                select {
                  class: "select select-bordered w-full",
                  id: "contact_fleet_size",
                  disabled: is_sending(),
                  value: "{fleet_size}",
                  onchange: move |e| fleet_size.set(e.value()),
                  option { value: "", "Prefer not to say" }
                  for size in ContactFleetSize::ALL {
                    option { value: size.wire(), "{size.label()}" }
                  }
                }

                label { class: "fieldset-legend text-xs font-semibold", r#for: "contact_message",
                  "What do you need?"
                }
                textarea {
                  class: "textarea textarea-bordered w-full h-40",
                  id: "contact_message",
                  required: true,
                  placeholder: if about_is_fleet {
                      "What the devices report, how they connect, and what would have to be true before you could ship."
                  } else {
                      "What you're building, and what you'd need from us."
                  },
                  maxlength: MAX_CONTACT_MESSAGE_BYTES as i64,
                  disabled: is_sending(),
                  value: "{message}",
                  oninput: move |e| message.set(e.value()),
                }
              }

              // Honeypot. Off-screen rather than `display: none`, since a
              // form-filling script that skips hidden fields is exactly
              // the one worth catching. Never visible, never focusable,
              // and hidden from assistive technology, so a real
              // submission always leaves it empty.
              div {
                class: "absolute w-px h-px overflow-hidden",
                style: "left:-9999px;top:auto",
                aria_hidden: "true",
                label { r#for: "contact_website", "Website" }
                input {
                  id: "contact_website",
                  name: "website",
                  r#type: "text",
                  tabindex: "-1",
                  autocomplete: "off",
                  value: "{website}",
                  oninput: move |e| website.set(e.value()),
                }
              }

              // Turnstile renders into this once the form is on screen; the
              // prerendered page ships it empty. Reserved at the Managed
              // widget's height so its arrival does not shift the button.
              div { id: "contact_turnstile", class: "mt-4 min-h-[65px]" }
              if let Some(notice) = turnstile_notice() {
                div { class: "alert alert-warning mt-4 text-sm", role: "alert", "{notice}" }
              }

              if let Some(err) = error_msg.read().as_ref() {
                div { class: "alert alert-error mt-4 text-sm", role: "alert", "{err}" }
              }

              div { class: "mt-6 flex flex-col sm:flex-row sm:items-center gap-4",
                button {
                  class: "btn btn-primary font-bold",
                  r#type: "submit",
                  disabled: !can_submit,
                  if is_sending() {
                    span { class: "loading loading-spinner loading-sm" }
                    "Sending"
                  } else {
                    "Send message"
                  }
                }
                p { class: "text-xs text-base-content/60 leading-relaxed",
                  "We use your address to reply and nothing else. No list, no sequence, no third party. See the "
                  Link { class: "link link-secondary", to: Route::PrivacyPage {}, "privacy policy" }
                  "."
                }
              }
            }
          }
        }

        aside { class: "lg:col-span-2 flex flex-col gap-6",
          div { class: "rounded-2xl border border-base-300 bg-base-100 p-6",
            h2 { class: "text-lg font-bold mb-3", "What happens next" }
            ul { class: "space-y-3 text-sm text-base-content/75 leading-relaxed",
              li { "A person reads it. There is no queue and no bot triage." }
              li { "You get a straight answer about fit, including when the answer is that we don't fit yet." }
              li { "Nothing is billing during early access, so nobody is going to try to close you." }
            }
          }

          div { class: "rounded-2xl border border-base-300 bg-base-100 p-6",
            h2 { class: "text-lg font-bold mb-3", "Faster than waiting on us" }
            ul { class: "space-y-3",
              li {
                Link {
                  class: "flex items-start gap-3 group hover:text-secondary transition-colors",
                  to: Route::DocumentationPage {},
                  Icon { icon: LdBookOpen, class: "size-5 mt-0.5 shrink-0", title: "Documentation" }
                  span { class: "text-sm leading-relaxed",
                    span { class: "font-semibold block", "Documentation" }
                    "Connecting a first device, shadows, telemetry, OTA."
                  }
                }
              }
              li {
                Link {
                  class: "flex items-start gap-3 group hover:text-secondary transition-colors",
                  to: Route::ApiReferencePage {},
                  Icon { icon: LdMessageSquare, class: "size-5 mt-0.5 shrink-0", title: "API reference" }
                  span { class: "text-sm leading-relaxed",
                    span { class: "font-semibold block", "API reference" }
                    "Every route, both auth models, request and response shapes."
                  }
                }
              }
              li {
                a {
                  class: "flex items-start gap-3 group hover:text-secondary transition-colors",
                  href: "https://github.com/justins-engineering",
                  Icon { icon: LdGithub, class: "size-5 mt-0.5 shrink-0", title: "GitHub" }
                  span { class: "text-sm leading-relaxed",
                    span { class: "font-semibold block", "The source" }
                    "Backend, dashboard and device firmware, all AGPL-3.0."
                  }
                }
              }
            }
          }

          div { class: "rounded-2xl border border-base-300 bg-base-100 p-6",
            h2 { class: "text-lg font-bold mb-3 flex items-center gap-2",
              Icon { icon: LdMail, class: "size-5", title: "Email" }
              "Prefer email?"
            }
            p { class: "text-sm text-base-content/75 leading-relaxed",
              "The form is the fastest route to us, but "
              a { class: "link link-secondary", href: "mailto:code@jes.contact", "code@jes.contact" }
              " reaches the same inbox."
            }
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn fleet_link_preselects_a_fleet_sized_band() {
    assert_eq!(
      preselected_fleet_size(Some("fleet")),
      Some(ContactFleetSize::From1500To10000)
    );
  }

  #[test]
  fn every_other_entry_point_leaves_the_select_unanswered() {
    for about in [None, Some("pricing"), Some("footer"), Some("")] {
      assert_eq!(preselected_fleet_size(about), None);
    }
  }

  /// The preselected value is written into the select as a wire string, so
  /// a band that stopped round-tripping would silently render as "Prefer
  /// not to say" rather than failing anywhere visible.
  #[test]
  fn the_preselected_band_round_trips_through_its_wire_value() {
    let preselected = preselected_fleet_size(Some("fleet")).unwrap();
    assert_eq!(
      ContactFleetSize::from_wire(preselected.wire()),
      Some(preselected)
    );
  }

  /// The three statuses the route answers before it asks Cloudflare leave
  /// the token unspent; everything else, including no answer at all, is
  /// treated as spent so a resend never carries a token that can only
  /// fail again.
  #[test]
  fn only_pre_verification_rejections_keep_the_token() {
    for status in [400, 413, 429] {
      assert!(!challenge_spent(Some(status)));
    }
    for status in [403, 500, 502, 503] {
      assert!(challenge_spent(Some(status)));
    }
    assert!(challenge_spent(None));
  }

  /// The site key lands inside a JS object literal, so it has to arrive
  /// as a quoted string and never as bare text.
  #[test]
  fn the_mount_script_quotes_the_site_key() {
    let site_key = serde_json::to_string(TURNSTILE_SITE_KEY).unwrap();
    let script = TURNSTILE_MOUNT_JS.replace("__SITE_KEY__", &site_key);
    assert!(script.contains(&format!("sitekey: \"{TURNSTILE_SITE_KEY}\",")));
    assert!(!script.contains("__SITE_KEY__"));
  }
}
