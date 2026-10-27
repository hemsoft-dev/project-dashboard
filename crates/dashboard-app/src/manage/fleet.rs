//! The Fleet dialog (#36): pmd's machines, subscriptions and agents, a tab each, with an editor that creates or
//! replaces one row through `PUT` and a quick Enable or Disable on each agent.
//!
//! The lists read with the read token, so the dialog opens without write access; it is then read-only and names the
//! missing scope. Left and Right switch tabs on the tab strip, Up and Down move through a list, Enter edits the row,
//! Ctrl+Enter saves, and Esc closes with the unsaved-edit question. A save sends the whole row: `If-Match` to replace,
//! `If-None-Match: *` to create. There is no delete; the editor says to disable an agent or give a machine 0 sessions.
//! The rows, their checks and the body live in `dashboard_core::fleet`.
use std::sync::Arc;

use dashboard_core::edit::{Form, Next};
use dashboard_core::fleet::{self, CAPABILITIES, FleetRow, Kind, Known, OSES, ROLES};
use dashboard_core::write::{CreateCheck, Feature, Method, Precondition, WriteError, WriteRequest, write_state};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Window, div, rems,
};

use super::work_items::{chip, field_row, read_only_note};
use super::{
    Content, Control, Dialog, FLEET_TABS, FleetNextTab, FleetPreviousTab, Frame, Manage, OpenItem, WORK_LIST, WorkDown,
    WorkUp, button, conflict_panel, control,
};

pub struct FleetDialog {
    tab: Kind,
    tabs: FocusHandle,
    list: FocusHandle,
    scroll: ScrollHandle,
    rows: [Vec<FleetRow>; 3],
    loading: [bool; 3],
    load_error: [Option<String>; 3],
    selected: [usize; 3],
    add: Control,
    /// One focusable Enable/Disable per agent row, rebuilt when the agents load.
    toggles: Vec<Control>,
    /// A quick toggle's failure, or a create that landed despite timing out.
    message: Option<(String, bool)>,
    /// Agents whose toggle is on its way, so a second click can't race the first with an older version.
    toggling: Vec<String>,
    retry: Control,
    editor: Option<FleetEditor>,
    editors_opened: u64,
}

pub struct FleetEditor {
    key: u64,
    kind: Kind,
    /// The row as pmd has it; `None` while creating.
    row: Option<FleetRow>,
    form: Form,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    /// Focus handles for the choice chips, by field: os, metered, enabled, machine, subscription, suggestions.
    choices: Vec<(&'static str, Vec<Control>)>,
    back: Control,
    save: Control,
    reapply: Control,
    discard: Control,
}

fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Machine => 0,
        Kind::Subscription => 1,
        Kind::Agent => 2,
    }
}

/// The fields typed as text; the others are chosen with chips.
fn text_fields(kind: Kind, creating: bool) -> Vec<&'static str> {
    let fields: &[&str] = match kind {
        Kind::Machine => &["name", "tailnetIp", "maxSessions", "capabilities"],
        Kind::Subscription => &["id", "provider", "plan"],
        Kind::Agent => &["provider", "alias", "model", "roles"],
    };
    fields
        .iter()
        .copied()
        .filter(|field| creating || !kind.key_fields().contains(field))
        .collect()
}

impl FleetDialog {
    pub(super) fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(|editor| editor.form.is_dirty())
    }

    pub(super) fn focus_tabs(&self, window: &mut Window, cx: &mut Context<Manage>) {
        self.tabs.focus(window, cx);
    }

    fn known(&self) -> Known {
        Known {
            machines: self.rows[0].iter().map(|row| row.key.clone()).collect(),
            subscriptions: self.rows[1].iter().map(|row| row.key.clone()).collect(),
        }
    }

    #[cfg(test)]
    pub fn tab(&self) -> Kind {
        self.tab
    }

    #[cfg(test)]
    pub fn rows(&self, kind: Kind) -> &[FleetRow] {
        &self.rows[slot(kind)]
    }

    #[cfg(test)]
    pub fn editor_form(&self) -> Option<&Form> {
        self.editor.as_ref().map(|editor| &editor.form)
    }

    #[cfg(test)]
    pub(super) fn input(&self, field: &str) -> Option<Entity<InputState>> {
        self.editor
            .as_ref()?
            .inputs
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, input)| input.clone())
    }
}

impl FleetEditor {
    fn sync_inputs(&self, window: &mut Window, cx: &mut Context<Manage>) {
        for (field, input) in &self.inputs {
            let text = self.form.value(field).to_owned();
            if input.read(cx).value() != text.as_str() {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    fn sync_disabled(&self, writable: bool, cx: &mut Context<Manage>) {
        let locked = !writable || self.form.is_saving() || self.form.conflict_state().is_some();
        for (_, input) in &self.inputs {
            input.update(cx, |input, cx| input.set_disabled(locked, cx));
        }
    }

    fn locked(&self, writable: bool) -> bool {
        !writable || self.form.is_saving() || self.form.conflict_state().is_some()
    }
}

impl Manage {
    fn fleet_writable(&self) -> bool {
        write_state(Feature::Fleet, &self.access).is_ok()
    }

    fn fleet_of(&mut self, id: u64) -> Option<&mut FleetDialog> {
        match self.content_of(id) {
            Some(Content::Fleet(dialog)) => Some(dialog),
            _ => None,
        }
    }

    fn open_fleet(&mut self) -> Option<(u64, &mut FleetDialog)> {
        let dialog = self.dialog.as_mut()?;
        match &mut dialog.content {
            Content::Fleet(fleet) => Some((dialog.id, fleet)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn fleet(&self) -> Option<&FleetDialog> {
        match &self.dialog.as_ref()?.content {
            Content::Fleet(fleet) => Some(fleet),
            _ => None,
        }
    }

    pub(super) fn new_fleet(&mut self, cx: &mut Context<Self>) -> FleetDialog {
        if matches!(
            self.access,
            dashboard_core::write::Access::Unchecked | dashboard_core::write::Access::Failed(_)
        ) {
            self.check_access(cx);
        }
        FleetDialog {
            tab: Kind::Machine,
            tabs: cx.focus_handle().tab_stop(true),
            list: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            loading: [true; 3],
            load_error: Default::default(),
            selected: [0; 3],
            add: control(cx),
            toggles: Vec::new(),
            message: None,
            toggling: Vec::new(),
            retry: control(cx),
            editor: None,
            editors_opened: 0,
        }
    }

    /// Loads one tab's list, or all three.
    pub(super) fn load_fleet(&mut self, id: u64, kinds: &[Kind], cx: &mut Context<Self>) {
        for kind in kinds.iter().copied() {
            let backend = Arc::clone(&self.backend);
            let Some(fleet) = self.fleet_of(id) else {
                return;
            };
            fleet.loading[slot(kind)] = true;
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { backend.read(kind.list_path()) })
                    .await
                    .and_then(|value| FleetRow::list_from_json(kind, &value));
                let _ = this.update(cx, |manage, cx| {
                    let Some(fleet) = manage.fleet_of(id) else {
                        return;
                    };
                    let ix = slot(kind);
                    fleet.loading[ix] = false;
                    match result {
                        Ok(rows) => {
                            fleet.load_error[ix] = None;
                            fleet.rows[ix] = merge(rows, &fleet.rows[ix]);
                            fleet.selected[ix] = fleet.selected[ix].min(fleet.rows[ix].len().saturating_sub(1));
                            sync_controls(fleet, cx);
                        }
                        Err(error) => fleet.load_error[ix] = Some(error.to_string()),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Reloads lists of the open Fleet dialog.
    pub fn reload_fleet(&mut self, kinds: &[Kind], cx: &mut Context<Self>) {
        if let Some((id, _)) = self.open_fleet() {
            self.load_fleet(id, kinds, cx);
        }
    }

    pub fn show_fleet_tab(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if let Some((_, fleet)) = self.open_fleet() {
            fleet.tab = kind;
            fleet.scroll.scroll_to_item(fleet.selected[slot(kind)]);
        }
        cx.notify();
    }

    fn step_fleet_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((_, fleet)) = self.open_fleet() else {
            return;
        };
        let next = (slot(fleet.tab) as isize + delta).rem_euclid(3) as usize;
        self.show_fleet_tab(Kind::ALL[next], cx);
    }

    fn move_in_fleet_list(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((_, fleet)) = self.open_fleet() else {
            return;
        };
        let ix = slot(fleet.tab);
        if fleet.rows[ix].is_empty() {
            return;
        }
        let last = fleet.rows[ix].len() as isize - 1;
        fleet.selected[ix] = (fleet.selected[ix] as isize + delta).clamp(0, last) as usize;
        fleet.scroll.scroll_to_item(fleet.selected[ix]);
        cx.notify();
    }

    /// Opens the editor on a row of the current tab, or on a new row with `None`.
    pub fn open_fleet_editor(&mut self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.fleet_writable();
        if row.is_none() && !writable {
            return;
        }
        let Some((id, fleet)) = self.open_fleet() else {
            return;
        };
        let kind = fleet.tab;
        let row = match row {
            Some(ix) => match fleet.rows[slot(kind)].get(ix) {
                Some(found) => {
                    fleet.selected[slot(kind)] = ix;
                    Some(found.clone())
                }
                None => return,
            },
            None => None,
        };
        fleet.editors_opened += 1;
        let key = fleet.editors_opened;
        let known = fleet.known();
        let form = match &row {
            Some(found) => Form::edit(found.row(), kind.fields()),
            None => Form::create(kind.fields(), &kind.defaults()),
        };
        let inputs: Vec<(&'static str, Entity<InputState>)> = text_fields(kind, row.is_none())
            .into_iter()
            .map(|field| {
                let value = form.value(field).to_owned();
                let placeholder = match field {
                    "tailnetIp" => "Empty when unknown",
                    "capabilities" | "roles" => "Words separated by commas",
                    "plan" => "Empty for none",
                    "provider" if kind == Kind::Agent => "The harness: claude, codex, …",
                    "provider" => "anthropic, openai, google, …",
                    _ => "",
                };
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                });
                (field, input)
            })
            .collect();
        for (field, input) in &inputs {
            let field = *field;
            cx.observe(input, move |manage, input, cx| {
                let text = input.read(cx).value().to_string();
                let Some(editor) = manage.fleet_of(id).and_then(|fleet| fleet.editor.as_mut()) else {
                    return;
                };
                if editor.key == key && editor.form.value(field) != text {
                    editor.form.set(field, text);
                    cx.notify();
                }
            })
            .detach();
        }
        let controls = |count: usize, cx: &mut Context<Self>| (0..count).map(|_| control(cx)).collect::<Vec<_>>();
        let choices = match kind {
            Kind::Machine => vec![
                ("os", controls(OSES.len(), cx)),
                ("capabilities", controls(CAPABILITIES.len(), cx)),
            ],
            Kind::Subscription => vec![("metered", controls(2, cx))],
            Kind::Agent => vec![
                ("machine", controls(known.machines.len(), cx)),
                ("subscription", controls(known.subscriptions.len(), cx)),
                ("roles", controls(ROLES.len(), cx)),
                ("enabled", controls(2, cx)),
            ],
        };
        let editor = FleetEditor {
            key,
            kind,
            row,
            form,
            inputs,
            choices,
            back: control(cx),
            save: control(cx),
            reapply: control(cx),
            discard: control(cx),
        };
        editor.sync_disabled(writable, cx);
        match editor.inputs.first() {
            Some((_, input)) if writable => {
                let input = input.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            _ => editor.back.focus.focus(window, cx),
        }
        fleet.editor = Some(editor);
        cx.notify();
    }

    pub fn fleet_back_to_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_fleet().is_some_and(|(_, fleet)| fleet.is_dirty()) {
            self.ask_before_discarding(true, window, cx);
            return;
        }
        self.leave_fleet_editor(window, cx);
    }

    pub fn leave_fleet_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, fleet)) = self.open_fleet() {
            fleet.editor = None;
            fleet.list.focus(window, cx);
        }
        cx.notify();
    }

    /// Picks a value with a chip, or adds a suggested word to a list field.
    fn set_fleet_field(&mut self, field: &'static str, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.fleet_writable();
        let Some(editor) = self.open_fleet().and_then(|(_, fleet)| fleet.editor.as_mut()) else {
            return;
        };
        if editor.locked(writable) {
            return;
        }
        let text = if matches!(field, "capabilities" | "roles") {
            fleet::add_word(editor.form.value(field), value)
        } else {
            value.to_owned()
        };
        editor.form.set(field, text);
        editor.sync_inputs(window, cx);
        cx.notify();
    }

    /// Ctrl+Enter or Save: checks the row, then one `PUT` of all of it.
    pub fn save_fleet_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.fleet_writable();
        let backend = Arc::clone(&self.backend);
        let Some((id, fleet)) = self.open_fleet() else {
            return;
        };
        let known = fleet.known();
        let Some(editor) = fleet.editor.as_mut() else {
            return;
        };
        if editor.locked(writable) || !editor.form.is_dirty() {
            return;
        }
        let kind = editor.kind;
        let (key, body) = match fleet::body(kind, editor.form.values(), &known) {
            Ok(found) => found,
            Err(problems) => {
                for (field, reason) in problems {
                    editor.form.reject(field, reason);
                }
                cx.notify();
                return;
            }
        };
        let Some(precondition) = editor.form.begin_save() else {
            cx.notify();
            return;
        };
        editor.sync_disabled(writable, cx);
        let editor_key = editor.key;
        let path = kind.path(&key);
        let request = WriteRequest {
            method: Method::Put,
            path: path.clone(),
            precondition,
            body,
        };
        cx.spawn_in(window, async move |this, cx| {
            let reader = Arc::clone(&backend);
            let result = cx
                .background_executor()
                .spawn(async move { backend.write(&request) })
                .await;
            let next = this.update_in(cx, |manage, window, cx| {
                let writable = manage.fleet_writable();
                let fleet = manage.fleet_of(id)?;
                let result = result.and_then(|value| FleetRow::from_json(kind, &value));
                // pmd took the write: the list shows it even when the editor closed while it was on its way.
                if let Ok(saved) = &result {
                    upsert(fleet, saved.clone(), cx);
                }
                // A create that timed out but landed is read back whether or not its editor is still open, so the
                // list gets the row; only the form updates below need that editor.
                let found = matches!(
                    result,
                    Err(WriteError::Timeout {
                        create: Some(CreateCheck::Exists)
                    })
                );
                let Some(editor) = fleet.editor.as_mut().filter(|editor| editor.key == editor_key) else {
                    return found.then_some(Next::Show);
                };
                let next = match result {
                    Ok(saved) => {
                        editor.form.saved(saved.row());
                        editor.sync_inputs(window, cx);
                        // After a create, the key fields are fixed: `reopen_created` rebuilds the editor on the row.
                        editor.row = Some(saved);
                        None
                    }
                    // The create probably landed: stay busy until the stored row is read back, so nothing typed
                    // meanwhile is overwritten and no second create is sent.
                    Err(WriteError::Timeout {
                        create: Some(CreateCheck::Exists),
                    }) => {
                        editor
                            .form
                            .checking("pmd didn't answer in time, but the row exists now. Loading it…");
                        Some(Next::Show)
                    }
                    Err(error) => {
                        let next = match fleet::error_field(kind, &error) {
                            Some(field) => {
                                editor.form.failed_on(field, &error);
                                Next::Show
                            }
                            None => editor.form.failed(&error),
                        };
                        (next == Next::Reread).then_some(Next::Reread)
                    }
                };
                editor.sync_disabled(writable, cx);
                cx.notify();
                next
            });
            let next = match next {
                Ok(Some(next)) => next,
                _ => {
                    // A create becomes an edit of the stored row, with its key fields read-only.
                    let _ = this.update_in(cx, |manage, window, cx| {
                        manage.reopen_created(id, editor_key, window, cx)
                    });
                    return;
                }
            };
            // Changed elsewhere, or a create that timed out but landed: read the row with the read token.
            let fresh = cx
                .background_executor()
                .spawn(async move { reader.read(&path) })
                .await
                .and_then(|value| FleetRow::from_json(kind, &value));
            let _ = this.update_in(cx, |manage, window, cx| {
                let writable = manage.fleet_writable();
                let Some(fleet) = manage.fleet_of(id) else {
                    return;
                };
                // pmd's current row goes in the list, so it never shows a version that is gone or misses a row that
                // was created.
                if let Ok(fresh) = &fresh {
                    upsert(fleet, fresh.clone(), cx);
                }
                let Some(editor) = fleet.editor.as_mut().filter(|editor| editor.key == editor_key) else {
                    cx.notify();
                    return;
                };
                let mut created = false;
                match (next, fresh) {
                    (Next::Reread, Ok(row)) => {
                        editor.form.conflict(row.row());
                        editor.row = Some(row);
                        editor.reapply.focus.focus(window, cx);
                    }
                    (Next::Reread, Err(error)) => editor.form.reread_failed(&error),
                    // The create landed: what pmd stored is saved, and the editor edits it from here.
                    (_, Ok(row)) => {
                        editor.form.saved(row.row());
                        editor.row = Some(row);
                        created = true;
                    }
                    // Whether it landed is unknown: the list reloads, and the timeout's message says to check.
                    (_, Err(_)) => {
                        editor.form.failed(&WriteError::Timeout {
                            create: Some(CreateCheck::Unknown),
                        });
                        manage.load_fleet(id, &[kind], cx);
                    }
                }
                if let Some(editor) = manage
                    .fleet_of(id)
                    .and_then(|fleet| fleet.editor.as_mut())
                    .filter(|editor| editor.key == editor_key)
                {
                    editor.sync_disabled(writable, cx);
                }
                if created {
                    manage.reopen_created(id, editor_key, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// After a create landed, the editor's text inputs for the key go away: rebuild it on the stored row.
    fn reopen_created(&mut self, id: u64, editor_key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fleet) = self.fleet_of(id) else {
            return;
        };
        let Some(editor) = fleet.editor.as_ref().filter(|editor| editor.key == editor_key) else {
            return;
        };
        let has_key_inputs = editor
            .inputs
            .iter()
            .any(|(field, _)| editor.kind.key_fields().contains(field));
        if editor.row.is_none() || !has_key_inputs || editor.form.is_dirty() {
            return;
        }
        let key = editor.row.as_ref().map(|row| row.key.clone()).unwrap_or_default();
        let kind = editor.kind;
        fleet.tab = kind;
        let Some(ix) = fleet.rows[slot(kind)].iter().position(|row| row.key == key) else {
            return;
        };
        fleet.editor = None;
        self.open_fleet_editor(Some(ix), window, cx);
    }

    pub fn reapply_fleet_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.open_fleet().and_then(|(_, fleet)| fleet.editor.as_mut()) {
            editor.form.reapply();
        }
        self.save_fleet_row(window, cx);
    }

    pub fn discard_fleet_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.fleet_writable();
        if let Some(editor) = self.open_fleet().and_then(|(_, fleet)| fleet.editor.as_mut()) {
            editor.form.discard();
            editor.sync_inputs(window, cx);
            editor.sync_disabled(writable, cx);
            editor.back.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// The quick Enable or Disable on an agent row: the whole row with `enabled` flipped, `If-Match` its version.
    pub fn toggle_agent(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.fleet_writable() {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let Some((id, fleet)) = self.open_fleet() else {
            return;
        };
        let known = fleet.known();
        let Some(row) = fleet.rows[2].get(ix).cloned() else {
            return;
        };
        let mut values = row.values.clone();
        let enable = row.value("enabled") != "true";
        values.insert("enabled".to_owned(), enable.to_string());
        let Ok((key, body)) = fleet::body(Kind::Agent, &values, &known) else {
            fleet.message = Some((format!("Agent {} can't be saved as it is; edit it.", row.key), true));
            cx.notify();
            return;
        };
        if fleet.toggling.contains(&row.key) {
            return;
        }
        fleet.toggling.push(row.key.clone());
        fleet.message = None;
        let request = WriteRequest {
            method: Method::Put,
            path: Kind::Agent.path(&key),
            precondition: Precondition::IfMatch(row.version),
            body,
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.write(&request) })
                .await
                .and_then(|value| FleetRow::from_json(Kind::Agent, &value));
            let _ = this.update(cx, |manage, cx| {
                let Some(fleet) = manage.fleet_of(id) else {
                    return;
                };
                fleet.toggling.retain(|key| *key != row.key);
                match result {
                    Ok(saved) => upsert(fleet, saved, cx),
                    Err(WriteError::StaleVersion) => {
                        fleet.message = Some((
                            format!("Agent {} changed elsewhere; reloaded it. Try again.", row.key),
                            false,
                        ));
                        manage.load_fleet(id, &[Kind::Agent], cx);
                    }
                    Err(error) => fleet.message = Some((format!("Agent {}: {error}", row.key), true)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Enter on the dialog outside the list and the controls: edit the selected row when the list shows.
    pub(super) fn fleet_default_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, fleet)) = self.open_fleet() else {
            return;
        };
        if fleet.editor.is_none() {
            let selected = fleet.selected[slot(fleet.tab)];
            self.open_fleet_editor(Some(selected), window, cx);
        }
    }

    pub(super) fn render_fleet(
        &self,
        dialog: &Dialog,
        fleet: &FleetDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let writable = write_state(Feature::Fleet, &self.access);
        let close = button(
            "dialog-cancel",
            "Close",
            &dialog.cancel.focus,
            false,
            false,
            false,
            |manage, window, cx| manage.request_close(window, cx),
            cx,
        );
        if let Some(editor) = &fleet.editor {
            return self.render_fleet_editor(dialog, fleet, editor, writable, close, window, cx);
        }
        let theme = cx.theme().clone();
        let ix = slot(fleet.tab);
        let tabs_focused = fleet.tabs.is_focused(window);
        let ring = theme.ring;
        let tabs = h_flex()
            .id("fleet-tabs")
            .test_support()
            .role(Role::TabList)
            .aria_label("Fleet")
            .key_context(FLEET_TABS)
            .track_focus(&fleet.tabs)
            .tab_index(0)
            .gap_1()
            .p_1()
            .rounded(theme.radius)
            .border_2()
            .border_color(gpui_kit::transparent_black())
            .focus(move |style| style.border_color(ring))
            .on_action(cx.listener(|manage, _: &FleetPreviousTab, _, cx| manage.step_fleet_tab(-1, cx)))
            .on_action(cx.listener(|manage, _: &FleetNextTab, _, cx| manage.step_fleet_tab(1, cx)))
            .children(Kind::ALL.map(|kind| {
                let selected = kind == fleet.tab;
                div()
                    .id(SharedString::from(format!("fleet-tab-{}", kind.noun())))
                    .test_support()
                    .role(Role::Tab)
                    .aria_label(kind.title())
                    .aria_selected(selected)
                    .px_3()
                    .py_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(if tabs_focused { theme.primary } else { theme.secondary })
                            .text_color(if tabs_focused {
                                theme.primary_foreground
                            } else {
                                theme.foreground
                            })
                            .font_semibold()
                    })
                    .when(!selected, |this| {
                        this.text_color(theme.muted_foreground)
                            .hover(|style| style.bg(theme.secondary_hover))
                    })
                    .on_click(cx.listener(move |manage, _, _, cx| manage.show_fleet_tab(kind, cx)))
                    .child(format!("{} ({})", kind.title(), fleet.rows[slot(kind)].len()))
            }));
        let mut body = v_flex().id("fleet").test_support().gap_2().child(tabs);
        if let Err(reason) = &writable {
            body = body.child(read_only_note("fleet-read-only", reason, cx));
        }
        let columns = columns(fleet.tab);
        body = body.child(
            h_flex()
                .gap_2()
                .px_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .children(columns.iter().map(|(title, width)| column(*width).child(*title)))
                .when(fleet.tab == Kind::Agent, |this| {
                    this.child(div().w(rems(5.5)).flex_none())
                })
                .child(div().w(rems(4.)).flex_none().child("Source")),
        );
        let focused = fleet.list.is_focused(window);
        let rows = fleet.rows[ix]
            .iter()
            .enumerate()
            .map(|(row_ix, row)| {
                let selected = row_ix == fleet.selected[ix];
                let mut line = div()
                    .id(SharedString::from(format!("fleet-row-{row_ix}")))
                    .test_support()
                    .role(Role::ListBoxOption)
                    .aria_selected(selected)
                    .aria_label(row_label(row))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(if focused { theme.accent } else { theme.list_active })
                    })
                    .when(!selected, |this| this.hover(|style| style.bg(theme.list_hover)))
                    .on_click(
                        cx.listener(move |manage, _, window, cx| manage.open_fleet_editor(Some(row_ix), window, cx)),
                    );
                for (field, (_, width)) in cells(row.kind).iter().zip(columns.iter()) {
                    let text = cell(row, field);
                    line = line.child(column(*width).truncate().child(text));
                }
                if row.kind == Kind::Agent
                    && let Some(toggle) = fleet.toggles.get(row_ix)
                {
                    let enabled = row.value("enabled") == "true";
                    // The toggle sits in the row: a click on it, even disabled, never also opens the row's editor.
                    line = line.child(
                        div()
                            .id(SharedString::from(format!("fleet-toggle-cell-{row_ix}")))
                            .flex_none()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(button(
                                SharedString::from(format!("fleet-toggle-{row_ix}")),
                                if enabled { "Disable" } else { "Enable" },
                                &toggle.focus,
                                false,
                                writable.is_err(),
                                fleet.toggling.contains(&row.key),
                                move |manage, window, cx| manage.toggle_agent(row_ix, window, cx),
                                cx,
                            )),
                    );
                }
                line.child(
                    div()
                        .w(rems(4.))
                        .flex_none()
                        .text_xs()
                        .text_color(if row.from_config() {
                            theme.warning
                        } else {
                            theme.muted_foreground
                        })
                        .child(row.source.clone()),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = if fleet.loading[ix] && fleet.rows[ix].is_empty() {
            Some("Loading…".to_owned())
        } else if fleet.rows[ix].is_empty() && fleet.load_error[ix].is_none() {
            Some(format!("No {}s yet.", fleet.tab.noun()))
        } else {
            None
        };
        body = body.child(
            v_flex()
                .id("fleet-list")
                .test_support()
                .role(Role::ListBox)
                .aria_label(fleet.tab.title())
                .key_context(WORK_LIST)
                .track_focus(&fleet.list)
                .tab_index(0)
                .h(rems(16.))
                .overflow_y_scroll()
                .track_scroll(&fleet.scroll)
                .p_1()
                .rounded(theme.radius)
                .border_2()
                .border_color(theme.border)
                .focus(move |style| style.border_color(ring))
                .on_action(cx.listener(|manage, _: &WorkUp, _, cx| manage.move_in_fleet_list(-1, cx)))
                .on_action(cx.listener(|manage, _: &WorkDown, _, cx| manage.move_in_fleet_list(1, cx)))
                .on_action(cx.listener(|manage, _: &OpenItem, window, cx| manage.fleet_default_action(window, cx)))
                .children(rows)
                .when_some(empty, |this, text| {
                    this.child(
                        div()
                            .id("fleet-list-empty")
                            .test_support()
                            .aria_label(text.clone())
                            .p_2()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(text),
                    )
                }),
        );
        body = body.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(match fleet.tab {
                    Kind::Machine => "No delete: a machine with 0 max sessions starts none there.",
                    Kind::Subscription => "No delete: a subscription no agent uses pays for nothing.",
                    Kind::Agent => "No delete: disable an agent to stop it taking work.",
                }),
        );
        let mut footer = Vec::new();
        if fleet.load_error[ix].is_some() && !fleet.loading[ix] {
            let kind = fleet.tab;
            footer.push(button(
                "fleet-retry",
                "Retry",
                &fleet.retry.focus,
                false,
                false,
                false,
                move |manage, _, cx| manage.reload_fleet(&[kind], cx),
                cx,
            ));
        }
        footer.push(close);
        if writable.is_ok() {
            footer.push(button(
                "fleet-add",
                format!("Add {}", fleet.tab.noun()),
                &fleet.add.focus,
                true,
                false,
                false,
                |manage, window, cx| manage.open_fleet_editor(None, window, cx),
                cx,
            ));
        }
        let message = fleet
            .message
            .clone()
            .or_else(|| fleet.load_error[ix].clone().map(|error| (error, true)));
        self.render_frame(
            dialog,
            Frame {
                title: "Fleet".into(),
                body: body.into_any_element(),
                message,
                footer,
                wide: true,
            },
            window,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_fleet_editor(
        &self,
        dialog: &Dialog,
        fleet: &FleetDialog,
        editor: &FleetEditor,
        writable: Result<(), String>,
        close: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let kind = editor.kind;
        let form = &editor.form;
        let locked = editor.locked(writable.is_ok());
        let creating = editor.row.is_none();
        let title = match &editor.row {
            Some(row) => format!("{} {}", capitalized(kind.noun()), row.key),
            None => format!("New {}", kind.noun()),
        };
        let mut body = v_flex().id("fleet-editor").test_support().gap_3().child(
            div()
                .id("fleet-editor-title")
                .test_support()
                .aria_label(title.clone())
                .font_semibold()
                .child(title),
        );
        if let Err(reason) = &writable {
            body = body.child(read_only_note("fleet-read-only", reason, cx));
        }
        if let Some(row) = editor.row.as_ref().filter(|row| row.from_config()) {
            let text = format!(
                "pmd's config file owns this {}. Saving makes it API-owned, and the file stops updating it.",
                kind.noun()
            );
            let _ = row;
            body = body.child(
                h_flex()
                    .id("fleet-config-note")
                    .test_support()
                    .aria_label(text.clone())
                    .gap_2()
                    .text_sm()
                    .text_color(theme.warning)
                    .child(Icon::new(IconName::Info).xsmall())
                    .child(div().flex_1().child(text)),
            );
        }
        let known = fleet.known();
        for field in kind.fields().iter().copied() {
            let label = kind.label(field);
            let error = form.field_error(field);
            let control: AnyElement =
                if let Some((_, input)) = editor.inputs.iter().find(|(name, _)| *name == field) {
                    let mut column = v_flex().gap_1().child(
                        Input::new(input)
                            .id(SharedString::from(format!("fleet-field-{field}")))
                            .aria_label(label)
                            .disabled(locked),
                    );
                    let suggestions: &[&str] = match field {
                        "capabilities" => &CAPABILITIES,
                        "roles" => &ROLES,
                        _ => &[],
                    };
                    if let Some((_, controls)) = editor.choices.iter().find(|(name, _)| *name == field) {
                        column = column.child(h_flex().gap_1().flex_wrap().children(
                            suggestions.iter().zip(controls).map(|(word, control)| {
                                let word = *word;
                                let present = fleet::words(form.value(field)).iter().any(|w| w == word);
                                chip(
                                    format!("fleet-suggest-{field}-{word}"),
                                    format!("+ {word}"),
                                    &control.focus,
                                    present,
                                    locked || present,
                                    move |manage, window, cx| manage.set_fleet_field(field, word, window, cx),
                                    cx,
                                )
                            }),
                        ));
                    }
                    if field == "maxSessions" {
                        column = column.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("0 to 64. 0 starts no sessions there."),
                        );
                    }
                    column.into_any_element()
                } else if kind.key_fields().contains(&field) && !creating && field != "machine" {
                    read_only_text(form.value(field), cx)
                } else {
                    let options: Vec<(String, String)> = match field {
                        "os" => OSES.iter().map(|os| ((*os).to_owned(), (*os).to_owned())).collect(),
                        "metered" | "enabled" => vec![("true".into(), "Yes".into()), ("false".into(), "No".into())],
                        "machine" => known.machines.iter().map(|m| (m.clone(), m.clone())).collect(),
                        "subscription" => known.subscriptions.iter().map(|s| (s.clone(), s.clone())).collect(),
                        _ => Vec::new(),
                    };
                    if field == "machine" && !creating {
                        read_only_text(form.value(field), cx)
                    } else {
                        let controls = editor
                            .choices
                            .iter()
                            .find(|(name, _)| *name == field)
                            .map(|(_, controls)| controls.as_slice())
                            .unwrap_or_default();
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(options.into_iter().zip(controls).map(|((value, text), control)| {
                                let selected = form.value(field) == value;
                                chip(
                                    format!("fleet-{field}-{value}"),
                                    text,
                                    &control.focus,
                                    selected,
                                    locked,
                                    move |manage, window, cx| manage.set_fleet_field(field, &value, window, cx),
                                    cx,
                                )
                            }))
                            .into_any_element()
                    }
                };
            body = body.child(field_row(label, field, control, error, cx));
        }
        if kind == Kind::Agent && creating {
            let id = fleet::key_of(kind, form.values());
            body = body.child(
                div()
                    .id("fleet-agent-id")
                    .test_support()
                    .aria_label(format!("Id: {id}"))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("Id: {id} (fixed once created)")),
            );
        }
        let back = button(
            "fleet-back",
            "Back to list",
            &editor.back.focus,
            false,
            false,
            false,
            |manage, window, cx| manage.fleet_back_to_list(window, cx),
            cx,
        );
        let footer = if let Some(conflict) = form.conflict_state() {
            let labels: Vec<(&str, &str)> = kind.fields().iter().map(|f| (*f, kind.label(f))).collect();
            body = body.child(conflict_panel(conflict, &labels, cx));
            vec![
                back,
                button(
                    "conflict-discard",
                    "Discard mine",
                    &editor.discard.focus,
                    false,
                    false,
                    false,
                    |manage, window, cx| manage.discard_fleet_edits(window, cx),
                    cx,
                ),
                button(
                    "conflict-reapply",
                    "Reapply mine",
                    &editor.reapply.focus,
                    true,
                    false,
                    false,
                    |manage, window, cx| manage.reapply_fleet_row(window, cx),
                    cx,
                ),
            ]
        } else if writable.is_ok() {
            vec![
                back,
                close,
                button(
                    "fleet-save",
                    if form.is_saving() {
                        "Saving…"
                    } else if creating {
                        "Create (Ctrl+Enter)"
                    } else {
                        "Save (Ctrl+Enter)"
                    },
                    &editor.save.focus,
                    true,
                    !form.is_dirty(),
                    form.is_saving(),
                    |manage, window, cx| manage.save_fleet_row(window, cx),
                    cx,
                ),
            ]
        } else {
            vec![back, close]
        };
        let message = form.message().map(|text| {
            (
                text.to_owned(),
                form.conflict_state().is_none() && !text.starts_with("Changed elsewhere."),
            )
        });
        self.render_frame(
            dialog,
            Frame {
                title: "Fleet".into(),
                body: body.into_any_element(),
                message,
                footer,
                wide: true,
            },
            window,
            cx,
        )
    }
}

/// A list pmd returned, reconciled with what writes have shown since it was asked for: a row the dialog holds at a
/// newer version stays, and a row the list doesn't have yet (created meanwhile) is kept. pmd has no delete, so a row
/// missing from a list is one it hadn't stored when it answered.
fn merge(loaded: Vec<FleetRow>, current: &[FleetRow]) -> Vec<FleetRow> {
    let mut rows: Vec<FleetRow> = loaded
        .into_iter()
        .map(|row| match current.iter().find(|known| known.key == row.key) {
            Some(known) if known.version > row.version => known.clone(),
            _ => row,
        })
        .collect();
    for known in current {
        if !rows.iter().any(|row| row.key == known.key) {
            rows.push(known.clone());
        }
    }
    rows
}

/// Puts a row pmd returned in its list, replacing the old one or adding a new one, and keeps the controls in step. A
/// row the list holds at a newer version stays: an answer that arrives late never undoes a later write.
fn upsert(fleet: &mut FleetDialog, row: FleetRow, cx: &mut Context<Manage>) {
    let rows = &mut fleet.rows[slot(row.kind)];
    match rows.iter_mut().find(|known| known.key == row.key) {
        Some(known) if known.version > row.version => {}
        Some(known) => *known = row,
        None => rows.push(row),
    }
    sync_controls(fleet, cx);
}

/// One quick toggle per agent, and an agent editor's pickers one control per machine and subscription, as the lists
/// grow or shrink. Existing controls stay, so focus does too.
fn sync_controls(fleet: &mut FleetDialog, cx: &mut Context<Manage>) {
    let resize = |controls: &mut Vec<Control>, count: usize, cx: &mut Context<Manage>| {
        controls.truncate(count);
        while controls.len() < count {
            controls.push(control(cx));
        }
    };
    let agents = fleet.rows[2].len();
    resize(&mut fleet.toggles, agents, cx);
    let (machines, subscriptions) = (fleet.rows[0].len(), fleet.rows[1].len());
    if let Some(editor) = fleet.editor.as_mut().filter(|editor| editor.kind == Kind::Agent) {
        for (field, controls) in &mut editor.choices {
            match *field {
                "machine" => resize(controls, machines, cx),
                "subscription" => resize(controls, subscriptions, cx),
                _ => {}
            }
        }
    }
}

/// A key that can't change after creation, shown as text.
fn read_only_text(text: &str, cx: &Context<Manage>) -> AnyElement {
    div()
        .py_1()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_owned())
        .into_any_element()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// A tab's columns and their widths in rems; `None` takes the rest.
fn columns(kind: Kind) -> Vec<(&'static str, Option<f32>)> {
    match kind {
        Kind::Machine => vec![
            ("Name", Some(8.)),
            ("OS", Some(6.)),
            ("Tailnet IP", Some(9.)),
            ("Max sessions", Some(7.)),
            ("Capabilities", None),
        ],
        Kind::Subscription => vec![
            ("Id", Some(10.)),
            ("Provider", Some(8.)),
            ("Plan", None),
            ("Metered", Some(5.)),
        ],
        Kind::Agent => vec![
            ("Id", None),
            ("Model", Some(9.)),
            ("Subscription", Some(7.5)),
            ("Roles", Some(8.)),
            ("Enabled", Some(4.)),
        ],
    }
}

/// The field each column shows.
fn cells(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Machine => &["name", "os", "tailnetIp", "maxSessions", "capabilities"],
        Kind::Subscription => &["id", "provider", "plan", "metered"],
        Kind::Agent => &["id", "model", "subscription", "roles", "enabled"],
    }
}

fn cell(row: &FleetRow, field: &str) -> String {
    let text = match field {
        "id" if row.kind == Kind::Agent => row.key.clone(),
        "metered" | "enabled" => (if row.value(field) == "true" { "Yes" } else { "No" }).to_owned(),
        _ => row.value(field).to_owned(),
    };
    if text.is_empty() { "—".to_owned() } else { text }
}

fn column(width: Option<f32>) -> gpui_kit::Div {
    match width {
        Some(width) => div().w(rems(width)).flex_none(),
        None => div().flex_1().min_w_0(),
    }
}

fn row_label(row: &FleetRow) -> String {
    let cells: Vec<String> = cells(row.kind).iter().map(|field| cell(row, field)).collect();
    format!("{}, source {}", cells.join(", "), row.source)
}
