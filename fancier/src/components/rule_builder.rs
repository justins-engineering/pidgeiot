//! The rule builder: a rule as rows of When, Only if, Then and Otherwise, edited in place in the
//! [`RuleModel`] signal the rule page owns and saves. Number and time fields keep their own text
//! while a person types, so the page remounts the form through `generation` whenever rows move,
//! change kind or load.

use crate::helpers::rule_builder_form::{
  BAND_KINDS, CONDITION_KINDS, SEVERITIES, band_op, band_op_value, condition_key, condition_kind,
  new_condition, new_mail, new_value, number_text, operand_from, operand_is_key, operand_text,
  parse_count, parse_number, secs_from, secs_text, set_condition_key, severity_of, severity_value,
  trigger_key, with_kind,
};
use capsules::{
  MAX_MODEL_ACTIONS, MAX_MODEL_BANDS, MAX_MODEL_CONDITIONS, MAX_MODEL_COUNT, RuleAction, RuleBand,
  RuleBranch, RuleCondition, RuleMailRepeat, RuleMatch, RuleModel, RuleOperand, RuleTrigger,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::LdX;

/// The datalist every key field offers the flock's keys from.
const KEYS_LIST_ID: &str = "flock-rule-builder-keys";

const MINUTE: u32 = 60;
const HOUR: u32 = 3_600;

fn actions_mut(model: &mut RuleModel, branch: RuleBranch) -> &mut Vec<RuleAction> {
  match branch {
    RuleBranch::Then => &mut model.then,
    RuleBranch::Otherwise => &mut model.otherwise,
  }
}

/// A text field for a number. It keeps the text as typed, so "4." stays "4." while the model
/// holds 4.
#[component]
fn NumberField(value: f64, label: String, readonly: bool, on_change: EventHandler<f64>) -> Element {
  let mut text = use_signal(|| number_text(value));
  rsx! {
    input {
      class: "input input-bordered input-sm w-24",
      r#type: "text",
      inputmode: "decimal",
      "aria-label": "{label}",
      readonly,
      value: "{text}",
      oninput: move |evt| {
          let typed = evt.value();
          on_change.call(parse_number(&typed));
          text.set(typed);
      },
    }
  }
}

/// A text field for a time in `unit` seconds, such as minutes; blank means none.
#[component]
fn TimeField(
  secs: u32,
  unit: u32,
  label: String,
  readonly: bool,
  on_change: EventHandler<u32>,
) -> Element {
  let mut text = use_signal(|| secs_text(secs, unit));
  rsx! {
    input {
      class: "input input-bordered input-sm w-20",
      r#type: "text",
      inputmode: "decimal",
      placeholder: "0",
      "aria-label": "{label}",
      readonly,
      value: "{text}",
      oninput: move |evt| {
          let typed = evt.value();
          on_change.call(secs_from(&typed, unit));
          text.set(typed);
      },
    }
  }
}

/// A telemetry key, typed or picked from the flock's keys.
#[component]
fn KeyField(
  value: String,
  label: String,
  readonly: bool,
  on_change: EventHandler<String>,
) -> Element {
  rsx! {
    input {
      class: "input input-bordered input-sm w-36",
      r#type: "text",
      list: KEYS_LIST_ID,
      spellcheck: "false",
      autocomplete: "off",
      placeholder: "telemetry key",
      "aria-label": "{label}",
      readonly,
      value: "{value}",
      oninput: move |evt| on_change.call(evt.value()),
    }
  }
}

#[component]
fn RemoveButton(label: String, disabled: bool, onclick: EventHandler<()>) -> Element {
  rsx! {
    button {
      class: "btn btn-ghost btn-xs btn-square ms-auto",
      r#type: "button",
      "aria-label": "{label}",
      title: "{label}",
      disabled,
      onclick: move |_| onclick.call(()),
      Icon { width: 14, height: 14, icon: LdX }
    }
  }
}

/// The value side of a comparison: a typed value, or another key's value.
#[component]
fn OperandField(to: RuleOperand, readonly: bool, on_change: EventHandler<RuleOperand>) -> Element {
  let is_key = operand_is_key(&to);
  let mut text = use_signal(|| operand_text(&to));
  rsx! {
    select {
      class: "select select-bordered select-sm w-auto",
      "aria-label": "Compare with",
      disabled: readonly,
      onchange: move |evt| on_change.call(operand_from(&text.peek(), evt.value() == "key")),
      option { value: "value", selected: !is_key, "the value" }
      option { value: "key", selected: is_key, "the value of" }
    }
    input {
      class: "input input-bordered input-sm w-32",
      r#type: "text",
      list: if is_key { KEYS_LIST_ID } else { "" },
      spellcheck: "false",
      placeholder: if is_key { "telemetry key" } else { "a number or a word" },
      "aria-label": if is_key { "Other key" } else { "Value" },
      readonly,
      value: "{text}",
      oninput: move |evt| {
          let typed = evt.value();
          on_change.call(operand_from(&typed, is_key));
          text.set(typed);
      },
    }
  }
}

/// One Only if row: a key, what to check, and the check's own fields.
#[component]
fn ConditionRow(
  model: Signal<RuleModel>,
  generation: Signal<u32>,
  index: usize,
  row: RuleCondition,
  readonly: bool,
) -> Element {
  let mut model = model;
  let mut generation = generation;
  let kind = condition_kind(&row);
  let mut edit = move |change: &dyn Fn(&mut RuleCondition)| {
    if let Some(c) = model.write().only_if.get_mut(index) {
      change(c);
    }
  };
  let hold = |for_secs: u32| {
    rsx! {
      span { class: "text-sm", "for" }
      TimeField {
        secs: for_secs,
        unit: MINUTE,
        label: "Minutes it must last",
        readonly,
        on_change: move |secs: u32| edit(&|c: &mut RuleCondition| match c {
            RuleCondition::Compare { for_secs, .. } | RuleCondition::Between { for_secs, .. } => {
                *for_secs = secs
            }
            _ => {}
        }),
      }
      span { class: "text-sm text-base-content/70", "minutes" }
    }
  };
  let fields = match row.clone() {
    RuleCondition::Compare { to, for_secs, .. } => rsx! {
      OperandField {
        to,
        readonly,
        on_change: move |operand: RuleOperand| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::Compare { to, .. } = c {
                *to = operand.clone();
            }
        }),
      }
      {hold(for_secs)}
    },
    RuleCondition::Between {
      low,
      high,
      for_secs,
      ..
    } => rsx! {
      NumberField {
        value: low,
        label: "Lowest",
        readonly,
        on_change: move |n: f64| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::Between { low, .. } = c {
                *low = n;
            }
        }),
      }
      span { class: "text-sm", "and" }
      NumberField {
        value: high,
        label: "Highest",
        readonly,
        on_change: move |n: f64| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::Between { high, .. } = c {
                *high = n;
            }
        }),
      }
      {hold(for_secs)}
    },
    RuleCondition::Changed { .. } => rsx! {
      span { class: "text-sm text-base-content/70", "from the value before" }
    },
    RuleCondition::ChangedBy { more_than, .. } => rsx! {
      NumberField {
        value: more_than,
        label: "Amount of change",
        readonly,
        on_change: move |n: f64| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::ChangedBy { more_than, .. } = c {
                *more_than = n;
            }
        }),
      }
    },
    RuleCondition::BitSet { bit, .. } => rsx! {
      NumberField {
        value: f64::from(bit),
        label: "Bit number",
        readonly,
        on_change: move |n: f64| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::BitSet { bit, .. } = c {
                *bit = if n.is_finite() && (0.0..=255.0).contains(&n) { n as u8 } else { u8::MAX };
            }
        }),
      }
      span { class: "text-sm", "set" }
    },
    RuleCondition::Counted {
      more_than,
      within_secs,
      ..
    } => rsx! {
      NumberField {
        value: f64::from(more_than),
        label: "Times",
        readonly,
        on_change: move |n: f64| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::Counted { more_than, .. } = c {
                *more_than = parse_count(&n.to_string(), MAX_MODEL_COUNT + 1);
            }
        }),
      }
      span { class: "text-sm", "times within" }
      TimeField {
        secs: within_secs,
        unit: HOUR,
        label: "Hours to count over",
        readonly,
        on_change: move |secs: u32| edit(&|c: &mut RuleCondition| {
            if let RuleCondition::Counted { within_secs, .. } = c {
                *within_secs = secs;
            }
        }),
      }
      span { class: "text-sm text-base-content/70", "hours" }
    },
  };
  rsx! {
    li { class: "flex flex-wrap items-center gap-2 rounded-box bg-base-200/40 p-2",
      KeyField {
        value: condition_key(&row).to_string(),
        label: "Key to check",
        readonly,
        on_change: move |key: String| {
            edit(&|c: &mut RuleCondition| set_condition_key(c, key.clone()))
        },
      }
      select {
        class: "select select-bordered select-sm w-auto",
        "aria-label": "What to check",
        disabled: readonly,
        onchange: move |evt| {
            if let Some(c) = model.write().only_if.get_mut(index) {
                *c = with_kind(c, &evt.value());
            }
            generation += 1;
        },
        for (value, label) in CONDITION_KINDS {
          option { value, selected: value == kind, "{label}" }
        }
      }
      {fields}
      if !readonly {
        RemoveButton {
          label: "Remove this condition",
          disabled: false,
          onclick: move |_| {
              let mut m = model.write();
              if index < m.only_if.len() {
                  m.only_if.remove(index);
              }
              drop(m);
              generation += 1;
          },
        }
      }
    }
  }
}

/// One row of a value's table: if a key compares to a number, this value.
#[component]
fn BandRow(
  model: Signal<RuleModel>,
  generation: Signal<u32>,
  branch: RuleBranch,
  action: usize,
  index: usize,
  band: RuleBand,
  readonly: bool,
) -> Element {
  let mut model = model;
  let mut generation = generation;
  let mut edit = move |change: &dyn Fn(&mut RuleBand)| {
    if let Some(RuleAction::SetValue { bands, .. }) =
      actions_mut(&mut model.write(), branch).get_mut(action)
      && let Some(b) = bands.get_mut(index)
    {
      change(b);
    }
  };
  let (op, number) = match &band.when {
    RuleCondition::Compare {
      op,
      to: RuleOperand::Number(n),
      ..
    } => (band_op_value(*op), *n),
    _ => ("above", f64::NAN),
  };
  rsx! {
    li { class: "flex flex-wrap items-center gap-2",
      span { class: "text-sm", "if" }
      KeyField {
        value: condition_key(&band.when).to_string(),
        label: "Key for this row",
        readonly,
        on_change: move |key: String| {
            edit(&|b: &mut RuleBand| set_condition_key(&mut b.when, key.clone()))
        },
      }
      select {
        class: "select select-bordered select-sm w-auto",
        "aria-label": "Comparison for this row",
        disabled: readonly,
        onchange: move |evt| {
            let picked = band_op(&evt.value());
            edit(&|b: &mut RuleBand| {
                if let RuleCondition::Compare { op, .. } = &mut b.when {
                    *op = picked;
                }
            })
        },
        for (value, label) in BAND_KINDS {
          option { value, selected: value == op, "{label}" }
        }
      }
      NumberField {
        value: number,
        label: "Number for this row",
        readonly,
        on_change: move |n: f64| edit(&|b: &mut RuleBand| {
            if let RuleCondition::Compare { to, .. } = &mut b.when {
                *to = RuleOperand::Number(n);
            }
        }),
      }
      span { class: "text-sm", "use" }
      input {
        class: "input input-bordered input-sm w-28",
        r#type: "text",
        "aria-label": "Value for this row",
        readonly,
        value: "{band.value}",
        oninput: move |evt| {
            let typed = evt.value();
            edit(&|b: &mut RuleBand| b.value = typed.clone())
        },
      }
      if !readonly {
        RemoveButton {
          label: "Remove this row",
          disabled: false,
          onclick: move |_| {
              let mut m = model.write();
              if let Some(RuleAction::SetValue { bands, .. }) = actions_mut(&mut m, branch)
                  .get_mut(action)
                  && index < bands.len()
              {
                  bands.remove(index);
              }
              generation += 1;
          },
        }
      }
    }
  }
}

/// One Then or Otherwise action: a mail, or a value.
#[component]
fn ActionRow(
  model: Signal<RuleModel>,
  generation: Signal<u32>,
  branch: RuleBranch,
  index: usize,
  action: RuleAction,
  readonly: bool,
) -> Element {
  let mut model = model;
  let mut generation = generation;
  let mut edit = move |change: &dyn Fn(&mut RuleAction)| {
    if let Some(a) = actions_mut(&mut model.write(), branch).get_mut(index) {
      change(a);
    }
  };
  let body = match action.clone() {
    RuleAction::Mail {
      severity,
      subject,
      body,
      repeat,
    } => rsx! {
      div { class: "flex flex-wrap items-center gap-2",
        span { class: "text-sm font-medium", "Send a mail" }
        select {
          class: "select select-bordered select-sm w-auto",
          "aria-label": "How urgent",
          disabled: readonly,
          onchange: move |evt| {
              let picked = severity_of(&evt.value());
              edit(&|a: &mut RuleAction| {
                  if let RuleAction::Mail { severity, .. } = a {
                      *severity = picked;
                  }
              })
          },
          for (value, label) in SEVERITIES {
            option { value, selected: value == severity_value(severity), "{label}" }
          }
        }
        select {
          class: "select select-bordered select-sm w-auto",
          "aria-label": "How often",
          disabled: readonly,
          onchange: move |evt| {
              let once = evt.value() == "once";
              edit(&|a: &mut RuleAction| {
                  if let RuleAction::Mail { repeat, .. } = a {
                      *repeat = if once { RuleMailRepeat::Once } else { RuleMailRepeat::StartStop };
                  }
              })
          },
          option { value: "start_stop", selected: repeat == RuleMailRepeat::StartStop,
            "when it starts, and again when it stops"
          }
          option { value: "once", selected: repeat == RuleMailRepeat::Once,
            "once, each time it starts"
          }
        }
      }
      label { class: "flex flex-col gap-1 text-sm w-full",
        "Subject"
        input {
          class: "input input-bordered input-sm w-full",
          r#type: "text",
          readonly,
          value: "{subject}",
          oninput: move |evt| {
              let typed = evt.value();
              edit(&|a: &mut RuleAction| {
                  if let RuleAction::Mail { subject, .. } = a {
                      *subject = typed.clone();
                  }
              })
          },
        }
      }
      label { class: "flex flex-col gap-1 text-sm w-full",
        "Message (optional)"
        textarea {
          class: "textarea textarea-bordered textarea-sm w-full",
          rows: 2,
          readonly,
          value: "{body}",
          oninput: move |evt| {
              let typed = evt.value();
              edit(&|a: &mut RuleAction| {
                  if let RuleAction::Mail { body, .. } = a {
                      *body = typed.clone();
                  }
              })
          },
        }
      }
    },
    RuleAction::SetValue { key, value, bands } => {
      let rows = bands.len();
      rsx! {
        div { class: "flex flex-wrap items-center gap-2",
          span { class: "text-sm font-medium", "Set the value" }
          input {
            class: "input input-bordered input-sm w-36",
            r#type: "text",
            spellcheck: "false",
            placeholder: "a name, like fan",
            "aria-label": "Value name",
            readonly,
            value: "{key}",
            oninput: move |evt| {
                let typed = evt.value();
                edit(&|a: &mut RuleAction| {
                    if let RuleAction::SetValue { key, .. } = a {
                        *key = typed.clone();
                    }
                })
            },
          }
          span { class: "text-sm", if rows == 0 { "to" } else { "to, when no row matches," } }
          input {
            class: "input input-bordered input-sm w-28",
            r#type: "text",
            "aria-label": "Value",
            readonly,
            value: "{value}",
            oninput: move |evt| {
                let typed = evt.value();
                edit(&|a: &mut RuleAction| {
                    if let RuleAction::SetValue { value, .. } = a {
                        *value = typed.clone();
                    }
                })
            },
          }
        }
        if rows > 0 {
          ul { class: "flex flex-col gap-2 ps-4 border-s border-base-content/10",
            for (j , band) in bands.into_iter().enumerate() {
              BandRow {
                model,
                generation,
                branch,
                action: index,
                index: j,
                band,
                readonly,
              }
            }
          }
        }
        if !readonly && rows < MAX_MODEL_BANDS {
          button {
            class: "btn btn-ghost btn-xs self-start",
            r#type: "button",
            onclick: move |_| {
                let trigger = trigger_key(&model.peek()).to_string();
                let mut m = model.write();
                if let Some(RuleAction::SetValue { bands, .. }) = actions_mut(&mut m, branch)
                    .get_mut(index)
                {
                    bands.push(RuleBand {
                        when: new_condition(&trigger),
                        value: String::new(),
                    });
                }
                generation += 1;
            },
            "Add a row for another value"
          }
        }
      }
    }
  };
  rsx! {
    li { class: "flex flex-col gap-2 rounded-box bg-base-200/40 p-2",
      div { class: "flex items-start gap-2",
        div { class: "flex flex-col gap-2 min-w-0 grow", {body} }
        if !readonly {
          RemoveButton {
            label: "Remove this action",
            disabled: false,
            onclick: move |_| {
                let mut m = model.write();
                let list = actions_mut(&mut m, branch);
                if index < list.len() {
                    list.remove(index);
                }
                drop(m);
                generation += 1;
            },
          }
        }
      }
    }
  }
}

/// The actions of one branch, with the buttons that add to it.
#[component]
fn ActionList(
  model: Signal<RuleModel>,
  generation: Signal<u32>,
  branch: RuleBranch,
  readonly: bool,
) -> Element {
  let mut model = model;
  let actions = match branch {
    RuleBranch::Then => model.read().then.clone(),
    RuleBranch::Otherwise => model.read().otherwise.clone(),
  };
  let room = actions.len() < MAX_MODEL_ACTIONS;
  rsx! {
    if actions.is_empty() {
      p { class: "text-sm text-base-content/60", "Nothing yet." }
    }
    ul { class: "flex flex-col gap-2",
      for (i , action) in actions.into_iter().enumerate() {
        ActionRow {
          model,
          generation,
          branch,
          index: i,
          action,
          readonly,
        }
      }
    }
    if !readonly && room {
      div { class: "flex flex-wrap gap-2",
        button {
          class: "btn btn-outline btn-xs",
          r#type: "button",
          onclick: move |_| actions_mut(&mut model.write(), branch).push(new_mail()),
          "Add a mail"
        }
        button {
          class: "btn btn-outline btn-xs",
          r#type: "button",
          onclick: move |_| actions_mut(&mut model.write(), branch).push(new_value()),
          "Add a value"
        }
      }
    }
  }
}

/// The builder form over `model`. `keys` are the flock's reported keys for the pickers; a key
/// not reported yet can still be typed.
#[component]
pub fn RuleBuilder(
  model: Signal<RuleModel>,
  generation: Signal<u32>,
  keys: Vec<String>,
  readonly: bool,
) -> Element {
  let mut model = model;
  let current = model.read().clone();
  let trigger = trigger_key(&current).to_string();
  let known = keys.iter().any(|k| *k == trigger) || keys.is_empty();
  rsx! {
    // Dioxus compares keys only between siblings, so the form is a list of one to remount.
    for g in [generation()] {
      div { key: "{g}", class: "flex flex-col gap-4",
        datalist { id: KEYS_LIST_ID,
          for key in keys.iter() {
            option { value: "{key}" }
          }
        }
        div { class: "flex flex-col gap-2",
          h3 { class: "font-semibold", "When" }
          div { class: "flex flex-wrap items-center gap-2 rounded-box bg-base-200/40 p-2",
            span { class: "text-sm", "a pigeon in this flock reports" }
            KeyField {
              value: trigger.clone(),
              label: "Key that starts the rule",
              readonly,
              on_change: move |key: String| model.write().when = RuleTrigger::Telemetry { key },
            }
          }
          if !known && !trigger.trim().is_empty() {
            p { class: "text-xs text-base-content/60",
              "No pigeon in this flock has reported this key in the last day. The rule still "
              "runs once one does."
            }
          }
          p { class: "text-xs text-base-content/60",
            "The rule looks at each report that carries this key, the moment it arrives."
          }
        }
        div { class: "flex flex-col gap-2",
          div { class: "flex flex-wrap items-center gap-2",
            h3 { class: "font-semibold", "Only if" }
            select {
              class: "select select-bordered select-sm w-auto",
              "aria-label": "How the conditions combine",
              disabled: readonly,
              onchange: move |evt| {
                  let any = evt.value() == "any";
                  model.write().r#match = if any { RuleMatch::Any } else { RuleMatch::All };
              },
              option { value: "all", selected: current.r#match == RuleMatch::All,
                "all of these are true"
              }
              option { value: "any", selected: current.r#match == RuleMatch::Any,
                "any of these is true"
              }
            }
          }
          if current.only_if.is_empty() {
            p { class: "text-sm text-base-content/60", "No conditions: every report acts." }
          }
          ul { class: "flex flex-col gap-2",
            for (i , row) in current.only_if.iter().cloned().enumerate() {
              ConditionRow { model, generation, index: i, row, readonly }
            }
          }
          if !readonly && current.only_if.len() < MAX_MODEL_CONDITIONS {
            button {
              class: "btn btn-outline btn-xs self-start",
              r#type: "button",
              onclick: move |_| {
                  let key = trigger_key(&model.peek()).to_string();
                  model.write().only_if.push(new_condition(&key));
              },
              "Add a condition"
            }
          }
        }
        div { class: "flex flex-col gap-2",
          h3 { class: "font-semibold", "Then" }
          ActionList { model, generation, branch: RuleBranch::Then, readonly }
        }
        div { class: "flex flex-col gap-2",
          if current.otherwise.is_empty() {
            if !readonly {
              button {
                class: "btn btn-ghost btn-xs self-start",
                r#type: "button",
                onclick: move |_| model.write().otherwise.push(new_value()),
                "Add otherwise: what to do while the conditions are not met"
              }
            }
          } else {
            h3 { class: "font-semibold", "Otherwise" }
            ActionList { model, generation, branch: RuleBranch::Otherwise, readonly }
          }
        }
        p { class: "text-xs text-base-content/60",
          "In a subject, message or value, {{pigeon}} and {{flock}} become their names, and a key "
          "in braces, like {{temp_c}}, becomes its latest value."
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::helpers::rule_builder_form::starter_model;
  use capsules::{RuleCompareOp, RuleOperand};
  use dioxus::dioxus_core::VirtualDom;
  use std::cell::RefCell;

  thread_local! {
    static PAGE: RefCell<Option<(Signal<RuleModel>, Signal<u32>)>> = const { RefCell::new(None) };
    static START: RefCell<Option<RuleModel>> = const { RefCell::new(None) };
  }

  /// The rule page's part: it mounts the form on the starter, or on `START` when a test set one,
  /// and owns the signals a load writes.
  #[component]
  fn Page() -> Element {
    let model = use_signal(|| {
      START
        .with(|s| s.borrow_mut().take())
        .unwrap_or_else(starter_model)
    });
    let generation = use_signal(|| 0u32);
    use_hook(|| PAGE.with(|p| *p.borrow_mut() = Some((model, generation))));
    rsx! {
      RuleBuilder { model, generation, keys: Vec::new(), readonly: false }
    }
  }

  /// The `value` of every input labelled `label` in the page as rendered now.
  fn values_labelled(dom: &VirtualDom, label: &str) -> Vec<String> {
    let html = dioxus_ssr::render(dom);
    let value = regex::Regex::new(r#"value="([^"]*)""#).expect("value pattern");
    let mut labelled = String::with_capacity(label.len() + 13);
    labelled.push_str("aria-label=\"");
    labelled.push_str(label);
    labelled.push('"');
    html
      .split("<input")
      .skip(1)
      .filter_map(|rest| rest.split_once('>').map(|(tag, _)| tag))
      .filter(|tag| tag.contains(&labelled))
      .map(|tag| {
        value
          .captures(tag)
          .map_or(String::new(), |c| c[1].to_string())
      })
      .collect()
  }

  #[test]
  fn a_loaded_rule_without_a_hold_shows_an_empty_hold_field() {
    let mut dom = VirtualDom::new(Page);
    dom.rebuild_in_place();
    assert_eq!(values_labelled(&dom, "Minutes it must last"), ["5"]);

    let mut stored = starter_model();
    stored.only_if = vec![RuleCondition::Compare {
      key: "temp_c".to_string(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(40.0),
      for_secs: 0,
    }];
    dom.in_runtime(|| {
      let (mut model, mut generation) = PAGE.with(|p| p.borrow().expect("page mounted"));
      model.set(stored);
      generation += 1;
    });
    dom.render_immediate(&mut dioxus::dioxus_core::NoOpMutations);
    assert_eq!(values_labelled(&dom, "Minutes it must last"), [""]);
  }

  fn held(key: &str, for_secs: u32) -> RuleCondition {
    RuleCondition::Compare {
      key: key.to_string(),
      op: RuleCompareOp::Above,
      to: RuleOperand::Number(40.0),
      for_secs,
    }
  }

  #[test]
  fn removing_a_condition_leaves_the_next_one_its_own_hold() {
    let mut start = starter_model();
    start.only_if = vec![held("temp_c", 120), held("rh", 600)];
    START.with(|s| *s.borrow_mut() = Some(start));
    let mut dom = VirtualDom::new(Page);
    dom.rebuild_in_place();
    assert_eq!(values_labelled(&dom, "Minutes it must last"), ["2", "10"]);

    // What "Remove this condition" does on the first row.
    dom.in_runtime(|| {
      let (mut model, mut generation) = PAGE.with(|p| p.borrow().expect("page mounted"));
      model.write().only_if.remove(0);
      generation += 1;
    });
    dom.render_immediate(&mut dioxus::dioxus_core::NoOpMutations);
    assert_eq!(values_labelled(&dom, "Minutes it must last"), ["10"]);
  }
}
