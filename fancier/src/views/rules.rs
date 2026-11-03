//! A flock's rules: the list, and one rule's page with its editor, the assistant's place, the
//! test and deploy panel, its versions, runs and values. Rule code is only ever text here: the
//! dashboard's origin holds the session cookie, so nothing on these pages evaluates it.

use crate::components::{ConfirmModal, DangerAction, DangerZone, JsonViewer};
use crate::helpers::rule_view::{
  Badge, failure_groups, hourly_bars, origin_label, parse_recipients, rule_badges, save_origin,
  test_badge,
};
use crate::helpers::telemetry_export::display_utc;
use crate::{LocalSession, Route, api};
use capsules::{
  MAX_RULE_SOURCE_BYTES, Rule, RuleCreateRequest, RuleDeployRequest, RuleDetail, RuleRun,
  RuleUpdateRequest, RuleValue, RuleVersion, RuleVersionCreateRequest, check_rule_source,
  normalize_rule_name,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowLeft, LdX};
use time::OffsetDateTime;
use uuid::Uuid;

/// The editor's textarea, which the Tab key reaches through the DOM.
const SOURCE_ID: &str = "flock-rule-source";

/// What a new rule's editor starts from: a working rule that shows the shape of the contract.
const STARTER_SOURCE: &str = r#"// Runs over each telemetry batch from each pigeon in this flock.
// Return alerts, derived values and a state patch; return {} to do nothing.
import { num } from "./pidgeiot.js";

const LIMIT = 40; // degrees C

export default function (event) {
  let hottest = null;
  for (const reading of event.readings) {
    const t = num(reading.metrics.temp_c);
    if (t !== null && (hottest === null || t > hottest)) hottest = t;
  }
  if (hottest === null) return {};
  const hot = hottest > LIMIT;
  // Restated every batch: the platform mails only when the firing flag changes.
  return {
    alerts: [{
      key: "too-hot",
      kind: hot ? "fire" : "clear",
      severity: hot ? "warning" : "info",
      scope: "pigeon",
      subject: hot ? "Running hot" : "Back to normal",
      body: "Highest reading in this batch: " + hottest + " C.",
    }],
  };
}
"#;

/// Whether the caller can change this flock's rules: its own personal flock, or an org flock
/// where it is an owner or admin. dovecote makes the same check and has the last word.
fn can_manage(session: &LocalSession, flock_id: Uuid) -> bool {
  match session.flocks.read().get(&flock_id).map(|f| f.org_id) {
    Some(None) => true,
    Some(Some(org)) => session
      .orgs
      .read()
      .get(&org)
      .is_some_and(|m| m.role.is_manager()),
    None => false,
  }
}

/// The badges of a rule, in one row that wraps.
#[component]
fn RuleBadges(badges: Vec<Badge>) -> Element {
  rsx! {
    div { class: "flex flex-wrap gap-1",
      for badge in badges {
        span { class: "badge badge-sm whitespace-nowrap {badge.class}", "{badge.label}" }
      }
    }
  }
}

#[component]
pub fn FlockRules(flock_id: Uuid) -> Element {
  let session = use_context::<LocalSession>();
  let nav = use_navigator();
  let mut rules = use_signal(|| None::<Result<Vec<Rule>, String>>);
  let mut toggle_error = use_signal(|| None::<String>);
  let mut show_create = use_signal(|| false);

  use_resource(move || async move {
    rules.set(Some(api::rules::list(flock_id).await));
  });

  let manage = can_manage(&session, flock_id);
  let flock_name = session
    .flocks
    .read()
    .get(&flock_id)
    .map(|f| f.name.clone())
    .unwrap_or_else(|| "Flock".to_string());
  let now = OffsetDateTime::now_utc();

  rsx! {
    section { id: "flock-rules-list", class: "my-1 max-w-7xl mx-auto w-full",
      header { class: "flex flex-col sm:flex-row sm:items-center gap-4 mb-6",
        Link {
          to: Route::Pigeons { flock_id },
          class: "btn btn-ghost btn-sm text-base-content/80 self-start",
          Icon {
            width: 20,
            height: 20,
            icon: LdArrowLeft,
            title: "Back to the flock",
          }
        }
        div { class: "grow min-w-0",
          h1 { class: "text-xl font-bold", "Rules" }
          p { class: "text-sm text-base-content/70 break-words", "{flock_name}" }
        }
        // Only once the list has loaded: a flock whose organization has no rules refuses it.
        if manage && matches!(rules(), Some(Ok(_))) {
          button {
            class: "btn btn-outline btn-primary sm:px-6",
            onclick: move |_| show_create.set(true),
            "New rule"
          }
        }
      }
      p { class: "text-sm text-base-content/70 mb-6 max-w-3xl",
        "A rule is a short JavaScript function the platform runs over each telemetry batch from "
        "this flock's pigeons. It can raise alerts, record derived values and keep its own state. "
        "It has no network access."
      }
      if let Some(err) = toggle_error() {
        div { role: "alert", class: "alert alert-error alert-soft mb-4 text-sm", "{err}" }
      }
      match rules() {
        None => rsx! {
          div { class: "flex items-center gap-2 text-base-content/70",
            span { class: "loading loading-spinner loading-sm" }
            "Loading rules..."
          }
        },
        Some(Err(err)) => rsx! {
          div { class: "bg-base-100 border border-base-content/10 rounded-box p-8 text-center max-w-xl mx-auto",
            h2 { class: "text-lg font-semibold mb-2", "Rules are not available here" }
            p { class: "text-base-content/70 text-sm break-words", "{err}" }
          }
        },
        Some(Ok(list)) if list.is_empty() => rsx! {
          div { class: "bg-base-100 border border-base-content/10 rounded-box p-8 text-center max-w-xl mx-auto",
            h2 { class: "text-lg font-semibold mb-2", "No rules yet" }
            p { class: "text-base-content/70 text-sm",
              "Create a rule, write or upload its code, test it over this flock's recent "
              "telemetry, then deploy it."
            }
          }
        },
        Some(Ok(list)) => rsx! {
          div { class: "overflow-x-auto rounded-box border border-base-content/10 shadow-sm bg-base-100",
            table { class: "table w-full",
              thead {
                tr { class: "bg-base-200/50 text-base-content",
                  th { "Name" }
                  th { "Status" }
                  th { "Enabled" }
                  th { "Updated" }
                }
              }
              tbody {
                for rule in list {
                  tr { key: "{rule.id}", class: "hover",
                    td { class: "font-semibold min-w-40",
                      Link {
                        class: "link link-hover text-primary break-words",
                        to: Route::RuleView {
                            flock_id,
                            rule_id: rule.id,
                        },
                        "{rule.name}"
                      }
                    }
                    td { class: "min-w-48",
                      RuleBadges { badges: rule_badges(&rule, now) }
                    }
                    td {
                      {
                          let id = rule.id;
                          let enabled = rule.enabled;
                          let locked = !manage || rule.deployed_version.is_none() || rule.blocked;
                          rsx! {
                            input {
                              r#type: "checkbox",
                              class: "toggle toggle-sm toggle-success",
                              "aria-label": "Enabled",
                              checked: enabled,
                              disabled: locked,
                              onchange: move |_| async move {
                                  toggle_error.set(None);
                                  let req = RuleUpdateRequest {
                                      enabled: Some(!enabled),
                                      ..Default::default()
                                  };
                                  match api::rules::update(flock_id, id, &req).await {
                                      Ok(updated) => replace_rule(&mut rules, updated),
                                      Err(err) => toggle_error.set(Some(err)),
                                  }
                              },
                            }
                          }
                      }
                    }
                    td { class: "text-sm text-base-content/70 whitespace-nowrap",
                      "{display_utc(rule.updated_at)}"
                    }
                  }
                }
              }
            }
          }
        },
      }
      if show_create() {
        CreateRuleModal {
          flock_id,
          on_close: move |_| show_create.set(false),
          on_created: move |rule: Rule| {
              show_create.set(false);
              nav.push(Route::RuleView {
                  flock_id,
                  rule_id: rule.id,
              });
          },
        }
      }
    }
  }
}

/// Puts a mutation's answer in place of the rule it changed.
fn replace_rule(rules: &mut Signal<Option<Result<Vec<Rule>, String>>>, updated: Rule) {
  if let Some(Ok(list)) = rules.write().as_mut()
    && let Some(slot) = list.iter_mut().find(|r| r.id == updated.id)
  {
    *slot = updated;
  }
}

/// The create form, rendered from a parent signal so each open starts empty.
#[component]
fn CreateRuleModal(
  flock_id: Uuid,
  on_close: EventHandler<()>,
  on_created: EventHandler<Rule>,
) -> Element {
  let mut name = use_signal(String::new);
  let mut recipients = use_signal(String::new);
  let mut error = use_signal(|| None::<String>);
  let mut busy = use_signal(|| false);

  rsx! {
    div {
      class: "modal modal-open",
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": "create-rule-title",
      div { class: "modal-box relative",
        button {
          class: "btn btn-sm btn-circle btn-ghost absolute inset-e-2 top-2",
          r#type: "button",
          onclick: move |_| on_close.call(()),
          Icon { icon: LdX, title: "close" }
        }
        h3 { id: "create-rule-title", class: "text-lg font-bold mb-4", "New rule" }
        form {
          class: "flex flex-col gap-3",
          onsubmit: move |evt| async move {
              evt.prevent_default();
              error.set(None);
              let checked = match normalize_rule_name(&name()) {
                  Ok(checked) => checked,
                  Err(rejection) => {
                      error.set(Some(rejection.message().to_string()));
                      return;
                  }
              };
              busy.set(true);
              let req = RuleCreateRequest {
                  name: checked,
                  recipients: parse_recipients(&recipients()),
              };
              let answer = api::rules::create(flock_id, &req).await;
              busy.set(false);
              match answer {
                  Ok(rule) => on_created.call(rule),
                  Err(err) => error.set(Some(err)),
              }
          },
          label { class: "fieldset-legend text-xs font-semibold", r#for: "create-rule-name", "Name" }
          input {
            id: "create-rule-name",
            class: "input input-bordered w-full",
            r#type: "text",
            maxlength: 120,
            value: "{name}",
            oninput: move |evt| name.set(evt.value()),
          }
          label {
            class: "fieldset-legend text-xs font-semibold",
            r#for: "create-rule-recipients",
            "Who it mails (optional)"
          }
          textarea {
            id: "create-rule-recipients",
            class: "textarea textarea-bordered w-full text-sm",
            rows: 2,
            placeholder: "Addresses on this account, separated by commas",
            value: "{recipients}",
            oninput: move |evt| recipients.set(evt.value()),
          }
          p { class: "text-xs text-base-content/60",
            "Left empty, the rule mails the flock owner. A new rule is off and has no code until "
            "you save, test and deploy a version."
          }
          if let Some(err) = error() {
            p { class: "text-error text-sm break-words", "{err}" }
          }
          div { class: "modal-action",
            button {
              class: "btn btn-ghost",
              r#type: "button",
              onclick: move |_| on_close.call(()),
              "Cancel"
            }
            button {
              class: "btn btn-primary",
              r#type: "submit",
              disabled: busy(),
              "Create rule"
            }
          }
        }
      }
    }
  }
}

/// Replaces the editor's selection with two spaces, through the DOM so the caret lands after
/// them, and returns the new text for the signal. Shift+Tab is left alone to leave the editor.
#[cfg(feature = "web")]
fn indent_at_caret() -> Option<String> {
  use crate::helpers::rule_view::insert_indent;
  use wasm_bindgen::{JsCast, JsValue};
  let el = web_sys::window()?
    .document()?
    .get_element_by_id(SOURCE_ID)?;
  let prop = |name: &str| js_sys::Reflect::get(&el, &JsValue::from_str(name)).ok();
  let start = prop("selectionStart")?.as_f64()? as u32;
  let end = prop("selectionEnd")?.as_f64()? as u32;
  let (text, caret) = insert_indent(&prop("value")?.as_string()?, start, end);
  js_sys::Reflect::set(&el, &JsValue::from_str("value"), &JsValue::from_str(&text)).ok()?;
  let select = prop("setSelectionRange")?
    .dyn_into::<js_sys::Function>()
    .ok()?;
  select.call2(&el, &caret.into(), &caret.into()).ok()?;
  Some(text)
}

#[cfg(not(feature = "web"))]
fn indent_at_caret() -> Option<String> {
  None
}

/// A run's status badge: red for a failure, plain for `ok` and `skipped`.
fn run_badge_class(status: &str) -> &'static str {
  if crate::helpers::rule_view::is_failure(status) {
    "badge badge-error badge-sm"
  } else {
    "badge badge-ghost badge-sm"
  }
}

/// Saves `source` as `rule.js`.
fn download_source(source: &str) {
  crate::helpers::download_bytes(source.as_bytes(), "rule.js", "text/javascript");
}

/// A version number as the pages show it.
fn version_label(version: i32) -> String {
  let number = version.to_string();
  let mut label = String::with_capacity(1 + number.len());
  label.push('v');
  label.push_str(&number);
  label
}

#[component]
pub fn RuleView(flock_id: Uuid, rule_id: Uuid) -> Element {
  let session = use_context::<LocalSession>();
  let nav = use_navigator();
  let manage = can_manage(&session, flock_id);

  let mut detail = use_signal(|| None::<Result<RuleDetail, String>>);
  // The editor's text, and the stored version it started from.
  let mut source = use_signal(String::new);
  let mut base = use_signal(|| None::<RuleVersion>);
  let mut save_note = use_signal(|| None::<Result<String, String>>);
  let mut busy = use_signal(|| false);
  // Test and deploy: the version they act on and the server's last answer.
  let mut target = use_signal(|| None::<i32>);
  let mut reset_state = use_signal(|| false);
  let mut action_note = use_signal(|| None::<Result<String, String>>);
  let mut tested = use_signal(|| None::<RuleVersion>);
  // Up to two versions open side by side.
  let mut open_versions = use_signal(Vec::<RuleVersion>::new);
  let mut runs = use_signal(|| None::<Result<Vec<RuleRun>, String>>);
  let mut values = use_signal(|| None::<Result<Vec<RuleValue>, String>>);
  let mut show_delete = use_signal(|| false);

  use_resource(move || async move {
    let answer = api::rules::detail(flock_id, rule_id).await;
    let start = answer.as_ref().ok().and_then(|d| {
      d.rule
        .deployed_version
        .or_else(|| d.versions.first().map(|v| v.version))
    });
    detail.set(Some(answer));
    target.set(start);
    match start {
      Some(version) => match api::rules::version(flock_id, rule_id, version).await {
        Ok(stored) => {
          source.set(stored.source.clone().unwrap_or_default());
          base.set(Some(stored));
        }
        Err(err) => save_note.set(Some(Err(err))),
      },
      None => source.set(STARTER_SOURCE.to_string()),
    }
  });
  use_resource(move || async move {
    runs.set(Some(api::rules::runs(flock_id, rule_id).await));
  });
  use_resource(move || async move {
    values.set(Some(api::rules::values(flock_id, rule_id).await));
  });

  let current = detail();
  let Some(current) = current else {
    return rsx! {
      div { class: "flex items-center gap-2 text-base-content/70 max-w-7xl mx-auto",
        span { class: "loading loading-spinner loading-sm" }
        "Loading rule..."
      }
    };
  };
  let RuleDetail { rule, versions } = match current {
    Ok(found) => found,
    Err(err) => {
      return rsx! {
        div { class: "bg-base-100 border border-base-content/10 rounded-box p-8 text-center max-w-xl mx-auto",
          h1 { class: "text-lg font-semibold mb-2", "This rule could not be loaded" }
          p { class: "text-base-content/70 text-sm break-words mb-4", "{err}" }
          Link { to: Route::FlockRules { flock_id }, class: "btn btn-sm", "Back to rules" }
        }
      };
    }
  };

  let now = OffsetDateTime::now_utc();
  let source_problem = check_rule_source(&source()).err();
  let unchanged = base
    .read()
    .as_ref()
    .is_some_and(|b| b.source.as_deref() == Some(source().as_str()));
  let base_label = base.read().as_ref().map(|b| version_label(b.version));
  let target_version = target();

  // Writes a mutation's rule back into the page.
  let mut set_rule = move |updated: Rule| {
    if let Some(Ok(found)) = detail.write().as_mut() {
      found.rule = updated;
    }
  };

  // Saves `text` as the next version and makes it the editor's base.
  let save = move |text: String| async move {
    save_note.set(None);
    busy.set(true);
    let req = RuleVersionCreateRequest {
      source: text,
      origin: save_origin(base.read().as_ref()),
      based_on: base.read().as_ref().map(|b| b.version),
    };
    let answer = api::rules::save_version(flock_id, rule_id, &req).await;
    busy.set(false);
    match answer {
      Ok(saved) => {
        let mut listed = saved.clone();
        listed.source = None;
        listed.test_summary = None;
        if let Some(Ok(found)) = detail.write().as_mut() {
          found.versions.insert(0, listed);
        }
        let mut note = String::with_capacity(16);
        note.push_str("Saved as ");
        note.push_str(&version_label(saved.version));
        save_note.set(Some(Ok(note)));
        target.set(Some(saved.version));
        source.set(saved.source.clone().unwrap_or_default());
        base.set(Some(saved));
      }
      Err(err) => save_note.set(Some(Err(err))),
    }
  };

  let deploy = move |version: i32| async move {
    action_note.set(None);
    busy.set(true);
    let req = RuleDeployRequest {
      reset_state: reset_state(),
    };
    let answer = api::rules::deploy(flock_id, rule_id, version, &req).await;
    busy.set(false);
    match answer {
      Ok(updated) => {
        set_rule(updated);
        let mut note = String::with_capacity(48);
        note.push_str("Deployed ");
        note.push_str(&version_label(version));
        note.push_str(". Rules start within about a minute.");
        action_note.set(Some(Ok(note)));
      }
      Err(err) => action_note.set(Some(Err(err))),
    }
  };

  let bars = match runs() {
    Some(Ok(list)) => hourly_bars(&list, now, 24),
    _ => Vec::new(),
  };
  let tallest = bars
    .iter()
    .map(|b| b.ok + b.skipped + b.failed)
    .max()
    .unwrap_or(0)
    .max(1);

  rsx! {
    div { class: "my-1 max-w-7xl mx-auto w-full flex flex-col gap-8",
      header { class: "flex flex-col gap-3",
        div { class: "flex items-center gap-3",
          Link {
            to: Route::FlockRules { flock_id },
            class: "btn btn-ghost btn-sm text-base-content/80",
            Icon {
              width: 20,
              height: 20,
              icon: LdArrowLeft,
              title: "Back to rules",
            }
          }
          h1 { class: "text-xl font-bold break-words min-w-0", "{rule.name}" }
        }
        RuleBadges { badges: rule_badges(&rule, now) }
        if let Some(pending) = rule.pending_version {
          div { role: "alert", class: "alert alert-warning alert-soft text-sm flex flex-wrap",
            span { "The deploy of {version_label(pending)} did not finish. The rule runs as before." }
            if manage {
              button {
                class: "btn btn-sm",
                disabled: busy(),
                onclick: move |_| deploy(pending),
                "Retry"
              }
            }
          }
        }
        p { class: "text-sm text-base-content/70 max-w-3xl",
          "The rules of one flock run together, so each can see the data the others are given."
        }
      }

      div { class: "grid gap-8 lg:grid-cols-2",
        section { id: "flock-rule-editor", class: "flex flex-col gap-3 min-w-0",
          div { class: "flex flex-wrap items-baseline justify-between gap-2",
            h2 { class: "text-lg font-bold", "Code" }
            span { class: "text-xs text-base-content/60",
              match (base_label.as_deref(), versions.is_empty()) {
                  (Some(label), _) => rsx! { "Editing from {label}" },
                  (None, true) => rsx! { "No version saved yet: this is an example to start from" },
                  (None, false) => rsx! { "The saved code could not be loaded" },
              }
            }
          }
          textarea {
            id: SOURCE_ID,
            class: "textarea textarea-bordered w-full h-96 font-mono text-xs leading-5",
            spellcheck: "false",
            autocomplete: "off",
            wrap: "off",
            "aria-label": "Rule code",
            readonly: !manage,
            value: "{source}",
            oninput: move |evt| source.set(evt.value()),
            onkeydown: move |evt: KeyboardEvent| {
                if evt.key() == Key::Tab && evt.modifiers().is_empty() {
                    evt.prevent_default();
                    if let Some(text) = indent_at_caret() {
                        source.set(text);
                    }
                }
            },
          }
          if let Some(problem) = source_problem {
            p { class: "text-error text-xs", "{problem.message()}" }
          }
          if manage {
            div { class: "flex flex-wrap items-center gap-2",
              button {
                class: "btn btn-primary btn-sm",
                disabled: busy() || source_problem.is_some() || unchanged,
                onclick: move |_| save(source()),
                "Save as new version"
              }
              label { class: "btn btn-outline btn-sm",
                "Upload .js"
                input {
                  r#type: "file",
                  accept: ".js,.mjs,text/javascript",
                  class: "hidden",
                  onchange: move |evt: Event<FormData>| async move {
                      let Some(file) = evt.files().into_iter().next() else {
                          return;
                      };
                      if file.size() > MAX_RULE_SOURCE_BYTES as u64 {
                          save_note
                              .set(Some(Err("The file is over the 64 KiB a rule may be.".to_string())));
                          return;
                      }
                      match file.read_string().await {
                          Ok(text) => save(text).await,
                          Err(_) => save_note.set(Some(Err("The file could not be read.".to_string()))),
                      }
                  },
                }
              }
              button {
                class: "btn btn-ghost btn-sm",
                onclick: move |_| download_source(&source()),
                "Download"
              }
            }
          }
          match save_note() {
              Some(Ok(note)) => rsx! { p { class: "text-success text-sm", "{note}" } },
              Some(Err(err)) => rsx! { p { class: "text-error text-sm break-words", "{err}" } },
              None => rsx! {},
          }
          p { class: "text-xs text-base-content/60",
            "Tab indents; Shift+Tab leaves the editor. Saving keeps a new version and changes "
            "nothing that runs until you test and deploy it."
          }
        }

        section {
          id: "flock-rule-assistant",
          class: "flex flex-col gap-3 min-w-0 bg-base-200/40 border border-base-content/10 rounded-box p-4",
          div { class: "flex flex-wrap items-center gap-2",
            h2 { class: "text-lg font-bold", "Assistant" }
            span { class: "badge badge-ghost badge-sm", "Coming in stage 2" }
          }
          p { class: "text-sm text-base-content/70",
            "In stage 2 of the rules rollout, you will describe a rule in words and the assistant "
            "will draft its code, explain it back to you and test it before you deploy. Until "
            "then, write the code by hand or start from the example."
          }
          textarea {
            class: "textarea textarea-bordered w-full text-sm",
            rows: 4,
            disabled: true,
            "aria-label": "Describe the rule you want",
            placeholder: "Describe the rule you want",
          }
          label { class: "flex items-start gap-2 text-sm",
            input {
              r#type: "checkbox",
              class: "toggle toggle-sm mt-0.5",
              checked: false,
              disabled: true,
            }
            span {
              "Include sample values"
              span { class: "block text-xs text-base-content/60",
                "Off and unavailable: the assistant does not exist yet, and when it does it will "
                "see no telemetry values unless you turn this on."
              }
            }
          }
          button { class: "btn btn-sm self-start", disabled: true, "Draft rule" }
        }
      }

      section { id: "flock-rule-dry-run", class: "flex flex-col gap-3",
        h2 { class: "text-lg font-bold", "Test and deploy" }
        p { class: "text-sm text-base-content/70 max-w-3xl",
          "A test replays the version over this flock's recent telemetry beside the deployed "
          "version and applies nothing. Only a tested version deploys, and a deploy records "
          "every run for the next 24 hours."
        }
        if versions.is_empty() {
          p { class: "text-sm text-base-content/60", "Save a version to test it." }
        } else {
          div { class: "flex flex-wrap items-center gap-3",
            select {
              class: "select select-bordered select-sm",
              "aria-label": "Version to test or deploy",
              onchange: move |evt| target.set(evt.value().parse().ok()),
              for v in versions.iter() {
                option {
                  value: "{v.version}",
                  selected: Some(v.version) == target_version,
                  "{version_label(v.version)}"
                }
              }
            }
            if manage {
              if let Some(version) = target_version {
                button {
                  class: "btn btn-outline btn-sm",
                  disabled: busy(),
                  onclick: move |_| async move {
                      action_note.set(None);
                      busy.set(true);
                      let answer = api::rules::test(flock_id, rule_id, version).await;
                      busy.set(false);
                      match answer {
                          Ok(found) => {
                              if let Some(Ok(d)) = detail.write().as_mut()
                                  && let Some(slot) = d
                                      .versions
                                      .iter_mut()
                                      .find(|v| v.version == version)
                              {
                                  slot.tested_contract = found.tested_contract;
                                  slot.tested_at = found.tested_at;
                              }
                              action_note.set(Some(Ok("Test finished.".to_string())));
                              tested.set(Some(found));
                          }
                          Err(err) => action_note.set(Some(Err(err))),
                      }
                  },
                  "Test"
                }
                label { class: "flex items-center gap-2 text-sm",
                  input {
                    r#type: "checkbox",
                    class: "checkbox checkbox-sm",
                    checked: reset_state(),
                    onchange: move |evt: Event<FormData>| reset_state.set(evt.checked()),
                  }
                  "Reset state on deploy"
                }
                button {
                  class: "btn btn-primary btn-sm",
                  disabled: busy(),
                  onclick: move |_| deploy(version),
                  "Deploy"
                }
              }
            }
          }
        }
        match action_note() {
            Some(Ok(note)) => rsx! { p { class: "text-success text-sm", "{note}" } },
            Some(Err(err)) => rsx! {
              div { role: "alert", class: "alert alert-error alert-soft text-sm break-words", "{err}" }
            },
            None => rsx! {},
        }
        if let Some(summary) = tested().and_then(|v| v.test_summary) {
          JsonViewer { json: summary, title: "Test summary".to_string() }
        }
      }

      section { id: "flock-rule-versions", class: "flex flex-col gap-3",
        h2 { class: "text-lg font-bold", "Versions" }
        if versions.is_empty() {
          p { class: "text-sm text-base-content/60", "No version saved yet." }
        } else {
          div { class: "overflow-x-auto rounded-box border border-base-content/10 bg-base-100",
            table { class: "table table-sm w-full",
              thead {
                tr { class: "bg-base-200/50 text-base-content",
                  th { "Version" }
                  th { "Origin" }
                  th { "Test" }
                  th { "Contract" }
                  th { "Saved" }
                  th { class: "text-right", "Actions" }
                }
              }
              tbody {
                for v in versions.iter().cloned() {
                  tr { key: "{v.version}",
                    td { class: "whitespace-nowrap",
                      span { class: "font-mono", "{version_label(v.version)}" }
                      if rule.deployed_version == Some(v.version) {
                        span { class: "badge badge-success badge-sm ms-2", "Live" }
                      }
                      if rule.pending_version == Some(v.version) {
                        span { class: "badge badge-warning badge-sm ms-2", "Deploying" }
                      }
                    }
                    td { class: "min-w-56",
                      "{origin_label(v.origin)}"
                      if let Some(prompt) = v.prompt.as_deref() {
                        p { class: "text-xs text-base-content/60 break-words max-w-xs",
                          "\u{201c}{prompt}\u{201d}"
                        }
                      }
                    }
                    td {
                      {
                          let badge = test_badge(&v);
                          rsx! {
                            span { class: "badge badge-sm {badge.class} whitespace-nowrap", "{badge.label}" }
                          }
                      }
                    }
                    td { "{v.contract}" }
                    td { class: "text-xs whitespace-nowrap", "{display_utc(v.created_at)}" }
                    td { class: "text-right whitespace-nowrap",
                      {
                          let version = v.version;
                          rsx! {
                            button {
                              class: "btn btn-ghost btn-xs",
                              onclick: move |_| async move {
                                  if let Ok(found) = api::rules::version(flock_id, rule_id, version).await {
                                      let mut open = open_versions.write();
                                      open.retain(|o| o.version != version);
                                      if open.len() == 2 {
                                          open.remove(0);
                                      }
                                      open.push(found);
                                  }
                              },
                              "View"
                            }
                            button {
                              class: "btn btn-ghost btn-xs",
                              onclick: move |_| async move {
                                  if let Ok(found) = api::rules::version(flock_id, rule_id, version).await {
                                      download_source(found.source.as_deref().unwrap_or_default());
                                  }
                              },
                              "Download"
                            }
                            if manage && rule.deployed_version != Some(version) {
                              button {
                                class: "btn btn-ghost btn-xs text-primary",
                                disabled: busy(),
                                onclick: move |_| deploy(version),
                                "Deploy this version"
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
          if !open_versions.read().is_empty() {
            div { class: "grid gap-4 md:grid-cols-2",
              for v in open_versions.read().iter().cloned() {
                div {
                  key: "{v.version}",
                  class: "flex flex-col gap-2 min-w-0 border border-base-content/10 rounded-box p-3 bg-base-100",
                  div { class: "flex items-center justify-between gap-2",
                    span { class: "font-mono font-semibold", "{version_label(v.version)}" }
                    div { class: "flex gap-1",
                      if manage {
                        {
                            let text = v.source.clone().unwrap_or_default();
                            let opened = v.clone();
                            rsx! {
                              button {
                                class: "btn btn-ghost btn-xs",
                                onclick: move |_| {
                                    source.set(text.clone());
                                    base.set(Some(opened.clone()));
                                },
                                "Edit"
                              }
                            }
                        }
                      }
                      {
                          let version = v.version;
                          rsx! {
                            button {
                              class: "btn btn-ghost btn-xs",
                              "aria-label": "Close",
                              onclick: move |_| open_versions.write().retain(|o| o.version != version),
                              Icon { width: 14, height: 14, icon: LdX }
                            }
                          }
                      }
                    }
                  }
                  pre { class: "text-xs font-mono overflow-x-auto max-h-96 bg-base-200/50 rounded p-2",
                    "{v.source.as_deref().unwrap_or_default()}"
                  }
                }
              }
            }
          }
        }
      }

      section { id: "flock-rule-runs", class: "flex flex-col gap-3",
        div { class: "flex flex-wrap items-center justify-between gap-2",
          h2 { class: "text-lg font-bold", "Runs" }
          if manage {
            button {
              class: "btn btn-outline btn-sm",
              onclick: move |_| async move {
                  match api::rules::watch(flock_id, rule_id).await {
                      Ok(updated) => set_rule(updated),
                      Err(err) => action_note.set(Some(Err(err))),
                  }
              },
              "Record every run for 1 hour"
            }
          }
        }
        p { class: "text-sm text-base-content/70 max-w-3xl",
          "Failed runs are always recorded. Quiet successful runs are counted only while the rule "
          "is watched: for 24 hours after a deploy, and for an hour after you ask."
        }
        match runs() {
            None => rsx! { span { class: "loading loading-spinner loading-sm" } },
            Some(Err(err)) => rsx! { p { class: "text-error text-sm break-words", "{err}" } },
            Some(Ok(list)) if list.is_empty() => rsx! {
              p { class: "text-sm text-base-content/60", "No run recorded yet." }
            },
            Some(Ok(list)) => rsx! {
              div {
                class: "flex items-end gap-px h-24 border-b border-base-content/20",
                role: "img",
                "aria-label": "Recorded runs per hour over the last 24 hours",
                for bar in bars.iter() {
                  div {
                    key: "{bar.start}",
                    class: "flex-1 flex flex-col-reverse min-w-0",
                    title: "{bar.ok} ok, {bar.failed} failed, {bar.skipped} skipped",
                    div {
                      class: "bg-success/70",
                      style: "height: {bar.ok * 96 / tallest}px",
                    }
                    div {
                      class: "bg-base-content/30",
                      style: "height: {bar.skipped * 96 / tallest}px",
                    }
                    div {
                      class: "bg-error/80",
                      style: "height: {bar.failed * 96 / tallest}px",
                    }
                  }
                }
              }
              div { class: "flex justify-between text-xs text-base-content/60",
                span { "24 hours ago" }
                span { "now" }
              }
              {
                  let groups = failure_groups(&list);
                  rsx! {
                    if !groups.is_empty() {
                      h3 { class: "font-semibold mt-2", "Failures" }
                      ul { class: "flex flex-col gap-1 text-sm",
                        for group in groups {
                          li { class: "flex flex-wrap gap-2 items-baseline",
                            span { class: "badge badge-error badge-sm", "{group.status}" }
                            span { class: "font-semibold", "{group.count}\u{d7}" }
                            span { class: "font-mono text-xs break-all",
                              "{group.error.as_deref().unwrap_or_default()}"
                            }
                            span { class: "text-xs text-base-content/60",
                              "last {display_utc(group.last_at)}"
                            }
                          }
                        }
                      }
                    }
                  }
              }
              h3 { class: "font-semibold mt-2", "Recorded runs, newest first" }
              div { class: "flex flex-col gap-1",
                for run in list {
                  details { class: "bg-base-100 border border-base-content/10 rounded-box px-3 py-2",
                    summary { class: "cursor-pointer text-sm flex flex-wrap gap-2 items-baseline",
                      span {
                        class: run_badge_class(&run.status),
                        "{run.status}"
                      }
                      span { class: "font-mono text-xs", "{version_label(run.version)}" }
                      span { class: "font-mono text-xs break-all", "{run.pigeon_id}" }
                      span { class: "text-xs text-base-content/60", "{display_utc(run.ran_at)}" }
                    }
                    div { class: "text-xs mt-2 flex flex-col gap-1",
                      p {
                        "{run.readings} readings, {run.alerts_sent} mailed, "
                        "{run.alerts_suppressed} held back, {run.values_written} values"
                      }
                      if let Some(err) = run.error.as_deref() {
                        pre { class: "font-mono whitespace-pre-wrap break-all text-error", "{err}" }
                      }
                      if !run.logs.is_empty() {
                        pre { class: "font-mono whitespace-pre-wrap break-all bg-base-200/50 rounded p-2",
                          "{run.logs.join(\"\\n\")}"
                        }
                      }
                    }
                  }
                }
              }
            },
        }
      }

      section { id: "flock-rule-values", class: "flex flex-col gap-3",
        h2 { class: "text-lg font-bold", "Values" }
        match values() {
            None => rsx! { span { class: "loading loading-spinner loading-sm" } },
            Some(Err(err)) => rsx! { p { class: "text-error text-sm break-words", "{err}" } },
            Some(Ok(list)) if list.is_empty() => rsx! {
              p { class: "text-sm text-base-content/60", "The rule has recorded no value." }
            },
            Some(Ok(list)) => rsx! {
              div { class: "overflow-x-auto rounded-box border border-base-content/10 bg-base-100",
                table { class: "table table-sm w-full",
                  thead {
                    tr { class: "bg-base-200/50 text-base-content",
                      th { "Pigeon" }
                      th { "Key" }
                      th { "Value" }
                      th { "At" }
                    }
                  }
                  tbody {
                    for value in list {
                      tr {
                        td { class: "font-mono text-xs whitespace-nowrap",
                          {
                              session
                                  .pigeons
                                  .read()
                                  .get(&value.pigeon_id)
                                  .and_then(|p| p.name.clone())
                                  .unwrap_or_else(|| value.pigeon_id.clone())
                          }
                        }
                        td { class: "font-mono text-xs whitespace-nowrap", "{value.key}" }
                        td { class: "font-mono text-xs break-all", "{value.value}" }
                        td { class: "text-xs whitespace-nowrap", "{display_utc(value.at)}" }
                      }
                    }
                  }
                }
              }
            },
        }
      }

      if manage {
        RuleSettings { flock_id, rule: rule.clone(), on_saved: set_rule }
        DangerZone { id: "flock-rule-danger-zone",
          DangerAction {
            title: "Delete this rule",
            description: "Deletes its versions, state, values and runs. It stops on the next batch.",
            label: "Delete rule",
            onclick: move |_| show_delete.set(true),
          }
        }
      }
      if show_delete() {
        DeleteRuleModal {
          flock_id,
          rule_id,
          name: rule.name.clone(),
          on_close: move |_| show_delete.set(false),
          on_deleted: move |_| {
              show_delete.set(false);
              nav.replace(Route::FlockRules { flock_id });
          },
        }
      }
    }
  }
}

/// Name, recipients and the on/off switch. Each save answers with the rule, which replaces the
/// page's copy.
#[component]
fn RuleSettings(flock_id: Uuid, rule: Rule, on_saved: EventHandler<Rule>) -> Element {
  let rule_id = rule.id;
  let mut name = use_signal(|| rule.name.clone());
  let mut recipients = use_signal(|| rule.recipients.join(", "));
  let mut note = use_signal(|| None::<Result<&'static str, String>>);
  let enabled = rule.enabled;
  let locked = rule.deployed_version.is_none() || rule.blocked;

  let send = move |req: RuleUpdateRequest, done: &'static str| async move {
    note.set(None);
    match api::rules::update(flock_id, rule_id, &req).await {
      Ok(updated) => {
        on_saved.call(updated);
        note.set(Some(Ok(done)));
      }
      Err(err) => note.set(Some(Err(err))),
    }
  };

  rsx! {
    section {
      id: "flock-rule-settings",
      class: "flex flex-col gap-3 bg-base-100 border border-base-content/10 rounded-box p-4",
      h2 { class: "text-lg font-bold", "Settings" }
      label { class: "flex items-center gap-3 text-sm",
        input {
          r#type: "checkbox",
          class: "toggle toggle-success",
          checked: enabled,
          disabled: locked,
          onchange: move |_| {
              send(
                  RuleUpdateRequest {
                      enabled: Some(!enabled),
                      ..Default::default()
                  },
                  if enabled { "Rule turned off." } else { "Rule turned on." },
              )
          },
        }
        if locked {
          "Enabled (a rule turns on once a version is deployed)"
        } else {
          "Enabled"
        }
      }
      div { class: "grid gap-3 md:grid-cols-2",
        div { class: "flex flex-col gap-1",
          label { class: "fieldset-legend text-xs font-semibold", r#for: "rule-settings-name", "Name" }
          input {
            id: "rule-settings-name",
            class: "input input-bordered w-full",
            maxlength: 120,
            value: "{name}",
            oninput: move |evt| name.set(evt.value()),
          }
        }
        div { class: "flex flex-col gap-1",
          label {
            class: "fieldset-legend text-xs font-semibold",
            r#for: "rule-settings-recipients",
            "Who it mails"
          }
          input {
            id: "rule-settings-recipients",
            class: "input input-bordered w-full",
            placeholder: "The flock owner",
            value: "{recipients}",
            oninput: move |evt| recipients.set(evt.value()),
          }
        }
      }
      button {
        class: "btn btn-sm self-start",
        onclick: move |_| {
            let checked = match normalize_rule_name(&name()) {
                Ok(checked) => checked,
                Err(rejection) => {
                    note.set(Some(Err(rejection.message().to_string())));
                    return;
                }
            };
            spawn(send(
                RuleUpdateRequest {
                    name: Some(checked),
                    recipients: Some(parse_recipients(&recipients())),
                    enabled: None,
                },
                "Saved.",
            ));
        },
        "Save settings"
      }
      match note() {
          Some(Ok(done)) => rsx! { p { class: "text-success text-sm", "{done}" } },
          Some(Err(err)) => rsx! { p { class: "text-error text-sm break-words", "{err}" } },
          None => rsx! {},
      }
    }
  }
}

/// Typed-name confirm, since a delete takes the rule's whole history with it.
#[component]
fn DeleteRuleModal(
  flock_id: Uuid,
  rule_id: Uuid,
  name: String,
  on_close: EventHandler<()>,
  on_deleted: EventHandler<()>,
) -> Element {
  let mut error = use_signal(|| None::<String>);
  rsx! {
    ConfirmModal {
      id: "delete-rule-title",
      title: "Delete rule",
      confirm_label: "Delete",
      confirm_value: Some(name),
      on_close: move |_| on_close.call(()),
      on_confirm: move |_| async move {
          match api::rules::delete(flock_id, rule_id).await {
              Ok(()) => on_deleted.call(()),
              Err(err) => error.set(Some(err)),
          }
      },
      "This deletes the rule with its versions, state, values and runs."
      if let Some(err) = error() {
        span { class: "block text-error mt-2 break-words", "{err}" }
      }
    }
  }
}
