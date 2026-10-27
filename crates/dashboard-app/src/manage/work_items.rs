//! The Work items dialog (#35): pmd's open work, filtered and paged, and an editor that overrides one item's status,
//! reason, priority and rank with `PATCH /v1/work/{id}`.
//!
//! The list reads `GET /v1/work` with the read token, so it opens without write access; the editor is then read-only
//! and says which scope is missing. Up and Down move through the list and Enter opens the editor; in the editor,
//! Ctrl+Enter saves. A save sends only the changed fields with `If-Match`. `409 stale_version` shows the user's values
//! beside the new ones ("Changed elsewhere"), and `409 item_closed` re-reads the item and leaves nothing to edit. A
//! rank or priority change reloads the list from its first page, since pmd warns that paging in progress can skip or
//! repeat an item. The data and the patch live in `dashboard_core::work`.
use std::sync::Arc;

use dashboard_core::edit::{Form, Next};
use dashboard_core::work::{
    self, AUTO, FIELDS, Kind, MAX_REASON, STATUSES, WorkFilter, WorkItem, WorkPage, status_label,
};
use dashboard_core::write::{Feature, Method, WriteError, WriteRequest, write_state};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Toggled, Window, div, px, rems,
};

use super::{
    Activate, CONTROL, Content, Control, Dialog, Frame, Manage, ManageEvent, OpenItem, WORK_LIST, WorkDown, WorkUp,
    button, conflict_panel, control,
};

/// The editor's fields with their labels, for errors and the "Changed elsewhere" panel.
const LABELS: [(&str, &str); 4] = [
    ("status", "Status"),
    ("statusReason", "Reason"),
    ("priority", "Priority"),
    ("rank", "Rank"),
];
/// The status filter: every open status, or one.
const STATUS_FILTERS: [Option<&str>; 5] = [
    None,
    Some("ready"),
    Some("in_progress"),
    Some("waiting_on_franz"),
    Some("blocked"),
];
const KIND_FILTERS: [Option<Kind>; 3] = [None, Some(Kind::Issue), Some(Kind::Pr)];
const PRIORITY_FILTERS: [Option<u8>; 7] = [None, Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)];
/// The editor's status choices: the manual statuses, then back to the rules.
const STATUS_CHOICES: [&str; 5] = [STATUSES[0], STATUSES[1], STATUSES[2], STATUSES[3], AUTO];
/// The editor's priority choices: P0 to P5, then the project's (empty).
const PRIORITY_CHOICES: [&str; 7] = ["0", "1", "2", "3", "4", "5", ""];

pub struct WorkItemsDialog {
    project: Entity<InputState>,
    filter: WorkFilter,
    /// The filters the rows on screen were listed with.
    rows_filter: WorkFilter,
    status_filters: Vec<Control>,
    kind_filters: Vec<Control>,
    priority_filters: Vec<Control>,
    list: FocusHandle,
    scroll: ScrollHandle,
    items: Vec<WorkItem>,
    next_cursor: Option<String>,
    loading: bool,
    load_error: Option<String>,
    /// Bumped by every load from the first page, so an older page landing late can't replace a newer list.
    generation: u64,
    selected: usize,
    load_more: Control,
    retry: Control,
    editor: Option<Editor>,
    editors_opened: u64,
}

pub(super) struct Editor {
    /// Unique per opening, so input changes and saves for an editor that has since closed are dropped.
    key: u64,
    item: WorkItem,
    form: Form,
    /// The item closed on GitHub: its status is done and nothing is editable.
    closed: bool,
    /// A save failure that is the dashboard's bug, not the user's, shown instead of pmd's message.
    bug: Option<String>,
    reason: Entity<InputState>,
    rank: Entity<InputState>,
    statuses: Vec<Control>,
    priorities: Vec<Control>,
    back: Control,
    save: Control,
    reapply: Control,
    discard: Control,
}

impl WorkItemsDialog {
    pub fn is_dirty(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| !editor.closed && editor.form.is_dirty())
    }

    #[cfg(test)]
    pub fn items(&self) -> &[WorkItem] {
        &self.items
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    #[cfg(test)]
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }

    #[cfg(test)]
    pub fn editor_form(&self) -> Option<&Form> {
        self.editor.as_ref().map(|editor| &editor.form)
    }

    #[cfg(test)]
    pub fn editor_item(&self) -> Option<&WorkItem> {
        self.editor.as_ref().map(|editor| &editor.item)
    }

    #[cfg(test)]
    pub fn is_closed_item(&self) -> bool {
        self.editor.as_ref().is_some_and(|editor| editor.closed)
    }

    pub(super) fn focus_list(&self, window: &mut Window, cx: &mut Context<Manage>) {
        self.list.focus(window, cx);
    }

    /// The input for `project`, `statusReason` or `rank`, for tests and captures.
    #[cfg(test)]
    pub(super) fn input(&self, field: &str) -> Option<Entity<InputState>> {
        match field {
            "project" => Some(self.project.clone()),
            "statusReason" => self.editor.as_ref().map(|editor| editor.reason.clone()),
            "rank" => self.editor.as_ref().map(|editor| editor.rank.clone()),
            _ => None,
        }
    }
}

impl Editor {
    /// Puts the form's values into the inputs, after a save, a discard or a re-read.
    fn sync_inputs(&self, window: &mut Window, cx: &mut Context<Manage>) {
        for (field, input) in [("statusReason", &self.reason), ("rank", &self.rank)] {
            let text = self.form.value(field).to_owned();
            if input.read(cx).value() != text.as_str() {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    /// Inputs take no edits while a save runs, a conflict waits, the item is closed or the dialog is read-only; the
    /// reason also not under an automatic status.
    fn sync_disabled(&self, writable: bool, cx: &mut Context<Manage>) {
        let locked = !writable || self.closed || self.form.is_saving() || self.form.conflict_state().is_some();
        let auto = self.form.value("status") == AUTO;
        self.reason
            .update(cx, |input, cx| input.set_disabled(locked || auto, cx));
        self.rank.update(cx, |input, cx| input.set_disabled(locked, cx));
    }
}

impl Manage {
    fn work_writable(&self) -> bool {
        write_state(Feature::WorkItems, &self.access).is_ok()
    }

    /// The open Work items dialog, while it is still dialog `id`.
    fn work_of(&mut self, id: u64) -> Option<&mut WorkItemsDialog> {
        match self.content_of(id) {
            Some(Content::WorkItems(dialog)) => Some(dialog),
            _ => None,
        }
    }

    /// The open Work items dialog and its id.
    fn open_work(&mut self) -> Option<(u64, &mut WorkItemsDialog)> {
        let dialog = self.dialog.as_mut()?;
        match &mut dialog.content {
            Content::WorkItems(work) => Some((dialog.id, work)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn work_items(&self) -> Option<&WorkItemsDialog> {
        match &self.dialog.as_ref()?.content {
            Content::WorkItems(work) => Some(work),
            _ => None,
        }
    }

    pub(super) fn new_work_items(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> WorkItemsDialog {
        let project = cx.new(|cx| InputState::new(window, cx).placeholder("Project slugs, such as gh-x, chief"));
        cx.observe(&project, move |manage, input, cx| {
            let text = input.read(cx).value().to_string();
            let Some(work) = manage.work_of(id) else {
                return;
            };
            if work.filter.project != text {
                work.filter.project = text;
                manage.load_work(id, true, cx);
            }
        })
        .detach();
        let controls = |count: usize, cx: &mut Context<Self>| (0..count).map(|_| control(cx)).collect::<Vec<_>>();
        let dialog = WorkItemsDialog {
            project,
            filter: WorkFilter::default(),
            rows_filter: WorkFilter::default(),
            status_filters: controls(STATUS_FILTERS.len(), cx),
            kind_filters: controls(KIND_FILTERS.len(), cx),
            priority_filters: controls(PRIORITY_FILTERS.len(), cx),
            list: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            items: Vec::new(),
            next_cursor: None,
            loading: true,
            load_error: None,
            generation: 0,
            selected: 0,
            load_more: control(cx),
            retry: control(cx),
            editor: None,
            editors_opened: 0,
        };
        if matches!(
            self.access,
            dashboard_core::write::Access::Unchecked | dashboard_core::write::Access::Failed(_)
        ) {
            self.check_access(cx);
        }
        dialog
    }

    /// Loads a page of `GET /v1/work`: the first page when `reset`, else the next one.
    pub(super) fn load_work(&mut self, id: u64, reset: bool, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let Some(work) = self.work_of(id) else {
            return;
        };
        let cursor = if reset {
            work.generation += 1;
            // The old listing's cursor means nothing for the new query.
            work.next_cursor = None;
            None
        } else {
            match &work.next_cursor {
                Some(cursor) if !work.loading => Some(cursor.clone()),
                _ => return,
            }
        };
        let generation = work.generation;
        let filter = work.filter.clone();
        let query = filter.query(cursor.as_deref());
        work.loading = true;
        work.load_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.list("v1/work", &query) })
                .await
                .and_then(|value| WorkPage::from_json(&value));
            let _ = this.update(cx, |manage, cx| {
                let Some(work) = manage.work_of(id) else {
                    return;
                };
                if work.generation != generation {
                    return;
                }
                work.loading = false;
                match result {
                    Ok(page) => {
                        if cursor.is_none() {
                            work.rows_filter = filter;
                            work.items = page.items;
                            work.selected = work.selected.min(work.items.len().saturating_sub(1));
                        } else {
                            work.items.extend(page.items);
                        }
                        work.next_cursor = page.next_cursor;
                    }
                    Err(error) => {
                        if cursor.is_none() && work.rows_filter != filter {
                            // The rows on screen belong to other filters: drop them rather than mix listings. Under
                            // the same filters they are the last good data and stay, beside the error.
                            work.items.clear();
                            work.selected = 0;
                        }
                        work.load_error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_work_filter(&mut self, change: impl FnOnce(&mut WorkFilter), cx: &mut Context<Self>) {
        let Some((id, work)) = self.open_work() else {
            return;
        };
        let before = work.filter.clone();
        change(&mut work.filter);
        if work.filter != before {
            work.selected = 0;
            self.load_work(id, true, cx);
        }
    }

    pub(super) fn move_in_work_list(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((_, work)) = self.open_work() else {
            return;
        };
        if work.items.is_empty() {
            return;
        }
        let last = work.items.len() as isize - 1;
        work.selected = (work.selected as isize + delta).clamp(0, last) as usize;
        work.scroll.scroll_to_item(work.selected);
        cx.notify();
    }

    /// Opens the editor on list row `ix`.
    pub fn open_work_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.work_writable();
        let Some((id, work)) = self.open_work() else {
            return;
        };
        let Some(item) = work.items.get(ix).cloned() else {
            return;
        };
        work.selected = ix;
        work.editors_opened += 1;
        let key = work.editors_opened;
        let form = Form::edit(item.row(), &FIELDS);
        let reason = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Why: shown on the Board")
                .default_value(form.value("statusReason").to_owned())
        });
        let rank = cx.new(|cx| InputState::new(window, cx).default_value(form.value("rank").to_owned()));
        for (field, input) in [("statusReason", &reason), ("rank", &rank)] {
            cx.observe(input, move |manage, input, cx| {
                let text = input.read(cx).value().to_string();
                let Some(editor) = manage.work_of(id).and_then(|work| work.editor.as_mut()) else {
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
        let editor = Editor {
            key,
            closed: item.is_closed(),
            item,
            form,
            bug: None,
            reason,
            rank,
            statuses: controls(STATUS_CHOICES.len(), cx),
            priorities: controls(PRIORITY_CHOICES.len(), cx),
            back: control(cx),
            save: control(cx),
            reapply: control(cx),
            discard: control(cx),
        };
        editor.sync_disabled(writable, cx);
        // Focus starts on the current status, or on Back when there is nothing to edit.
        let current = STATUS_CHOICES
            .iter()
            .position(|status| *status == editor.form.value("status"))
            .unwrap_or(0);
        if writable && !editor.closed {
            editor.statuses[current].focus.focus(window, cx);
        } else {
            editor.back.focus.focus(window, cx);
        }
        work.editor = Some(editor);
        cx.notify();
    }

    /// Back to the list. Unsaved edits ask first, as closing does.
    pub fn work_back_to_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.open_work().is_some_and(|(_, work)| work.is_dirty());
        if dirty {
            self.ask_before_discarding(true, window, cx);
            return;
        }
        self.leave_work_editor(window, cx);
    }

    pub fn leave_work_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, work)) = self.open_work() {
            work.editor = None;
            work.list.focus(window, cx);
            work.scroll.scroll_to_item(work.selected);
        }
        cx.notify();
    }

    /// Picks a status or a priority in the editor.
    fn set_work_field(&mut self, field: &'static str, value: &str, cx: &mut Context<Self>) {
        let writable = self.work_writable();
        let Some(editor) = self.open_work().and_then(|(_, work)| work.editor.as_mut()) else {
            return;
        };
        if !writable || editor.closed {
            return;
        }
        editor.form.set(field, value);
        if field == "status" && value == AUTO {
            // An automatic status sends no reason, so a problem with the reason no longer stops the save.
            editor.form.clear_error("statusReason");
        }
        editor.bug = None;
        editor.sync_disabled(writable, cx);
        cx.notify();
    }

    /// Ctrl+Enter or Save: checks the fields, then one `PATCH` of the changed ones with `If-Match`.
    pub fn save_work_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.work_writable();
        let backend = Arc::clone(&self.backend);
        let Some((id, work)) = self.open_work() else {
            return;
        };
        let status_filter = work.filter.status.is_some();
        let Some(editor) = work.editor.as_mut() else {
            return;
        };
        if !writable || editor.closed || editor.form.is_saving() || editor.form.conflict_state().is_some() {
            return;
        }
        let body = match work::patch(&editor.form) {
            Ok(Some(body)) => body,
            Ok(None) => return,
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
        editor.bug = None;
        editor.sync_disabled(writable, cx);
        let key = editor.key;
        let path = editor.item.path();
        // A rank or priority change reorders the list; a status change under a status filter can take the row out
        // of it. Either way the list reloads from its first page.
        let reorders = work::reorders(&body) || (status_filter && body.get("status").is_some());
        let request = WriteRequest {
            method: Method::Patch,
            path: path.clone(),
            precondition,
            body,
        };
        cx.spawn_in(window, async move |this, cx| {
            let reader = Arc::clone(&backend);
            let result = cx
                .background_executor()
                .spawn(async move { backend.write(&request) })
                .await
                .and_then(|value| WorkItem::from_json(&value));
            let next = this.update_in(cx, |manage, window, cx| {
                let writable = manage.work_writable();
                if let Ok(item) = &result {
                    // pmd took the write: the Board hears of it and the list shows it, even when the editor or the
                    // dialog closed while it was on its way.
                    cx.emit(ManageEvent::Wrote);
                    if let Some(work) = manage.work_of(id) {
                        if let Some(row) = work.items.iter_mut().find(|row| row.id == item.id) {
                            *row = item.clone();
                        }
                        if reorders {
                            manage.load_work(id, true, cx);
                        }
                    }
                }
                let work = manage.work_of(id)?;
                let editor = work.editor.as_mut().filter(|editor| editor.key == key)?;
                let next = match result {
                    Ok(item) => {
                        editor.form.saved(item.row());
                        editor.item = item;
                        editor.sync_inputs(window, cx);
                        None
                    }
                    Err(error) => {
                        let next = editor.form.failed(&error);
                        match error {
                            WriteError::PreconditionRequired => {
                                editor.bug = Some(
                                    "A dashboard bug: pmd says the save carried no version (HTTP 428). Nothing was \
                                     saved, and it won't be retried."
                                        .to_owned(),
                                );
                                None
                            }
                            WriteError::ItemClosed => Some(Next::Show),
                            _ if next == Next::Reread => Some(Next::Reread),
                            _ => None,
                        }
                    }
                };
                if let Some(editor) = manage
                    .work_of(id)
                    .and_then(|work| work.editor.as_mut())
                    .filter(|editor| editor.key == key)
                {
                    editor.sync_disabled(writable, cx);
                }
                cx.notify();
                next
            });
            let Ok(Some(next)) = next else {
                return;
            };
            // Changed elsewhere, or closed on GitHub: read the item again with the read token.
            let fresh = cx
                .background_executor()
                .spawn(async move { reader.read(&path) })
                .await
                .and_then(|value| WorkItem::from_json(&value));
            let _ = this.update_in(cx, |manage, window, cx| {
                let writable = manage.work_writable();
                let Some(work) = manage.work_of(id) else {
                    return;
                };
                // pmd's current row replaces the list's; the list then reloads from its first page, since the
                // change made elsewhere may move the item or take it out of the filters.
                let reconcile = fresh.is_ok();
                if let Ok(item) = &fresh
                    && let Some(row) = work.items.iter_mut().find(|row| row.id == item.id)
                {
                    *row = item.clone();
                }
                if let Some(editor) = work.editor.as_mut().filter(|editor| editor.key == key) {
                    show_reread(editor, next, fresh, writable, window, cx);
                }
                if reconcile {
                    manage.load_work(id, true, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// Shows a re-read in the editor: beside the user's values after `409 stale_version`, or as a closed item.
fn show_reread(
    editor: &mut Editor,
    next: Next,
    fresh: Result<WorkItem, WriteError>,
    writable: bool,
    window: &mut Window,
    cx: &mut Context<Manage>,
) {
    match (next, fresh) {
        (Next::Reread, Ok(item)) => {
            editor.form.conflict(item.row());
            editor.item = item;
            editor.reapply.focus.focus(window, cx);
        }
        (Next::Reread, Err(error)) => editor.form.reread_failed(&error),
        (_, Ok(item)) => {
            // Closed: show it as pmd has it now, with nothing left to edit.
            editor.form = Form::edit(item.row(), &FIELDS);
            editor.closed = true;
            editor.item = item;
            editor.sync_inputs(window, cx);
            editor.back.focus.focus(window, cx);
        }
        (_, Err(_)) => {
            editor.closed = true;
            editor.back.focus.focus(window, cx);
        }
    }
    editor.sync_disabled(writable, cx);
}

impl Manage {
    pub fn reapply_work_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.open_work().and_then(|(_, work)| work.editor.as_mut()) {
            editor.form.reapply();
        }
        self.save_work_item(window, cx);
    }

    pub fn discard_work_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.work_writable();
        if let Some(editor) = self.open_work().and_then(|(_, work)| work.editor.as_mut()) {
            editor.form.discard();
            editor.sync_inputs(window, cx);
            editor.sync_disabled(writable, cx);
            let current = STATUS_CHOICES
                .iter()
                .position(|status| *status == editor.form.value("status"))
                .unwrap_or(0);
            editor.statuses[current].focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Enter on the dialog outside the list and the controls: nothing in the editor, where Ctrl+Enter saves.
    pub(super) fn work_default_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, work)) = self.open_work() else {
            return;
        };
        if work.editor.is_none() {
            let selected = work.selected;
            self.open_work_item(selected, window, cx);
        }
    }

    pub(super) fn render_work_items(
        &self,
        dialog: &Dialog,
        work: &WorkItemsDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let writable = write_state(Feature::WorkItems, &self.access);
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
        if let Some(editor) = &work.editor {
            return self.render_work_editor(dialog, editor, writable, close, window, cx);
        }
        let theme = cx.theme().clone();
        let mut body = v_flex().id("work-items").test_support().gap_2();
        body = body.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().w(rems(6.)).text_sm().child("Project"))
                .child(
                    div().flex_1().child(
                        Input::new(&work.project)
                            .id("work-filter-project")
                            .aria_label("Project filter"),
                    ),
                ),
        );
        let status_chips = STATUS_FILTERS
            .iter()
            .zip(&work.status_filters)
            .map(|(status, control)| {
                let status = *status;
                chip(
                    format!("work-filter-status-{}", status.unwrap_or("open")),
                    status.map_or("All open", status_label),
                    &control.focus,
                    work.filter.status == status,
                    false,
                    move |manage, _, cx| manage.set_work_filter(|filter| filter.status = status, cx),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        body = body.child(filter_row("Status", status_chips, cx));
        let kind_chips = KIND_FILTERS
            .iter()
            .zip(&work.kind_filters)
            .map(|(kind, control)| {
                let kind = *kind;
                let (id, label) = match kind {
                    None => ("all", "All"),
                    Some(Kind::Issue) => ("issue", "Issues"),
                    Some(Kind::Pr) => ("pr", "PRs"),
                };
                chip(
                    format!("work-filter-kind-{id}"),
                    label,
                    &control.focus,
                    work.filter.kind == kind,
                    false,
                    move |manage, _, cx| manage.set_work_filter(|filter| filter.kind = kind, cx),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let priority_chips = PRIORITY_FILTERS
            .iter()
            .zip(&work.priority_filters)
            .map(|(priority, control)| {
                let priority = *priority;
                chip(
                    format!(
                        "work-filter-priority-{}",
                        priority.map_or("any".to_owned(), |p| p.to_string())
                    ),
                    priority.map_or("Any".to_owned(), |p| format!("P{p}")),
                    &control.focus,
                    work.filter.max_priority == priority,
                    false,
                    move |manage, _, cx| manage.set_work_filter(|filter| filter.max_priority = priority, cx),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        body = body.child(
            h_flex()
                .gap_4()
                .flex_wrap()
                .child(filter_row("Kind", kind_chips, cx))
                .child(filter_row("Up to", priority_chips, cx)),
        );
        if let Err(reason) = &writable {
            body = body.child(read_only_note(reason, cx));
        }
        let rows = work.items.iter().enumerate().map(|(ix, item)| {
            let selected = ix == work.selected;
            let focused = work.list.is_focused(window);
            div()
                .id(SharedString::from(format!("work-row-{ix}")))
                .test_support()
                .role(Role::ListBoxOption)
                .aria_selected(selected)
                .aria_label(row_label(item))
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
                .on_click(cx.listener(move |manage, _, window, cx| manage.open_work_item(ix, window, cx)))
                .child(div().w(rems(8.)).flex_none().truncate().child(item.project.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("#{} {}", item.number, item.title)),
                )
                .child(
                    div()
                        .w(rems(3.))
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(if item.kind == "pr" { "PR" } else { "Issue" }),
                )
                .child(div().w(rems(12.)).flex_none().child(status_text(item)))
                .child(div().w(rems(6.)).flex_none().child(priority_text(item)))
                .child(
                    div()
                        .w(rems(4.))
                        .flex_none()
                        .text_right()
                        .text_color(theme.muted_foreground)
                        .child(item.rank.to_string()),
                )
                .into_any_element()
        });
        let empty = if work.loading && work.items.is_empty() {
            Some("Loading…".to_owned())
        } else if work.items.is_empty() && work.load_error.is_none() {
            Some("No open work matches these filters.".to_owned())
        } else {
            None
        };
        let focus_ring = theme.ring;
        body = body.child(
            h_flex()
                .gap_2()
                .px_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().w(rems(8.)).flex_none().child("Project"))
                .child(div().flex_1().child("Item"))
                .child(div().w(rems(3.)).flex_none().child("Kind"))
                .child(div().w(rems(12.)).flex_none().child("Status"))
                .child(div().w(rems(6.)).flex_none().child("Priority"))
                .child(div().w(rems(4.)).flex_none().text_right().child("Rank")),
        );
        body = body.child(
            v_flex()
                .id("work-list")
                .test_support()
                .role(Role::ListBox)
                .aria_label("Work items")
                .key_context(WORK_LIST)
                .track_focus(&work.list)
                .tab_index(0)
                .h(rems(17.))
                .overflow_y_scroll()
                .track_scroll(&work.scroll)
                .p_1()
                .rounded(theme.radius)
                .border_2()
                .border_color(theme.border)
                .focus(move |style| style.border_color(focus_ring))
                .on_action(cx.listener(|manage, _: &WorkUp, _, cx| manage.move_in_work_list(-1, cx)))
                .on_action(cx.listener(|manage, _: &WorkDown, _, cx| manage.move_in_work_list(1, cx)))
                .on_action(cx.listener(|manage, _: &OpenItem, window, cx| {
                    if let Some((_, work)) = manage.open_work() {
                        let selected = work.selected;
                        manage.open_work_item(selected, window, cx);
                    }
                }))
                .children(rows)
                .when_some(empty, |this, text| {
                    this.child(
                        div()
                            .id("work-list-empty")
                            .test_support()
                            .aria_label(text.clone())
                            .p_2()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(text),
                    )
                }),
        );
        let count = format!(
            "{} item{}{}",
            work.items.len(),
            if work.items.len() == 1 { "" } else { "s" },
            if work.next_cursor.is_some() {
                ", more to load"
            } else {
                ""
            }
        );
        let mut footer = vec![
            div()
                .id("work-count")
                .test_support()
                .aria_label(count.clone())
                .flex_1()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(count)
                .into_any_element(),
        ];
        if work.next_cursor.is_some() {
            footer.push(button(
                "work-load-more",
                if work.loading { "Loading…" } else { "Load more" },
                &work.load_more.focus,
                false,
                false,
                work.loading,
                |manage, _, cx| {
                    if let Some((id, _)) = manage.open_work() {
                        manage.load_work(id, false, cx);
                    }
                },
                cx,
            ));
        }
        if work.load_error.is_some() && !work.loading {
            footer.push(button(
                "work-retry",
                "Retry",
                &work.retry.focus,
                false,
                false,
                false,
                |manage, _, cx| {
                    if let Some((id, _)) = manage.open_work() {
                        manage.load_work(id, true, cx);
                    }
                },
                cx,
            ));
        }
        footer.push(close);
        self.render_frame(
            dialog,
            Frame {
                title: "Work items".into(),
                body: body.into_any_element(),
                message: work.load_error.clone().map(|error| (error, true)),
                footer,
                wide: true,
            },
            window,
            cx,
        )
    }

    fn render_work_editor(
        &self,
        dialog: &Dialog,
        editor: &Editor,
        writable: Result<(), String>,
        close: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let form = &editor.form;
        let item = &editor.item;
        let editable = writable.is_ok() && !editor.closed;
        let locked = !editable || form.is_saving() || form.conflict_state().is_some();
        let mut body = v_flex().id("work-editor").test_support().gap_3();
        body = body.child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .id("work-editor-title")
                        .test_support()
                        .aria_label(format!("{} #{} {}", item.project, item.number, item.title))
                        .font_semibold()
                        .child(format!("{} #{} {}", item.project, item.number, item.title)),
                )
                .child(
                    div()
                        .id("work-editor-now")
                        .test_support()
                        .aria_label(format!(
                            "Now: {}, {}, rank {}",
                            status_text(item),
                            priority_text(item),
                            item.rank
                        ))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} · now {} · {} · rank {}",
                            if item.kind == "pr" { "PR" } else { "Issue" },
                            status_text(item),
                            priority_text(item),
                            item.rank
                        )),
                ),
        );
        if let Err(reason) = &writable {
            body = body.child(read_only_note(reason, cx));
        }
        if editor.closed {
            let text = "This item closed on GitHub. Its status is Done and comes from GitHub now, so there is nothing \
                        to edit here.";
            body = body.child(
                h_flex()
                    .id("work-closed")
                    .test_support()
                    .aria_label(text)
                    .gap_2()
                    .p_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(Icon::new(IconName::CircleCheck).text_color(theme.muted_foreground))
                    .child(div().flex_1().child(text)),
            );
            let back = button(
                "work-back",
                "Back to list",
                &editor.back.focus,
                true,
                false,
                false,
                |manage, window, cx| manage.work_back_to_list(window, cx),
                cx,
            );
            return self.render_frame(
                dialog,
                Frame {
                    title: "Work items".into(),
                    body: body.into_any_element(),
                    message: None,
                    footer: vec![back, close],
                    wide: true,
                },
                window,
                cx,
            );
        }
        let status_chips = STATUS_CHOICES
            .iter()
            .zip(&editor.statuses)
            .map(|(status, control)| {
                let status = *status;
                chip(
                    format!("work-status-{status}"),
                    if status == AUTO {
                        "Back to automatic"
                    } else {
                        status_label(status)
                    },
                    &control.focus,
                    form.value("status") == status,
                    locked,
                    move |manage, _, cx| manage.set_work_field("status", status, cx),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        body = body.child(field_row(
            "Status",
            h_flex().gap_1().flex_wrap().children(status_chips),
            form.field_error("status"),
            cx,
        ));
        let reason_count = form.value("statusReason").chars().count();
        let over = reason_count > MAX_REASON;
        let auto = form.value("status") == AUTO;
        let reason = v_flex()
            .gap_1()
            .child(
                Input::new(&editor.reason)
                    .id("work-reason")
                    .aria_label("Reason")
                    .disabled(locked || auto),
            )
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().flex_1().text_color(theme.muted_foreground).child(if auto {
                        "An automatic status has no reason."
                    } else {
                        "Optional. Shown on the Board."
                    }))
                    .child(
                        div()
                            .id("work-reason-count")
                            .test_support()
                            .aria_label(format!("{reason_count} of {MAX_REASON} characters"))
                            .text_color(if over { theme.danger } else { theme.muted_foreground })
                            .child(format!("{reason_count}/{MAX_REASON}")),
                    ),
            )
            .when_some(work::reason_note(form), |this, note| {
                this.child(
                    h_flex()
                        .id("work-reason-note")
                        .test_support()
                        .aria_label(note.clone())
                        .gap_1()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(note),
                )
            });
        body = body.child(field_row("Reason", reason, form.field_error("statusReason"), cx));
        let priority_chips = PRIORITY_CHOICES
            .iter()
            .zip(&editor.priorities)
            .map(|(priority, control)| {
                let priority = *priority;
                chip(
                    format!(
                        "work-priority-{}",
                        if priority.is_empty() { "inherit" } else { priority }
                    ),
                    if priority.is_empty() {
                        "Inherit project priority".to_owned()
                    } else {
                        format!("P{priority}")
                    },
                    &control.focus,
                    form.value("priority") == priority,
                    locked,
                    move |manage, _, cx| manage.set_work_field("priority", priority, cx),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        body = body.child(field_row(
            "Priority",
            h_flex().gap_1().flex_wrap().children(priority_chips),
            form.field_error("priority"),
            cx,
        ));
        let rank = h_flex()
            .gap_2()
            .child(
                div().w(rems(10.)).child(
                    Input::new(&editor.rank)
                        .id("work-rank")
                        .aria_label("Rank")
                        .disabled(locked),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Lower sorts first within a priority."),
            );
        body = body.child(field_row("Rank", rank, form.field_error("rank"), cx));
        let back = button(
            "work-back",
            "Back to list",
            &editor.back.focus,
            false,
            false,
            false,
            |manage, window, cx| manage.work_back_to_list(window, cx),
            cx,
        );
        let footer = if let Some(conflict) = form.conflict_state() {
            body = body.child(conflict_panel(&readable(conflict), &LABELS, cx));
            vec![
                back,
                button(
                    "conflict-discard",
                    "Discard mine",
                    &editor.discard.focus,
                    false,
                    false,
                    false,
                    |manage, window, cx| manage.discard_work_edits(window, cx),
                    cx,
                ),
                button(
                    "conflict-reapply",
                    "Reapply mine",
                    &editor.reapply.focus,
                    true,
                    false,
                    false,
                    |manage, window, cx| manage.reapply_work_item(window, cx),
                    cx,
                ),
            ]
        } else if writable.is_ok() {
            vec![
                back,
                close,
                button(
                    "work-save",
                    if form.is_saving() {
                        "Saving…"
                    } else {
                        "Save (Ctrl+Enter)"
                    },
                    &editor.save.focus,
                    true,
                    !form.is_dirty(),
                    form.is_saving(),
                    |manage, window, cx| manage.save_work_item(window, cx),
                    cx,
                ),
            ]
        } else {
            vec![back, close]
        };
        let message = editor.bug.clone().map(|bug| (bug, true)).or_else(|| {
            form.message().map(|text| {
                (
                    text.to_owned(),
                    form.conflict_state().is_none() && !text.starts_with("Changed elsewhere."),
                )
            })
        });
        self.render_frame(
            dialog,
            Frame {
                title: "Work items".into(),
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

/// A field's text as the editor shows it: a status by its label, a priority as `P0`, an empty one as the project's.
fn shown(field: &str, value: &str) -> String {
    match (field, value) {
        ("status", status) => {
            if status == AUTO {
                "Automatic".to_owned()
            } else {
                status_label(status).to_owned()
            }
        }
        ("priority", "") => "Project's".to_owned(),
        ("priority", priority) => format!("P{priority}"),
        (_, value) => value.to_owned(),
    }
}

/// The "Changed elsewhere" values in the editor's own words.
fn readable(conflict: &dashboard_core::edit::Conflict) -> dashboard_core::edit::Conflict {
    let mut shown_conflict = conflict.clone();
    for (field, value) in shown_conflict.mine.iter_mut() {
        *value = shown(field, value);
    }
    for (field, value) in shown_conflict.theirs.values.iter_mut() {
        *value = shown(field, value);
    }
    shown_conflict
}

fn status_text(item: &WorkItem) -> String {
    format!(
        "{} · {}",
        status_label(&item.status),
        if item.is_manual() { "manual" } else { "auto" }
    )
}

fn priority_text(item: &WorkItem) -> String {
    let priority = item.priority.map_or("—".to_owned(), |p| format!("P{p}"));
    if item.priority_override.is_some() {
        format!("{priority} · own")
    } else {
        priority
    }
}

fn row_label(item: &WorkItem) -> String {
    format!(
        "{} #{} {}, {}, {}, {}, rank {}",
        item.project,
        item.number,
        item.title,
        if item.kind == "pr" { "PR" } else { "Issue" },
        status_text(item),
        priority_text(item),
        item.rank
    )
}

fn read_only_note(reason: &str, cx: &Context<Manage>) -> AnyElement {
    let theme = cx.theme();
    let text = format!("Read-only. {reason}.");
    h_flex()
        .id("work-read-only")
        .test_support()
        .aria_label(text.clone())
        .gap_2()
        .p_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.warning)
        .bg(theme.warning.opacity(0.08))
        .text_sm()
        .child(Icon::new(IconName::Info).text_color(theme.warning))
        .child(div().flex_1().child(text))
        .into_any_element()
}

fn filter_row(label: &str, chips: Vec<AnyElement>, cx: &Context<Manage>) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(rems(6.))
                .flex_none()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(h_flex().gap_1().flex_wrap().children(chips))
        .into_any_element()
}

fn field_row(label: &str, control: impl IntoElement, error: Option<&str>, cx: &Context<Manage>) -> AnyElement {
    let theme = cx.theme();
    let field = LABELS
        .iter()
        .find(|(_, name)| *name == label)
        .map_or(label, |(field, _)| field);
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(rems(6.))
                .flex_none()
                .pt(px(4.))
                .text_sm()
                .child(label.to_owned()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(control)
                .when_some(error.map(str::to_owned), |this, error| {
                    this.child(
                        h_flex()
                            .id(SharedString::from(format!("field-{field}-error")))
                            .test_support()
                            .aria_label(error.clone())
                            .gap_1()
                            .text_xs()
                            .text_color(theme.danger)
                            .child(Icon::new(IconName::CircleAlert).xsmall())
                            .child(error),
                    )
                }),
        )
        .into_any_element()
}

/// One choice in a group: a compact toggle that holds focus, with Enter or Space choosing it and a ring for focus.
fn chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    focus: &FocusHandle,
    selected: bool,
    disabled: bool,
    on_choose: impl Fn(&mut Manage, &mut Window, &mut Context<Manage>) + 'static,
    cx: &mut Context<Manage>,
) -> AnyElement {
    let theme = cx.theme();
    let label: SharedString = label.into();
    let (bg, fg) = if selected {
        (theme.primary, theme.primary_foreground)
    } else {
        (theme.secondary, theme.foreground)
    };
    let hover = if selected {
        theme.primary.opacity(0.85)
    } else {
        theme.secondary_hover
    };
    let ring = if selected { theme.foreground } else { theme.ring };
    let on_choose = std::rc::Rc::new(on_choose);
    let choose = std::rc::Rc::clone(&on_choose);
    div()
        .id(id.into())
        .test_support()
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_toggled(if selected { Toggled::True } else { Toggled::False })
        .key_context(CONTROL)
        .track_focus(focus)
        .tab_index(0)
        .px_2()
        .py(px(2.))
        .rounded(theme.radius)
        .border_2()
        .border_color(gpui_kit::transparent_black())
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .when(disabled, |this| this.opacity(0.6))
        .when(!disabled, |this| {
            this.cursor_pointer().hover(move |style| style.bg(hover))
        })
        .focus(move |style| style.border_color(ring))
        .child(label)
        .on_action(cx.listener(move |manage, _: &Activate, window, cx| {
            if !disabled {
                choose(manage, window, cx);
            }
        }))
        .on_click(cx.listener(move |manage, _, window, cx| {
            if !disabled {
                on_choose(manage, window, cx);
            }
        }))
        .into_any_element()
}
