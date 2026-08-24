use super::org::redirect_to;
use crate::helpers::pricing_data::{self, Column, Profile, Provenance, Row};
use crate::{Route, Session, api};
use capsules::BillingPlan;
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdCheck, LdPlay};
use time::Date;

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

/// A paid tier card's call to action. Signed-out visitors keep the
/// disabled "Free in beta" chip -- checkout is only offered to a signed-in
/// visitor, and even then only resolves for someone who manages exactly
/// one org with no live subscription (an entitled org changes plan in the
/// Billing Portal from its own page instead, since a second Checkout would
/// create a second subscription). Anyone else lands on the Organizations
/// page to pick or create the org to bill.
#[component]
fn TierUpgradeCta(plan: BillingPlan) -> Element {
  let session = use_context::<Session>();
  let nav = use_navigator();
  let mut busy = use_signal(|| false);
  let mut cta_error = use_signal(|| Option::<String>::None);

  if !(session.state)().is_authenticated() {
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
          let managed: Vec<_> = api::orgs::list()
              .await
              .unwrap_or_default()
              .into_iter()
              .filter(|m| m.role.is_manager())
              .collect();
          match managed.as_slice() {
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
  }
}

/// Where one figure came from and when we last looked. Deliberately quiet:
/// a small line under the platform name, not a column of its own, because
/// a reader comparing prices should be able to ignore it right up until it
/// matters. It stops being quiet only when the figure has gone unchecked
/// past the data file's own threshold, which is the one case where the
/// number on screen might not be the number the vendor charges.
#[component]
fn FigureSource(row: Row, stale: bool) -> Element {
  rsx! {
    div { class: "mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs font-normal text-base-content/60",
      if let (Some(href), Some(host)) = (row.source.clone(), row.source_host()) {
        a {
          class: "link link-hover",
          href,
          target: "_blank",
          rel: "nofollow noopener",
          "{host}"
        }
      } else {
        span { "our own published price" }
      }
      span { class: if stale { "text-warning font-medium" }, "checked {row.verified_label()}" }
      if let Some(qualifier) = row.provenance().qualifier() {
        span { "· {qualifier}" }
      }
      if stale {
        span {
          class: "badge badge-warning badge-xs",
          title: "This figure has gone long enough unchecked that the vendor may have repriced since.",
          "recheck"
        }
      }
    }
  }
}

/// One vendor: what they charge, and in one line, the thing that actually
/// decides it. The difference sits with the platform name rather than in a
/// list below the table, because a reader comparing prices will not scroll
/// to find out what a price buys.
#[component]
fn ComparisonRow(row: Row, columns: Vec<Column>, today: Date, stale_after: i64) -> Element {
  let stale = row.is_stale(today, stale_after);

  rsx! {
    tr { class: if row.provenance() == Provenance::Ours { "font-bold" },
      td {
        div {
          "{row.platform}"
          if let Some(plan) = row.plan.as_ref() {
            span { class: "font-normal text-base-content/70", " · {plan}" }
          }
        }
        if let Some(note) = row.note.as_ref() {
          div { class: "mt-0.5 text-sm font-normal text-base-content/70", "{note}" }
        }
        FigureSource { row: row.clone(), stale }
      }
      for column in columns.iter() {
        td { key: "{column.key}", class: "text-right whitespace-nowrap align-top",
          if let Some(figure) = row.figure(column) {
            div { "{figure.value}" }
            if let Some(qualifier) = figure.qualifier.as_ref() {
              div { class: "text-xs font-normal text-base-content/60", "{qualifier}" }
            }
          } else {
            span { class: "font-normal italic text-base-content/50",
              "{pricing_data::NOT_PUBLISHED}"
            }
          }
        }
      }
    }
  }
}

/// One cadence: one table of things you can buy, then the separate
/// question of assembling it yourself.
#[component]
fn ProfileComparison(profile: Profile, today: Date, stale_after: i64) -> Element {
  rsx! {
    div { class: "mt-16 first:mt-0",
      h3 { class: "text-2xl md:text-3xl font-extrabold tracking-tight", "{profile.heading}" }
      p { class: "mt-3 text-base-content/70 leading-relaxed", "{profile.subhead}" }

      div { class: "mt-6 overflow-x-auto rounded-2xl border border-base-300 bg-base-100",
        table { class: "table",
          thead {
            tr {
              th { "Platform" }
              for column in profile.columns.iter() {
                th { key: "{column.key}", class: "text-right",
                  div { "{column.label}" }
                  div { class: "font-normal text-base-content/60", "{column.unit}" }
                }
              }
            }
          }
          tbody {
            for row in profile.rows.iter() {
              ComparisonRow {
                key: "{row.id}",
                row: row.clone(),
                columns: profile.columns.clone(),
                today,
                stale_after,
              }
            }
          }
        }
      }

      if let Some(closing) = profile.closing.as_ref() {
        p { class: "mt-5 text-base-content/80 leading-relaxed font-medium", "{closing}" }
      }

      // Raw infrastructure, deliberately not a row in the table above. A
      // rate in cents sitting in a price column anchors a reader before any
      // feature list reaches them, so they meet the reason first and the
      // rates second, as a footnote rather than a ranking.
      div { class: "mt-6 rounded-2xl border border-base-300 bg-base-100 p-5",
        p { class: "text-sm text-base-content/75 leading-relaxed",
          "{profile.build_your_own_intro}"
        }
        p { class: "mt-3 text-xs text-base-content/50",
          "Per device per month at {reference_label(&profile)}:"
        }
        ul { class: "mt-1.5 flex flex-col gap-1.5 text-xs text-base-content/60",
          for row in profile.build_your_own.iter() {
            li { key: "{row.id}", class: "flex flex-wrap items-baseline gap-x-2",
              span { class: "font-bold text-base-content/80",
                "{row.platform}"
                if let Some(plan) = row.plan.as_ref() {
                  span { class: "font-normal", ", {plan}" }
                }
              }
              span { class: "font-bold text-base-content/80",
                "{rate_at_reference(&row, &profile)}"
              }
              if let Some(note) = row.note.as_ref() {
                span { "{note}" }
              }
              if let Some(host) = row.source_host() {
                span { class: if row.is_stale(today, stale_after) { "text-warning font-medium" },
                  "{host}, checked {row.verified_label()}"
                }
              }
            }
          }
        }
      }
    }
  }
}

/// The rate to print for a build-it-yourself line: the one at the
/// profile's reference scale, named beside the list so it is never a bare
/// number. Quoting each row's own cheapest instead would have printed
/// Azure's ten-thousand-device rate next to AWS's thousand-device one,
/// with nothing on the page saying they were measured differently.
fn rate_at_reference(row: &Row, profile: &Profile) -> String {
  profile
    .columns
    .iter()
    .find(|column| column.key == profile.reference_column)
    .and_then(|column| row.figure(column))
    .map(|figure| figure.value.clone())
    .unwrap_or_else(|| pricing_data::NOT_PUBLISHED.to_string())
}

/// The reference column's own label, for the line that says what scale
/// those rates are quoted at.
fn reference_label(profile: &Profile) -> String {
  profile
    .columns
    .iter()
    .find(|column| column.key == profile.reference_column)
    .map(|column| column.label.clone())
    .unwrap_or_default()
}

/// The competitor comparison, rendered from `public/data/pricing-comparison.json`
/// rather than written into this file. See `helpers::pricing_data` for why
/// the numbers live in a data file and how a correction reaches a running
/// page without a rebuild.
#[component]
fn ComparisonTable() -> Element {
  let mut comparison = use_signal(pricing_data::baked);

  // Same shape as views::demo's poll, and never runs during the
  // synchronous SSG pass for the same reason -- so the prerendered tables
  // are the baked copy, and this is only the hydrated page catching up
  // with whatever the deployed file says now.
  //
  // Sets the signal even when the fetch fails, because `today()` below is
  // evaluated at render time and the prerendered HTML evaluated it at
  // build time. A prerendered page hydrates by adopting its own markup, so
  // without a post-hydration write nothing would re-render and the table
  // would keep the build's verdict on its own staleness.
  use_future(move || async move {
    let published = pricing_data::fetch_published().await;
    comparison.set(published.unwrap_or_else(pricing_data::baked));
  });

  let data = comparison();
  let today = pricing_data::today();
  let stale_after = data.stale_after_days;

  rsx! {
    h2 { class: "text-3xl md:text-4xl font-extrabold tracking-tight",
      "What everyone else charges for the same fleet"
    }
    p { class: "mt-4 text-base-content/75 leading-relaxed",
      "Vendors price in units that refuse to compare: per device, per message, per datapoint, per event, in blocks. So every figure below is the same device, normalized to what one costs for a month, with each vendor on the cheapest tier that genuinely fits."
    }

    for profile in data.profiles.iter() {
      ProfileComparison {
        key: "{profile.id}",
        profile: profile.clone(),
        today,
        stale_after,
      }
    }
  }
}

#[component]
fn Answer(question: String, body: String) -> Element {
  rsx! {
    div {
      h3 { class: "text-lg font-bold mb-2", "{question}" }
      p { class: "text-base-content/75 leading-relaxed", "{body}" }
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
          "We're pre-revenue and won't pretend otherwise — nothing below is billing today. One ladder, no editions, no feature paywall: device count is the only number you'd have to forecast."
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
              "10,000 devices, 300M pooled messages, $0.12 per device beyond. We'd rather scope a fleet this size with you than sell it from a page — MQTT and custom dashboards aren't here yet, and you should hear that from us before you sign."
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
          body: "Retention applies to history we keep for you. Point a pigeon's telemetry endpoint at your own database and it goes straight there instead — for as long as you keep it, at whatever resolution you like.",
        }
      }
    }

    section { id: "pricing-example", class: "px-4 md:px-10 py-14 bg-base-200 border-y border-base-300",
      div { class: "max-w-4xl mx-auto", ComparisonTable {} }
    }

    section { id: "pricing-faq", class: "px-4 md:px-10 py-14",
      div { class: "max-w-4xl mx-auto",
        h2 { class: "text-2xl md:text-3xl font-extrabold tracking-tight mb-8", "Straight answers" }
        div { class: "flex flex-col gap-8",
          Answer {
            question: "What counts as a message?",
            body: "A report from a device to us. Shadow polls, firmware chunks, dashboard calls and WebSocket keep-alives don't count — they'd punish exactly the behaviour we want to encourage.",
          }
          Answer {
            question: "What happens if I go over?",
            body: "Nothing is billed in beta, and nothing is metered yet either. When paid tiers start, overage will run at $0.30 per 10,000 and service will keep going; free accounts will pause ingestion instead, warned well before the cap — no surprise invoice, ever.",
          }
          Answer {
            question: "How long do you keep my data?",
            body: "As long as your tier's retention says — 7 days on the free tier, up to 13 months on Scale. Thirteen rather than twelve on purpose: comparing this month to the same month last year needs both of them, and a twelve-month window is one month short of that. Those limits are on the history we store for you. Telemetry forwarded to your own store has no limit from us at all, because we never hold a copy of it.",
          }
          Answer {
            question: "Is anything locked behind a tier?",
            body: "No feature that costs us nothing to serve. Every transport, OTA, remote logs, the firmware catalog and per-device crypto are in the free tier. Tiers differ by devices, messages, retention, SSO and support.",
          }
          Answer {
            question: "Can I self-host it?",
            body: "Honestly: not usefully. The backend is built on Cloudflare Workers and Durable Objects, so \"self-hosting\" means running your own Cloudflare account. The source is public and always will be — but we're not going to sell you a self-host SKU we can't support well.",
          }
          Answer {
            question: "Then what stops lock-in?",
            body: "The exit, not the licence. The device library, the wire protocol and the API spec are open and documented forever, telemetry forwards to any line-protocol store you own, and every device's history is readable straight from the documented API.",
          }
          Answer {
            question: "Will these prices hold?",
            body: "These are planned prices, published early so you can budget — deliberately introductory while MQTT and custom dashboards are still missing. They can still move before billing starts, and we'll tell you well ahead of any change that affects you.",
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
