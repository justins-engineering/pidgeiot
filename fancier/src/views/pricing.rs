use super::org::redirect_to;
use crate::components::ComparisonTables;
use crate::helpers::pricing_data::View;
use crate::{Route, Session, UpgradeIntent, api};
use capsules::{BillingPlan, OrganizationCreateRequest, TaxIdType};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdCheck, LdPlay};
use uuid::Uuid;

/// One rung of the ladder. Every paid tier carries a "planned" badge and a
/// price note saying it is not billing, because publishing numbers we do not
/// charge yet is only honest if that is impossible to miss.
#[component]
fn TierCard(
  name: String,
  badge: String,
  tagline: String,
  price: String,
  cadence: String,
  price_note: String,
  features: Vec<String>,
  cta: Element,
  featured: bool,
) -> Element {
  let frame = if featured {
    "border-primary shadow-xl"
  } else {
    "border-base-300"
  };

  rsx! {
    div { class: "flex flex-col rounded-2xl border {frame} bg-base-100 p-6 h-full",
      div { class: "flex items-baseline gap-2 flex-wrap mb-1",
        h2 { class: "text-xl font-bold", "{name}" }
        if !badge.is_empty() {
          span { class: "badge badge-sm badge-outline font-mono tracking-wide", "{badge}" }
        }
      }
      p { class: "text-sm text-base-content/70 mb-5", "{tagline}" }

      div { class: "flex items-baseline gap-1",
        span { class: "text-4xl font-extrabold tracking-tight", "{price}" }
        if !cadence.is_empty() {
          span { class: "text-base-content/60", "{cadence}" }
        }
      }
      p { class: "mt-1 mb-6 text-xs text-base-content/60", "{price_note}" }

      ul { class: "flex flex-col gap-2 text-sm mb-6",
        for line in features.iter() {
          li { class: "flex gap-2 items-start",
            Icon {
              icon: LdCheck,
              class: "w-4 h-4 mt-0.5 shrink-0 stroke-primary",
              title: "Included",
            }
            span { "{line}" }
          }
        }
      }

      div { class: "mt-auto", {cta} }
    }
  }
}

/// A cost that never appears on an invoice. Separated from the ladder
/// because "what we will not charge for" is the part a fleet operator is
/// actually scanning for.
#[component]
fn NeverCard(label: String, value: String, note: String, body: String) -> Element {
  rsx! {
    div { class: "rounded-2xl border border-base-300 bg-base-100 p-6",
      p { class: "font-mono text-xs tracking-widest uppercase text-primary mb-3", "{label}" }
      div { class: "flex items-baseline gap-2 flex-wrap",
        span { class: "text-2xl font-bold", "{value}" }
        span { class: "text-xs text-base-content/60", "{note}" }
      }
      p { class: "mt-3 text-sm text-base-content/70 leading-relaxed", "{body}" }
    }
  }
}

/// A paid tier card's call to action. Until billing goes live, and for
/// signed-out visitors after that, this is the disabled "Free in beta"
/// chip -- checkout is only offered to a signed-in visitor, and even then
/// only resolves for someone who manages exactly one org with no live
/// subscription (an entitled org changes plan in the Billing Portal from
/// its own page instead, since a second Checkout would create a second
/// subscription). Someone with nothing to bill yet names an organization
/// right here; someone managing several is sent to the Organizations page
/// carrying the picked tier, since which one goes on the plan is a choice
/// only they can make.
#[component]
fn TierUpgradeCta(plan: BillingPlan) -> Element {
  let session = use_context::<Session>();
  let local_session = use_context::<crate::LocalSession>();
  let mut upgrade_intent = use_context::<UpgradeIntent>().0;
  let nav = use_navigator();
  let mut busy = use_signal(|| false);
  let mut cta_error = use_signal(|| Option::<String>::None);
  let mut naming_org = use_signal(|| false);

  if !crate::config::BILLING_LIVE || !(session.state)().is_authenticated() {
    return rsx! {
      div { class: "btn btn-outline w-full font-bold btn-disabled", "Free in beta" }
    };
  }

  rsx! {
    button {
      class: "btn btn-outline w-full font-bold",
      disabled: busy(),
      onclick: move |_| async move {
          busy.set(true);
          cta_error.set(None);
          let managed: Vec<_> = local_session
              .orgs
              .read()
              .values()
              .filter(|m| m.role.is_manager())
              .cloned()
              .collect();
          match managed.as_slice() {
              // Checkout's only missing input is the org to bill, so ask
              // for it here rather than sending them off to find it.
              [] => naming_org.set(true),
              [only] => {
                  let org_id = only.organization.id;
                  match api::billing::overview(org_id).await {
                      Some(o) if o.entitled => {
                          nav.push(Route::OrgView { org_id });
                      }
                      _ => {
                          match api::billing::checkout(org_id, plan).await {
                              Ok(url) => redirect_to(&url),
                              Err(msg) => cta_error.set(Some(msg)),
                          }
                      }
                  }
              }
              _ => {
                  upgrade_intent.set(Some(plan));
                  nav.push(Route::Orgs {});
              }
          }
          busy.set(false);
      },
      if busy() {
        span { class: "loading loading-spinner loading-sm" }
      } else {
        "Upgrade"
      }
    }
    if let Some(msg) = cta_error() {
      p { class: "text-error text-xs mt-2", "{msg}" }
    }
    if naming_org() {
      UpgradeOrgCreate { plan, on_close: move |_| naming_org.set(false) }
    }
  }
}

/// The sentence over the org-name field, naming the tier that is waiting on
/// it so the form cannot be mistaken for an unrelated detour.
fn name_org_prompt(plan: BillingPlan) -> String {
  const HEAD: &str = "A plan is billed to an organization, and you don't have one yet. Name it \
                      and we'll take you straight to ";
  const TAIL: &str = " checkout.";
  let plan = plan.as_str();
  let mut prompt = String::with_capacity(HEAD.len() + plan.len() + TAIL.len());
  prompt.push_str(HEAD);
  prompt.push_str(plan);
  prompt.push_str(TAIL);
  prompt
}

/// One field, because the org's name is all checkout is missing -- business
/// details and tax registration belong to the org's own page, and asking
/// for them here would put a form between someone and the thing they
/// already decided to buy.
///
/// Rendered from a signal rather than a native `<dialog>` so every open
/// remounts it empty, same reason as `TokenReveal` in `views/pigeons.rs`.
#[component]
fn UpgradeOrgCreate(plan: BillingPlan, on_close: EventHandler<()>) -> Element {
  let mut busy = use_signal(|| false);
  let mut error = use_signal(|| Option::<String>::None);
  // Set once the org exists. Submitting again would create a second one,
  // so a checkout that fails after the org landed stops offering the
  // button and points at what it already made.
  let mut created = use_signal(|| Option::<Uuid>::None);

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "upgrade_org_title",
      tabindex: "-1",
      onkeydown: move |e| {
          if e.key() == Key::Escape {
              on_close.call(());
          }
      },
      div { class: "modal-box relative max-w-sm",
        h3 { class: "text-lg font-bold", id: "upgrade_org_title",
          if created().is_some() {
            "Organization created"
          } else {
            "Name your organization"
          }
        }
        p { class: "py-3 text-sm text-base-content/70",
          if created().is_some() {
            "Checkout didn't start, so nothing has been charged. Its Billing section can start one."
          } else {
            "{name_org_prompt(plan)}"
          }
        }

        // Outside the branch below: a checkout that fails after the org
        // landed replaces the form, and that is exactly when the reason
        // matters most.
        if let Some(msg) = error.read().as_ref() {
          p { class: "text-error text-xs mb-3", "{msg}" }
        }

        if let Some(org_id) = created() {
          Link {
            class: "btn btn-primary w-full font-bold",
            to: Route::OrgView { org_id },
            "Open the organization"
          }
        } else {
          form {
            onsubmit: move |evt: FormEvent| async move {
                evt.prevent_default();
                let mut name = String::new();
                for (key, val) in evt.values() {
                    if key == "name"
                        && let FormValue::Text(val) = val
                    {
                        name = val;
                    }
                }
                busy.set(true);
                error.set(None);
                let request = OrganizationCreateRequest {
                    name,
                    business_name: None,
                    tax_id: None,
                    tax_id_type: TaxIdType::None,
                };
                match api::orgs::create(&request).await {
                    Ok(org) => {
                        match api::billing::checkout(org.id, plan).await {
                            Ok(url) => redirect_to(&url),
                            Err(msg) => {
                                created.set(Some(org.id));
                                error.set(Some(msg));
                            }
                        }
                    }
                    Err(msg) => error.set(Some(msg)),
                }
                busy.set(false);
            },
            label { class: "input w-full focus:outline-0",
              input {
                class: "grow focus:outline-0",
                name: "name",
                placeholder: "e.g. Pioneer Valley Transit Authority",
                r#type: "text",
                required: true,
                // Focus lands on the field, not the dialog: Escape still
                // reaches the container by bubbling.
                onmounted: move |e| async move {
                    let _ = e.set_focus(true).await;
                },
              }
            }
            div { class: "mt-5 flex items-center justify-end gap-3",
              button {
                class: "btn btn-ghost",
                r#type: "button",
                onclick: move |_| on_close.call(()),
                "Cancel"
              }
              button {
                class: "btn btn-primary font-bold",
                r#type: "submit",
                disabled: busy(),
                if busy() {
                  span { class: "loading loading-spinner loading-sm" }
                } else {
                  "Continue to checkout"
                }
              }
            }
          }
        }
      }
    }
  }
}

/// `children` continue the answer's paragraph, for the one answer that
/// ends in a link; plain text cannot carry a `Link`.
#[component]
fn Answer(question: String, body: String, children: Element) -> Element {
  rsx! {
    div {
      h3 { class: "text-lg font-bold mb-2", "{question}" }
      p { class: "text-base-content/75 leading-relaxed", "{body}" {children} }
    }
  }
}

#[component]
pub fn PricingPage() -> Element {
  rsx! {
    section { id: "pricing-hero", class: "px-4 md:px-10 pt-16 pb-12 bg-base-200 border-b border-base-300",
      div { class: "max-w-6xl mx-auto",
        p { class: "font-mono text-sm tracking-widest uppercase text-primary mb-4", "Pricing" }
        h1 { class: "text-4xl md:text-6xl font-extrabold tracking-tight max-w-4xl text-pretty",
          "Free while we're in beta. Here's what we plan to charge after."
        }
        p { class: "mt-6 text-xl md:text-2xl leading-relaxed max-w-3xl text-base-content/80 text-pretty",
          "We're pre-revenue and won't pretend otherwise: nothing below is billing today. One ladder, no editions, no feature paywall, and device count is the only number you'd have to forecast."
        }
      }
    }

    section { id: "pricing-tiers", class: "px-4 md:px-10 py-14",
      div { class: "max-w-6xl mx-auto",
        div { class: "grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-6",

          TierCard {
            name: "Perch",
            badge: "not billing yet",
            tagline: "A real pilot, not a teaser.",
            price: "$0",
            cadence: "",
            price_note: "free in beta, and after · no card",
            features: vec![
                "10 devices".into(),
                "300K pooled messages/mo".into(),
                "7 days in our history store".into(),
                "1 seat · 1 alert".into(),
            ],
            featured: true,
            cta: rsx! {
              Link {
                class: "btn btn-primary w-full font-bold",
                to: Route::RegisterFlow { flow: None },
                "Start free"
              }
            },
          }

          TierCard {
            name: "Builder",
            badge: "planned",
            tagline: "First hardware out the door.",
            price: "$29",
            cadence: "/mo",
            price_note: "$0.55 per extra device · not billing yet",
            features: vec![
                "50 devices".into(),
                "1.5M pooled messages/mo".into(),
                "30 days in our history store".into(),
                "3 seats · 10 alerts · 1 org".into(),
            ],
            featured: false,
            cta: rsx! {
              TierUpgradeCta { plan: BillingPlan::Builder }
            },
          }

          TierCard {
            name: "Growth",
            badge: "planned",
            tagline: "A fleet with customers on it.",
            price: "$99",
            cadence: "/mo",
            price_note: "$0.35 per extra device · not billing yet",
            features: vec![
                "250 devices".into(),
                "7.5M pooled messages/mo".into(),
                "90 days in our history store".into(),
                "Unlimited seats, orgs and alerts".into(),
                "Priority email support".into(),
            ],
            featured: false,
            cta: rsx! {
              TierUpgradeCta { plan: BillingPlan::Growth }
            },
          }

          TierCard {
            name: "Scale",
            badge: "planned",
            tagline: "Thousands in the field.",
            price: "$349",
            cadence: "/mo",
            price_note: "$0.20 per extra device · not billing yet",
            features: vec![
                "1,500 devices".into(),
                "45M pooled messages/mo".into(),
                "13 months in our history store".into(),
                "Unlimited seats, orgs and alerts".into(),
                "SSO · priority support with SLA".into(),
            ],
            featured: false,
            cta: rsx! {
              TierUpgradeCta { plan: BillingPlan::Scale }
            },
          }
        }

        // Fleet sits outside the grid rather than as a fifth column: the
        // design's own copy says it should be scoped in a conversation, and
        // a card row invites comparison shopping instead.
        div { class: "mt-6 rounded-2xl border border-base-300 bg-base-100 p-6 md:p-8 flex flex-col lg:flex-row gap-6 lg:items-center",
          div { class: "lg:max-w-2xl",
            div { class: "flex items-baseline gap-2 flex-wrap mb-2",
              h2 { class: "text-xl font-bold", "Fleet" }
              span { class: "badge badge-sm badge-outline font-mono tracking-wide",
                "planned · talk to us first"
              }
            }
            p { class: "text-base-content/75 leading-relaxed",
              "10,000 devices, 300M pooled messages, $0.12 per device beyond. We'd rather scope a fleet this size with you than sell it from a page. Custom dashboards aren't here yet, and you should hear that from us before you sign."
            }
          }
          div { class: "lg:ml-auto lg:text-right shrink-0",
            div { class: "flex items-baseline gap-1 lg:justify-end",
              span { class: "text-4xl font-extrabold tracking-tight", "$1,499" }
              span { class: "text-base-content/60", "/mo" }
            }
            p { class: "mt-1 mb-4 text-xs text-base-content/60", "indicative · not billing yet" }
            // A plain href, not a Link: the funnel context rides in the
            // query string, which a Route with no query prop cannot carry
            // (see views/contact.rs for why it has none). The destination
            // is prerendered, so the full-page load paints immediately.
            a { class: "btn btn-outline font-bold", href: "/contact/?about=fleet", "Talk to us" }
          }
        }

        // Exclusive, which is the US convention and the one every comparable
        // platform follows. Saying so is the cheap half: an inclusive figure
        // cannot be computed for a reader whose VAT rate we do not know, and
        // silence is what leaves someone assuming the number is final.
        p { class: "mt-6 text-sm text-base-content/60",
          "Prices in USD, exclusive of any applicable sales tax or VAT."
        }
      }
    }

    section { id: "pricing-never-billed", class: "px-4 md:px-10 pb-14",
      div { class: "max-w-6xl mx-auto grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-6",
        NeverCard {
          label: "Not billing yet",
          value: "$0.30",
          note: "per 10,000 · planned rate",
          body: "A billable message is a device→platform report: telemetry, shadow report-back, or a log upload. Pooled across the whole account.",
        }
        NeverCard {
          label: "Never metered",
          value: "Firmware bandwidth",
          note: "",
          body: "OTA is free at any image size on every tier, including free. So are shadow polls, dashboard API calls and provisioning.",
        }
        NeverCard {
          label: "Never billed",
          value: "Idle devices",
          note: "",
          body: "A device counts toward billing in a month it sends at least one billable message. The 400 units sitting in a warehouse are free.",
        }
        NeverCard {
          label: "No limit",
          value: "Your own store",
          note: "",
          body: "Retention applies to history we keep for you. Point a pigeon's telemetry endpoint at your own database and it goes straight there instead, for as long as you keep it, at whatever resolution you like.",
        }
      }
    }

    section { id: "pricing-example", class: "px-4 md:px-10 py-14 bg-base-200 border-y border-base-300",
      div { class: "max-w-4xl mx-auto",
        ComparisonTables { view: View::Summary }
        // The rest of the field, the fleet sizes either side of this one,
        // and the rates we lose to all live one click away. They answer
        // "how do you compare", which is a different question from "what
        // does this cost", and mixing the two costs the second its clarity.
        p { class: "mt-8 text-sm",
          Link { class: "link link-hover font-medium", to: Route::ComparePage {},
            "Full comparison: nine platforms, three fleet sizes, and what AWS and Azure cost \u{2192}"
          }
        }
        // The row the table above cannot have. ThingsBoard's free edition
        // on your own server is the alternative most readers of this
        // table are actually weighing, and its price is mostly hours.
        p { class: "mt-2 text-sm",
          Link { class: "link link-hover font-medium", to: Route::SelfHostingPage {},
            "Weighing your own server instead? What self-hosting actually costs \u{2192}"
          }
        }
      }
    }

    section { id: "pricing-faq", class: "px-4 md:px-10 py-14",
      div { class: "max-w-4xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight mb-8", "Straight answers" }
        div { class: "flex flex-col gap-8",
          Answer {
            question: "What counts as a message?",
            body: "A report from a device to us. Shadow polls, firmware chunks, dashboard calls and WebSocket keep-alives don't count, because they'd punish exactly the behaviour we want to encourage.",
          }
          Answer {
            question: "What happens if I go over?",
            body: "Nothing is billed in beta. Usage is already counted, so you can see exactly where you'd land: when paid tiers start, overage will run at $0.30 per 10,000 and service will keep going; free accounts pause ingestion instead, warned at 80% of the cap. No surprise invoice, ever.",
          }
          Answer {
            question: "How long do you keep my data?",
            body: "As long as your tier's retention says: 7 days on the free tier, up to 13 months on Scale. Thirteen rather than twelve on purpose: comparing this month to the same month last year needs both of them, and a twelve-month window is one month short of that. Those limits are on the history we store for you. Telemetry forwarded to your own store has no limit from us at all, because we never hold a copy of it.",
          }
          Answer {
            question: "Is anything locked behind a tier?",
            body: "No feature that costs us nothing to serve. Every transport, OTA, remote logs, the firmware catalog and per-device crypto are in the free tier. Tiers differ by devices, messages, retention, SSO and support.",
          }
          Answer {
            question: "Can I self-host it?",
            body: "Honestly: not usefully. The backend is built on Cloudflare Workers and Durable Objects, so \"self-hosting\" means running your own Cloudflare account. The source is public and always will be, but we're not going to sell you a self-host SKU we can't support well.",
            " If what you're weighing is self-hosting a stack of your own instead, "
            Link { class: "link link-hover font-medium", to: Route::SelfHostingPage {},
              "here is what that actually costs"
            }
            "."
          }
          Answer {
            question: "Then what stops lock-in?",
            body: "The exit, not the licence. The device library, the wire protocol and the API spec are open and documented forever, telemetry forwards to any line-protocol store you own, and every device's history is readable straight from the documented API.",
          }
          Answer {
            question: "Will these prices hold?",
            body: "These are planned prices, published early so you can budget, and deliberately introductory while custom dashboards are still missing. They can still move before billing starts, and we'll tell you well ahead of any change that affects you.",
          }
        }
      }
    }

    section { id: "pricing-cta", class: "px-4 md:px-10 pb-24",
      div { class: "max-w-4xl mx-auto rounded-3xl border border-neutral-content bg-linear-to-br/srgb from-primary/40 via-secondary/40 to-accent/40 p-10 text-center shadow-2xl",
        h2 { class: "text-2xl md:text-3xl font-bold mb-3", "Everything's free while we're in beta." }
        p { class: "text-lg mb-8 leading-relaxed",
          "Ten devices stay free after that. Start now and help shape what the rest costs."
        }
        div { class: "flex flex-col sm:flex-row justify-center gap-4",
          Link {
            class: "btn btn-lg btn-glow font-bold",
            to: Route::RegisterFlow { flow: None },
            Icon { icon: LdPlay, class: "mr-2", title: "Start free" }
            "Start free"
          }
          Link { class: "btn btn-lg btn-outline font-bold", to: Route::DemoPage {},
            "Try the live demo"
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod name_org_prompt_tests {
  use super::name_org_prompt;
  use capsules::BillingPlan;

  #[test]
  fn names_the_plan_checkout_is_waiting_on() {
    assert_eq!(
      name_org_prompt(BillingPlan::Builder),
      "A plan is billed to an organization, and you don't have one yet. Name it and we'll take \
       you straight to builder checkout."
    );
  }

  // A resize would mean the parts were mis-counted.
  #[test]
  fn prompt_is_one_allocation() {
    for plan in [
      BillingPlan::Builder,
      BillingPlan::Growth,
      BillingPlan::Scale,
      BillingPlan::Fleet,
    ] {
      let prompt = name_org_prompt(plan);
      assert_eq!(prompt.len(), prompt.capacity(), "{plan}");
    }
  }
}
