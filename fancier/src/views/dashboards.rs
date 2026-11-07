//! Custom dashboards: the list of the ones an account can open, and one dashboard with its edit
//! mode. The built-in fleet overview at `/dashboard` stays a fixed page linked from the list, so
//! it can never be edited into a broken state.
//!
//! State is built from each write's response and never re-read to confirm it (CLAUDE.md, the
//! Hyperdrive client rule). Edit mode works on a draft; a save names the revision it loaded and
//! a 409 shows what was saved since instead of writing over it.

use crate::api::dashboards::{DashboardRead, SaveOutcome};
use crate::api::{self, telemetry};
use crate::components::{TimeRange, WidgetContent};
use crate::helpers::dashboard_layout::{
  KINDS, default_title, fresh_widget_id, kind_of, move_widget, put_widget, remove_widget,
  size_class, time_range, widget_range,
};
use crate::{LocalSession, Route};
use capsules::{
  Dashboard, DashboardCreateRequest, DashboardLayout, DashboardSummary, DashboardUpdateRequest,
  OrgRole, Widget, WidgetBody, WidgetChart, WidgetRange, WidgetScope, WidgetSize, validate_layout,
};
use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::ld_icons::{LdArrowDown, LdArrowUp, LdLayoutDashboard};
use std::collections::BTreeSet;
use uuid::Uuid;

/// How a dashboard's owner reads in a badge.
fn owner_label(local: &LocalSession, org_id: Option<Uuid>) -> String {
  match org_id {
    None => "Personal".to_string(),
    Some(org) => local
      .orgs
      .read()
      .get(&org)
      .map(|m| m.organization.name.clone())
      .unwrap_or_else(|| "Organization".to_string()),
  }
}

#[component]
pub fn DashboardList() -> Element {
  let local = use_context::<LocalSession>();
  let nav = use_navigator();
  let list = use_resource(api::dashboards::list);
  let mut creating = use_signal(|| false);
  let mut name = use_signal(String::new);
  let mut owner = use_signal(String::new);
  let mut error = use_signal(|| None::<String>);
  let mut busy = use_signal(|| false);

  let managed_orgs: Vec<(Uuid, String)> = {
    let mut orgs: Vec<(Uuid, String)> = local
      .orgs
      .read()
      .values()
      .filter(|m| matches!(m.role, OrgRole::Owner | OrgRole::Admin))
      .map(|m| (m.organization.id, m.organization.name.clone()))
      .collect();
    orgs.sort_by(|a, b| a.1.cmp(&b.1));
    orgs
  };

  let submit = move |evt: FormEvent| async move {
    evt.prevent_default();
    busy.set(true);
    error.set(None);
    let request = DashboardCreateRequest {
      name: name(),
      org_id: Uuid::parse_str(&owner()).ok(),
      layout: None,
    };
    match api::dashboards::create(&request).await {
      Ok(created) => {
        nav.push(Route::DashboardView {
          dashboard_id: created.id,
        });
      }
      Err(message) => error.set(Some(message)),
    }
    busy.set(false);
  };

  rsx! {
    section { id: "dashboards-list",
      header { class: "flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-8",
        div {
          h1 { class: "text-xl font-bold", "Dashboards" }
          p { class: "text-sm text-base-content/60",
            "Pages of widgets you arrange yourself. An organization's dashboards are shared with every member."
          }
        }
        button {
          class: "btn btn-primary btn-outline",
          onclick: move |_| creating.set(!creating()),
          "New dashboard"
        }
      }

      if creating() {
        form {
          id: "dashboards-create",
          class: "card bg-base-100 border border-base-content/10 p-5 mb-8 flex flex-col gap-3 max-w-xl",
          onsubmit: submit,
          label { class: "fieldset",
            span { class: "fieldset-legend", "Name" }
            input {
              class: "input w-full",
              r#type: "text",
              maxlength: "80",
              required: true,
              placeholder: "Depot 3",
              value: "{name}",
              oninput: move |e| name.set(e.value()),
            }
          }
          label { class: "fieldset",
            span { class: "fieldset-legend", "Belongs to" }
            select {
              class: "select w-full",
              value: "{owner}",
              onchange: move |e| owner.set(e.value()),
              option { value: "", "Me (personal)" }
              for (id , org_name) in managed_orgs.iter() {
                option { value: "{id}", "{org_name} (shared with its members)" }
              }
            }
          }
          if let Some(message) = error() {
            p { class: "text-error text-sm", "{message}" }
          }
          div { class: "flex gap-2",
            button { class: "btn btn-primary", r#type: "submit", disabled: busy(), "Create" }
            button {
              class: "btn btn-ghost",
              r#type: "button",
              onclick: move |_| creating.set(false),
              "Cancel"
            }
          }
        }
      }

      div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 mb-16",
        Link { to: Route::Dashboard {},
          div { class: "card bg-base-100 border border-base-content/10 hover:border-primary/40 p-5 h-full",
            div { class: "flex items-center gap-2",
              Icon { icon: LdLayoutDashboard, class: "size-5 text-primary" }
              h2 { class: "font-semibold", "Fleet overview" }
            }
            p { class: "text-sm text-base-content/60 mt-2",
              "The built-in summary of every flock you can see."
            }
          }
        }
        match &*list.read() {
          None => rsx! {
            div { class: "loading loading-spinner text-primary m-6" }
          },
          Some(None) => rsx! {
            div { class: "card bg-base-100 border border-warning/40 p-5",
              p { class: "text-sm", "Your dashboards could not be loaded. Try again in a moment." }
            }
          },
          Some(Some(dashboards)) => rsx! {
            for summary in dashboards.iter() {
              DashboardCard {
                key: "{summary.id}",
                summary: summary.clone(),
                owner: owner_label(&local, summary.org_id),
              }
            }
          },
        }
      }
    }
  }
}

#[component]
fn DashboardCard(summary: DashboardSummary, owner: String) -> Element {
  let widgets = if summary.widget_count == 1 {
    "1 widget".to_string()
  } else {
    format!("{} widgets", summary.widget_count)
  };
  rsx! {
    Link {
      to: Route::DashboardView {
          dashboard_id: summary.id,
      },
      div { class: "card bg-base-100 border border-base-content/10 hover:border-primary/40 p-5 h-full",
        h2 { class: "font-semibold break-words", "{summary.name}" }
        div { class: "flex flex-wrap items-center gap-2 mt-2 text-xs",
          span { class: "badge badge-ghost badge-sm", "{owner}" }
          span { class: "text-base-content/60", "{widgets}" }
          if !summary.can_edit {
            span { class: "text-base-content/60", "View only" }
          }
        }
      }
    }
  }
}

#[component]
pub fn DashboardView(dashboard_id: Uuid) -> Element {
  let local = use_context::<LocalSession>();
  let nav = use_navigator();
  let mut stored = use_signal(|| None::<Dashboard>);
  let mut state = use_signal(|| "loading");
  let mut draft = use_signal(|| None::<Dashboard>);
  let mut conflict = use_signal(|| None::<Dashboard>);
  let mut error = use_signal(|| None::<String>);
  let mut editor = use_signal(|| None::<(Widget, bool)>);
  let mut confirm_delete = use_signal(|| false);
  let mut busy = use_signal(|| false);

  use_effect(use_reactive((&dashboard_id,), move |(dashboard_id,)| {
    spawn(async move {
      state.set("loading");
      draft.set(None);
      match api::dashboards::get(dashboard_id).await {
        DashboardRead::Found(dashboard) => {
          stored.set(Some(dashboard));
          state.set("ready");
        }
        DashboardRead::Missing => state.set("missing"),
        DashboardRead::Unavailable => state.set("unavailable"),
      }
    });
  }));

  match state() {
    "missing" => {
      return rsx! {
        section { id: "dashboard-missing", class: "max-w-xl mx-auto text-center py-16",
          h1 { class: "text-xl font-bold", "Dashboard not found" }
          p { class: "text-base-content/60 mt-2",
            "It may have been deleted, or it belongs to an organization you are not a member of."
          }
          Link { class: "btn btn-outline mt-6", to: Route::DashboardList {}, "All dashboards" }
        }
      };
    }
    "unavailable" => {
      return rsx! {
        section { id: "dashboard-unavailable", class: "max-w-xl mx-auto text-center py-16",
          h1 { class: "text-xl font-bold", "This dashboard could not be loaded" }
          p { class: "text-base-content/60 mt-2", "Try again in a moment." }
        }
      };
    }
    "loading" => {
      return rsx! {
        div { class: "loading loading-spinner text-primary m-10" }
      };
    }
    _ => {}
  }
  let Some(saved) = stored() else {
    return rsx! {};
  };
  let editing = draft.read().is_some();
  let shown = draft().unwrap_or_else(|| saved.clone());
  let widget_count = shown.layout.widgets.len();

  let save = move |_| async move {
    let Some(current) = draft() else {
      return;
    };
    busy.set(true);
    error.set(None);
    let request = DashboardUpdateRequest {
      name: current.name.clone(),
      layout: current.layout.clone(),
      revision: current.revision,
    };
    match api::dashboards::save(current.id, &request).await {
      SaveOutcome::Saved(saved) => {
        stored.set(Some(saved));
        draft.set(None);
        conflict.set(None);
      }
      SaveOutcome::Conflict(theirs) => conflict.set(Some(theirs)),
      SaveOutcome::Failed(message) => error.set(Some(message)),
    }
    busy.set(false);
  };

  let copy = move |_| async move {
    busy.set(true);
    match api::dashboards::copy(dashboard_id).await {
      Ok(copied) => {
        nav.push(Route::DashboardView {
          dashboard_id: copied.id,
        });
      }
      Err(message) => error.set(Some(message)),
    }
    busy.set(false);
  };

  let delete = move |_| async move {
    busy.set(true);
    match api::dashboards::delete(dashboard_id).await {
      Ok(()) => {
        nav.replace(Route::DashboardList {});
      }
      Err(message) => error.set(Some(message)),
    }
    busy.set(false);
  };

  rsx! {
    section { id: "dashboard-header", class: "flex flex-col gap-3 mb-6",
      div { class: "flex flex-col lg:flex-row lg:items-center justify-between gap-3",
        div { class: "min-w-0",
          if editing {
            input {
              class: "input input-lg w-full font-bold",
              "aria-label": "Dashboard name",
              maxlength: "80",
              value: "{shown.name}",
              oninput: move |e| {
                  if let Some(d) = draft.write().as_mut() {
                      d.name = e.value();
                  }
              },
            }
          } else {
            h1 { class: "text-2xl font-bold break-words", "{shown.name}" }
          }
          div { class: "flex flex-wrap items-center gap-2 mt-1 text-xs",
            span { class: "badge badge-ghost badge-sm", {owner_label(&local, shown.org_id)} }
            if saved.unreadable {
              span { class: "text-base-content/60", "Cannot be shown or changed here." }
            } else if !saved.can_edit {
              span { class: "text-base-content/60", "View only. Copy it to make your own changes." }
            }
          }
        }
        div { class: "flex flex-wrap gap-2",
          if editing {
            button {
              class: "btn btn-primary btn-sm",
              disabled: busy(),
              onclick: save,
              "Save"
            }
            button {
              class: "btn btn-sm",
              disabled: busy(),
              onclick: move |_| {
                  let id = draft
                      .read()
                      .as_ref()
                      .map(|d| fresh_widget_id(&d.layout))
                      .unwrap_or_default();
                  editor.set(Some((Widget {
                      id,
                      size: WidgetSize::Half,
                      title: String::new(),
                      body: WidgetBody::Text { text: String::new() },
                  }, true)));
              },
              "Add widget"
            }
            button {
              class: "btn btn-ghost btn-sm",
              disabled: busy(),
              onclick: move |_| {
                  draft.set(None);
                  conflict.set(None);
                  error.set(None);
              },
              "Cancel"
            }
          } else {
            if saved.can_edit {
              button {
                class: "btn btn-primary btn-outline btn-sm",
                onclick: move |_| draft.set(stored()),
                "Edit"
              }
            }
            if !saved.unreadable {
              button {
                class: "btn btn-outline btn-sm",
                disabled: busy(),
                onclick: copy,
                "Copy to my dashboards"
              }
            }
            Link { class: "btn btn-ghost btn-sm", to: Route::DashboardList {}, "All dashboards" }
          }
        }
      }
      if let Some(message) = error() {
        div { role: "alert", class: "alert alert-error alert-soft text-sm", "{message}" }
      }
      if let Some(theirs) = conflict() {
        div {
          role: "alert",
          class: "alert alert-warning text-sm flex flex-col sm:flex-row gap-2 items-start sm:items-center",
          span { "Someone saved this dashboard after you opened it. Load their version, or save yours over it." }
          div { class: "flex gap-2",
            button {
              class: "btn btn-sm",
              onclick: {
                  let theirs = theirs.clone();
                  move |_| {
                      stored.set(Some(theirs.clone()));
                      draft.set(Some(theirs.clone()));
                      conflict.set(None);
                  }
              },
              "Load theirs"
            }
            button {
              class: "btn btn-sm btn-warning",
              onclick: {
                  let theirs = theirs.clone();
                  move |_| {
                      if let Some(d) = draft.write().as_mut() {
                          d.revision = theirs.revision;
                      }
                      stored.set(Some(theirs.clone()));
                      conflict.set(None);
                  }
              },
              "Keep mine"
            }
          }
        }
      }
    }

    section { id: "dashboard-widgets",
      if saved.unreadable {
        div { class: "card bg-base-100 border border-base-content/10 p-10 text-center",
          p { class: "font-semibold", "This dashboard could not be read" }
          p { class: "text-sm text-base-content/60 mt-1",
            "It was saved in a form this version of the dashboard does not know. Its widgets are kept as they are; reload later to see them."
          }
        }
      } else if widget_count == 0 {
        div { class: "card bg-base-100 border border-base-content/10 p-10 text-center",
          p { class: "font-semibold", "No widgets yet" }
          p { class: "text-sm text-base-content/60 mt-1",
            if saved.can_edit {
              "Choose Edit, then Add widget, to put charts, readings and notes on this page."
            } else {
              "An owner or admin of this organization can add widgets."
            }
          }
        }
      } else {
        div { class: "grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4",
          for (index , widget) in shown.layout.widgets.iter().cloned().enumerate() {
            WidgetCard {
              key: "{widget.id}",
              widget: widget.clone(),
              editing,
              first: index == 0,
              last: index + 1 == widget_count,
              on_move: move |up: bool| {
                  if let Some(d) = draft.write().as_mut() {
                      move_widget(&mut d.layout, index, up);
                  }
              },
              on_size: move |size: WidgetSize| {
                  if let Some(d) = draft.write().as_mut()
                      && let Some(w) = d.layout.widgets.get_mut(index)
                  {
                      w.size = size;
                  }
              },
              on_edit: move |w: Widget| editor.set(Some((w, false))),
              on_remove: move |id: String| {
                  if let Some(d) = draft.write().as_mut() {
                      remove_widget(&mut d.layout, &id);
                  }
              },
            }
          }
        }
      }
    }

    if editing && saved.can_edit {
      section { id: "dashboard-danger", class: "mt-10",
        if confirm_delete() {
          div { class: "flex flex-wrap items-center gap-2 text-sm",
            span { "Delete \"{saved.name}\" for everyone who can see it?" }
            button { class: "btn btn-error btn-sm", disabled: busy(), onclick: delete, "Delete" }
            button { class: "btn btn-ghost btn-sm", onclick: move |_| confirm_delete.set(false), "Keep it" }
          }
        } else {
          button {
            class: "btn btn-ghost btn-sm text-error",
            onclick: move |_| confirm_delete.set(true),
            "Delete dashboard"
          }
        }
      }
    }

    if let Some((widget, is_new)) = editor() {
      WidgetEditor {
        initial: widget,
        is_new,
        on_save: move |widget: Widget| {
            if let Some(d) = draft.write().as_mut() {
                put_widget(&mut d.layout, widget);
            }
            editor.set(None);
        },
        on_cancel: move |_| editor.set(None),
      }
    }
  }
}

#[component]
fn WidgetCard(
  widget: Widget,
  editing: bool,
  first: bool,
  last: bool,
  on_move: EventHandler<bool>,
  on_size: EventHandler<WidgetSize>,
  on_edit: EventHandler<Widget>,
  on_remove: EventHandler<String>,
) -> Element {
  let title = if widget.title.trim().is_empty() {
    default_title(&widget.body)
  } else {
    widget.title.clone()
  };
  rsx! {
    div {
      class: "{size_class(widget.size)} card bg-base-100 border border-base-content/10 p-4 flex flex-col gap-3 min-w-0",
      "data-widget": "{widget.id}",
      div { class: "flex items-start justify-between gap-2",
        h2 { class: "font-semibold break-words min-w-0", "{title}" }
        if editing {
          div { class: "flex flex-wrap items-center justify-end gap-1 shrink-0",
            button {
              class: "btn btn-ghost btn-xs btn-square",
              "aria-label": "Move up",
              disabled: first,
              onclick: move |_| on_move.call(true),
              Icon { icon: LdArrowUp, width: 14, height: 14 }
            }
            button {
              class: "btn btn-ghost btn-xs btn-square",
              "aria-label": "Move down",
              disabled: last,
              onclick: move |_| on_move.call(false),
              Icon { icon: LdArrowDown, width: 14, height: 14 }
            }
          }
        }
      }
      if editing {
        div { class: "flex flex-wrap items-center gap-2",
          select {
            class: "select select-xs w-auto",
            "aria-label": "Widget size",
            onchange: move |e| {
                if let Some(size) = WidgetSize::ALL.into_iter().find(|s| s.label() == e.value()) {
                    on_size.call(size);
                }
            },
            for size in WidgetSize::ALL {
              option { value: "{size.label()}", selected: size == widget.size, "{size.label()}" }
            }
          }
          if !matches!(widget.body, WidgetBody::Unknown) {
            button {
              class: "btn btn-ghost btn-xs",
              onclick: {
                  let widget = widget.clone();
                  move |_| on_edit.call(widget.clone())
              },
              "Edit"
            }
          }
          button {
            class: "btn btn-ghost btn-xs text-error",
            onclick: {
                let id = widget.id.clone();
                move |_| on_remove.call(id.clone())
            },
            "Remove"
          }
        }
      }
      WidgetContent { body: widget.body.clone() }
    }
  }
}

/// Which pigeon or flock a widget being edited reads, decomposed for the form.
fn flock_of_pigeon(local: &LocalSession, pigeon: &str) -> Option<Uuid> {
  if let Some(p) = local.pigeons.read().get(pigeon) {
    return Some(p.flock_id);
  }
  local
    .flocks
    .read()
    .iter()
    .find(|(_, f)| f.pigeon_ids.iter().any(|id| id == pigeon))
    .map(|(id, _)| *id)
}

/// The editor's form state, seeded from the widget being edited.
#[derive(Clone, PartialEq)]
struct Form {
  kind: &'static str,
  title: String,
  size: WidgetSize,
  flock: Option<Uuid>,
  pigeon: String,
  flock_scope: bool,
  keys: Vec<String>,
  range: WidgetRange,
  chart: WidgetChart,
  unit: String,
  min: String,
  max: String,
  text: String,
}

impl Form {
  fn from_widget(local: &LocalSession, widget: &Widget) -> Self {
    let mut form = Form {
      kind: kind_of(&widget.body),
      title: widget.title.clone(),
      size: widget.size,
      flock: None,
      pigeon: String::new(),
      flock_scope: false,
      keys: Vec::new(),
      range: WidgetRange::default(),
      chart: WidgetChart::default(),
      unit: String::new(),
      min: "0".into(),
      max: "100".into(),
      text: String::new(),
    };
    match &widget.body {
      WidgetBody::TimeSeries {
        scope,
        keys,
        range,
        chart,
      } => {
        match scope {
          WidgetScope::Pigeon(p) => {
            form.pigeon = p.clone();
            form.flock = flock_of_pigeon(local, p);
          }
          WidgetScope::Flock(f) => {
            form.flock = Some(*f);
            form.flock_scope = true;
          }
        }
        form.keys = keys.clone();
        form.range = *range;
        form.chart = *chart;
      }
      WidgetBody::Latest { pigeon, key, unit } => {
        form.pigeon = pigeon.clone();
        form.flock = flock_of_pigeon(local, pigeon);
        form.keys = vec![key.clone()];
        form.unit = unit.clone();
      }
      WidgetBody::Threshold {
        pigeon,
        key,
        unit,
        min,
        max,
      } => {
        form.pigeon = pigeon.clone();
        form.flock = flock_of_pigeon(local, pigeon);
        form.keys = vec![key.clone()];
        form.unit = unit.clone();
        form.min = min.to_string();
        form.max = max.to_string();
      }
      WidgetBody::StatusTable { flock, keys } => {
        form.flock = Some(*flock);
        form.keys = keys.clone();
      }
      WidgetBody::ShadowKey { pigeon, key } => {
        form.pigeon = pigeon.clone();
        form.flock = flock_of_pigeon(local, pigeon);
        form.keys = vec![key.clone()];
      }
      WidgetBody::Alerts { flock } => form.flock = Some(*flock),
      WidgetBody::Text { text } => form.text = text.clone(),
      WidgetBody::Unknown => {}
    }
    form
  }

  /// Whether this kind reads one pigeon (as opposed to a flock, or nothing).
  fn wants_pigeon(&self) -> bool {
    match self.kind {
      "time_series" => !self.flock_scope,
      "latest" | "threshold" | "shadow_key" => true,
      _ => false,
    }
  }

  fn wants_flock(&self) -> bool {
    self.kind != "text"
  }

  /// How many keys the picker allows: none, exactly one, or several.
  fn key_limit(&self) -> usize {
    match self.kind {
      "time_series" if self.flock_scope => 1,
      "time_series" | "status_table" => capsules::MAX_WIDGET_KEYS,
      "latest" | "threshold" | "shadow_key" => 1,
      _ => 0,
    }
  }

  fn into_widget(self, id: String) -> Result<Widget, String> {
    let need_flock = || self.flock.ok_or_else(|| "Choose a flock.".to_string());
    let need_pigeon = || {
      if self.pigeon.is_empty() {
        Err("Choose a pigeon.".to_string())
      } else {
        Ok(self.pigeon.clone())
      }
    };
    let one_key = || {
      self
        .keys
        .first()
        .cloned()
        .ok_or_else(|| "Choose a key.".to_string())
    };
    let body = match self.kind {
      "time_series" => WidgetBody::TimeSeries {
        scope: if self.flock_scope {
          WidgetScope::Flock(need_flock()?)
        } else {
          WidgetScope::Pigeon(need_pigeon()?)
        },
        keys: self.keys.clone(),
        range: self.range,
        chart: self.chart,
      },
      "latest" => WidgetBody::Latest {
        pigeon: need_pigeon()?,
        key: one_key()?,
        unit: self.unit.trim().to_string(),
      },
      "threshold" => WidgetBody::Threshold {
        pigeon: need_pigeon()?,
        key: one_key()?,
        unit: self.unit.trim().to_string(),
        min: self
          .min
          .trim()
          .parse()
          .map_err(|_| "Minimum must be a number.".to_string())?,
        max: self
          .max
          .trim()
          .parse()
          .map_err(|_| "Maximum must be a number.".to_string())?,
      },
      "status_table" => WidgetBody::StatusTable {
        flock: need_flock()?,
        keys: self.keys.clone(),
      },
      "shadow_key" => WidgetBody::ShadowKey {
        pigeon: need_pigeon()?,
        key: one_key()?,
      },
      "alerts" => WidgetBody::Alerts {
        flock: need_flock()?,
      },
      _ => WidgetBody::Text {
        text: self.text.clone(),
      },
    };
    let widget = Widget {
      id,
      size: self.size,
      title: self.title.trim().to_string(),
      body,
    };
    let check = DashboardLayout {
      widgets: vec![widget.clone()],
      ..DashboardLayout::default()
    };
    match validate_layout(&check) {
      Ok(()) => Ok(widget),
      Err(rejection) => Err(
        rejection
          .message()
          .trim_start_matches("Bad Request: ")
          .to_string(),
      ),
    }
  }
}

/// What a key picker's choices depend on: the kind, the chosen pigeon or the flock's pigeons.
/// Kept apart from the rest of the form so typing a title or a unit refetches nothing.
#[derive(Clone, PartialEq)]
struct KeySource {
  kind: &'static str,
  wants_pigeon: bool,
  pigeon: String,
  flock_pigeons: Vec<String>,
}

/// The keys a picker offers: what the chosen pigeon has reported (or, at flock scope, any
/// pigeon in the flock), or the top-level keys of its config for a config widget.
async fn key_choices(source: KeySource) -> Vec<String> {
  let mut keys = BTreeSet::new();
  if source.kind == "shadow_key" {
    if source.pigeon.is_empty() {
      return Vec::new();
    }
    if let Some(detail) = api::pigeons::get_detail(&source.pigeon).await {
      for config in [&detail.shadow.target_config, &detail.shadow.current_config] {
        if let Ok(serde_json::Value::Object(map)) =
          serde_json::from_str::<serde_json::Value>(&config.to_string())
        {
          keys.extend(map.keys().cloned());
        }
      }
    }
  } else if source.wants_pigeon {
    if source.pigeon.is_empty() {
      return Vec::new();
    }
    if let Some(latest) = telemetry::get_latest(&source.pigeon).await {
      keys.extend(latest.into_iter().map(|t| t.key));
    }
  } else if !source.flock_pigeons.is_empty()
    && let Some(batch) = api::dashboards::latest_telemetry(&source.flock_pigeons, &[]).await
  {
    keys.extend(batch.into_iter().flat_map(|p| p.telemetry).map(|t| t.key));
  }
  keys.into_iter().collect()
}

#[component]
fn WidgetEditor(
  initial: Widget,
  is_new: bool,
  on_save: EventHandler<Widget>,
  on_cancel: EventHandler<()>,
) -> Element {
  let local = use_context::<LocalSession>();
  let mut form = use_signal(|| Form::from_widget(&local, &initial));
  let mut error = use_signal(|| None::<String>);
  let widget_id = initial.id.clone();

  let mut flocks: Vec<(Uuid, String)> = local
    .flocks
    .read()
    .iter()
    .map(|(id, f)| (*id, f.name.clone()))
    .collect();
  flocks.sort_by(|a, b| a.1.cmp(&b.1));
  let flock_pigeons: Vec<String> = form()
    .flock
    .and_then(|f| local.flocks.read().get(&f).map(|f| f.pigeon_ids.clone()))
    .unwrap_or_default();

  // Fills the pigeon cache for the chosen flock so the picker can show names.
  let pigeons_for_flock = flock_pigeons.clone();
  let loaded = use_resource(use_reactive(
    (&pigeons_for_flock,),
    |(pigeons_for_flock,)| async move {
      if !pigeons_for_flock.is_empty() {
        api::pigeons::list(&pigeons_for_flock).await;
      }
    },
  ));
  let _ = loaded;

  // Reads the flock's pigeons inside the memo, so choosing a flock reruns it.
  let key_source = use_memo(move || {
    let form = form();
    KeySource {
      kind: form.kind,
      wants_pigeon: form.wants_pigeon(),
      pigeon: form.pigeon.clone(),
      flock_pigeons: form
        .flock
        .and_then(|f| local.flocks.read().get(&f).map(|f| f.pigeon_ids.clone()))
        .unwrap_or_default(),
    }
  });
  let choices = use_resource(move || key_choices(key_source()));

  let f = form();
  let pigeon_options: Vec<(String, String)> = {
    let cache = local.pigeons.read();
    flock_pigeons
      .iter()
      .map(|id| {
        let label = cache
          .get(id)
          .and_then(|p| p.name.clone())
          .filter(|n| !n.trim().is_empty())
          .unwrap_or_else(|| id.chars().take(12).collect());
        (id.clone(), label)
      })
      .collect()
  };
  let key_limit = f.key_limit();

  rsx! {
    div { class: "modal modal-open modal-bottom sm:modal-middle", role: "dialog",
      div { id: "dashboard-widget-editor", class: "modal-box flex flex-col gap-3 max-w-lg",
        h3 { class: "font-bold text-lg",
          if is_new { "Add widget" } else { "Edit widget" }
        }
        label { class: "fieldset",
          span { class: "fieldset-legend", "Kind" }
          select {
            class: "select w-full",
            "aria-label": "Widget kind",
            onchange: move |e| {
                if let Some((tag, _, size)) = KINDS.iter().find(|(tag, _, _)| *tag == e.value()) {
                    let mut next = form();
                    next.kind = tag;
                    next.keys.clear();
                    if is_new {
                        next.size = *size;
                    }
                    form.set(next);
                }
            },
            for (tag , label , _) in KINDS {
              option { value: "{tag}", selected: f.kind == tag, "{label}" }
            }
          }
        }
        label { class: "fieldset",
          span { class: "fieldset-legend", "Title (optional)" }
          input {
            class: "input w-full",
            maxlength: "80",
            value: "{f.title}",
            oninput: move |e| form.write().title = e.value(),
          }
        }
        if f.kind == "time_series" {
          label { class: "label gap-2 cursor-pointer w-fit",
            input {
              r#type: "checkbox",
              class: "toggle toggle-sm",
              checked: f.flock_scope,
              onchange: move |e| {
                  let mut next = form();
                  next.flock_scope = e.checked();
                  next.keys.clear();
                  form.set(next);
              },
            }
            span { class: "text-sm", "One line per pigeon across the whole flock" }
          }
        }
        if f.wants_flock() {
          label { class: "fieldset",
            span { class: "fieldset-legend", "Flock" }
            select {
              class: "select w-full",
              "aria-label": "Flock",
              onchange: move |e| {
                  let mut next = form();
                  next.flock = Uuid::parse_str(&e.value()).ok();
                  next.pigeon.clear();
                  next.keys.clear();
                  form.set(next);
              },
              option { value: "", selected: f.flock.is_none(), "Choose a flock" }
              for (id , name) in flocks.iter() {
                option { value: "{id}", selected: f.flock == Some(*id), "{name}" }
              }
            }
          }
        }
        if f.wants_pigeon() && f.flock.is_some() {
          label { class: "fieldset",
            span { class: "fieldset-legend", "Pigeon" }
            select {
              class: "select w-full",
              "aria-label": "Pigeon",
              onchange: move |e| {
                  let mut next = form();
                  next.pigeon = e.value();
                  next.keys.clear();
                  form.set(next);
              },
              option { value: "", selected: f.pigeon.is_empty(), "Choose a pigeon" }
              for (id , label) in pigeon_options.iter() {
                option { value: "{id}", selected: f.pigeon == *id, "{label}" }
              }
            }
          }
        }
        if key_limit > 0 && (f.flock.is_some() || f.wants_pigeon()) {
          fieldset { class: "fieldset",
            legend { class: "fieldset-legend",
              if key_limit == 1 { "Key" } else { "Keys" }
            }
            match &*choices.read() {
              None => rsx! {
                span { class: "loading loading-spinner loading-xs" }
              },
              Some(keys) if keys.is_empty() => rsx! {
                p { class: "text-sm text-base-content/60",
                  if f.wants_pigeon() && f.pigeon.is_empty() {
                    "Choose a pigeon to see its keys."
                  } else if f.kind == "shadow_key" {
                    "This pigeon's config has no keys yet."
                  } else {
                    "Nothing has been reported here yet, so there are no keys to choose."
                  }
                }
              },
              Some(keys) => rsx! {
                div { class: "flex flex-wrap gap-2",
                  for key in keys.iter().cloned() {
                    label {
                      key: "{key}",
                      class: "label gap-1.5 cursor-pointer border border-base-content/15 rounded-field px-2 py-1",
                      input {
                        r#type: if key_limit == 1 { "radio" } else { "checkbox" },
                        class: if key_limit == 1 { "radio radio-xs" } else { "checkbox checkbox-xs" },
                        name: "widget-key",
                        checked: f.keys.contains(&key),
                        onchange: {
                            let key = key.clone();
                            move |e: FormEvent| {
                                let mut next = form();
                                if key_limit == 1 {
                                    next.keys = vec![key.clone()];
                                } else if e.checked() {
                                    if !next.keys.contains(&key) && next.keys.len() < key_limit {
                                        next.keys.push(key.clone());
                                    }
                                } else {
                                    next.keys.retain(|k| *k != key);
                                }
                                form.set(next);
                            }
                        },
                      }
                      span { class: "font-mono text-xs", "{key}" }
                    }
                  }
                }
              },
            }
          }
        }
        if f.kind == "time_series" {
          div { class: "grid grid-cols-2 gap-2",
            label { class: "fieldset",
              span { class: "fieldset-legend", "Range" }
              select {
                class: "select w-full",
                onchange: move |e| {
                    if let Some(range) = TimeRange::from_label(&e.value()) {
                        form.write().range = widget_range(range);
                    }
                },
                for r in TimeRange::ALL {
                  option { value: "{r.label()}", selected: r == time_range(f.range), "{r.label()}" }
                }
              }
            }
            label { class: "fieldset",
              span { class: "fieldset-legend", "Chart" }
              select {
                class: "select w-full",
                onchange: move |e| {
                    let chart = match e.value().as_str() {
                        "Step" => WidgetChart::Step,
                        "Area" => WidgetChart::Area,
                        "Bar" => WidgetChart::Bar,
                        "Scatter" => WidgetChart::Scatter,
                        _ => WidgetChart::Line,
                    };
                    form.write().chart = chart;
                },
                for (label , chart) in [
                    ("Line", WidgetChart::Line),
                    ("Step", WidgetChart::Step),
                    ("Area", WidgetChart::Area),
                    ("Bar", WidgetChart::Bar),
                    ("Scatter", WidgetChart::Scatter),
                ]
                {
                  option { value: "{label}", selected: f.chart == chart, "{label}" }
                }
              }
            }
          }
        }
        if matches!(f.kind, "latest" | "threshold") {
          label { class: "fieldset",
            span { class: "fieldset-legend", "Unit (optional)" }
            input {
              class: "input w-full",
              maxlength: "12",
              placeholder: "°C",
              value: "{f.unit}",
              oninput: move |e| form.write().unit = e.value(),
            }
          }
        }
        if f.kind == "threshold" {
          div { class: "grid grid-cols-2 gap-2",
            label { class: "fieldset",
              span { class: "fieldset-legend", "Minimum" }
              input {
                class: "input w-full",
                inputmode: "decimal",
                value: "{f.min}",
                oninput: move |e| form.write().min = e.value(),
              }
            }
            label { class: "fieldset",
              span { class: "fieldset-legend", "Maximum" }
              input {
                class: "input w-full",
                inputmode: "decimal",
                value: "{f.max}",
                oninput: move |e| form.write().max = e.value(),
              }
            }
          }
          p { class: "text-xs text-base-content/60",
            "The bar turns red while a threshold alert on this key, the pigeon's own or its flock's, is firing for it."
          }
        }
        if f.kind == "text" {
          label { class: "fieldset",
            span { class: "fieldset-legend", "Text" }
            textarea {
              class: "textarea w-full h-28",
              maxlength: "2000",
              placeholder: "Site address, who to call, what this dashboard is for",
              value: "{f.text}",
              oninput: move |e| form.write().text = e.value(),
            }
          }
        }
        if let Some(message) = error() {
          p { class: "text-error text-sm", "{message}" }
        }
        div { class: "modal-action",
          button {
            class: "btn btn-ghost",
            onclick: move |_| on_cancel.call(()),
            "Cancel"
          }
          button {
            class: "btn btn-primary",
            onclick: move |_| match form().into_widget(widget_id.clone()) {
                Ok(widget) => on_save.call(widget),
                Err(message) => error.set(Some(message)),
            },
            if is_new { "Add" } else { "Done" }
          }
        }
      }
      div { class: "modal-backdrop", onclick: move |_| on_cancel.call(()) }
    }
  }
}
