//! The Manage menu and its modal dialogs (#33).
//!
//! The title bar's **Manage** button opens a menu with **Projects…**, **Work items…**, **Fleet…** and **Write access…**.
//! An entry that can't open yet stays visible, disabled, with a one-line reason under it. Each entry opens one modal
//! dialog in a shared frame: focus moves into it and stays there (Tab and Shift+Tab cycle inside), Esc or the close
//! button asks before discarding unsaved edits, Enter runs the default action, and closing returns focus to the
//! Manage button. Only one dialog is open at a time and the table behind it takes no input, but F5 still refreshes:
//! the dialog sits inside the dashboard's element tree, so the refresh action bubbles up to it as before.
//!
//! The dialogs' logic that needs no window lives in `dashboard_core::edit` (one row's edit, its errors and the
//! "Changed elsewhere" flow) and `dashboard_core::write` (the write token and client). Projects, Work items and Fleet
//! (#34, #35, #36) add their content to this frame; until then their entries say they aren't built yet. With
//! `--demo`, a **Sample dialog** entry shows the frame with a fake row, validation and a conflict.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use dashboard_core::edit::{Form, Next, Row};
use dashboard_core::write::{Access, Feature, Method, WriteBackend, WriteError, WriteRequest, entry_state};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme as _, FocusTrapElement as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, Pixels, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, actions, canvas, div, px, rems,
};

actions!(manage, [OpenMenu, Close, NextEntry, Previous, Activate, DefaultAction]);

const BUTTON: &str = "ManageButton";
const MENU: &str = "ManageMenu";
const DIALOG: &str = "ManageDialog";
const CONTROL: &str = "ManageControl";

/// The dialogs built so far. #34, #35 and #36 add theirs.
const BUILT: &[Feature] = &[];

/// The demo's sample row, at this API-shaped path.
pub const SAMPLE_PATH: &str = "v1/sample/demo";
/// The sample dialog's fields and labels.
const SAMPLE_FIELDS: [(&str, &str); 2] = [("title", "Title"), ("priority", "Priority (0–5, empty for none)")];

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", OpenMenu, Some(BUTTON)),
        KeyBinding::new("space", OpenMenu, Some(BUTTON)),
        KeyBinding::new("down", OpenMenu, Some(BUTTON)),
        KeyBinding::new("escape", Close, Some(MENU)),
        KeyBinding::new("tab", Close, Some(MENU)),
        KeyBinding::new("shift-tab", Close, Some(MENU)),
        KeyBinding::new("down", NextEntry, Some(MENU)),
        KeyBinding::new("up", Previous, Some(MENU)),
        KeyBinding::new("escape", Close, Some(DIALOG)),
        KeyBinding::new("enter", DefaultAction, Some(DIALOG)),
        KeyBinding::new("enter", Activate, Some(CONTROL)),
        KeyBinding::new("space", Activate, Some(CONTROL)),
    ]);
}

/// The Manage menu's entries, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Feature(Feature),
    WriteAccess,
    /// `--demo` only: the frame with a fake row.
    Sample,
}

impl Entry {
    fn label(self) -> String {
        match self {
            Self::Feature(feature) => format!("{}…", feature.label()),
            Self::WriteAccess => "Write access…".to_owned(),
            Self::Sample => "Sample dialog (demo)…".to_owned(),
        }
    }

    /// The element id of the entry's row in the menu.
    pub fn id(self) -> &'static str {
        match self {
            Self::Feature(Feature::Projects) => "manage-projects",
            Self::Feature(Feature::WorkItems) => "manage-work-items",
            Self::Feature(Feature::Fleet) => "manage-fleet",
            Self::WriteAccess => "manage-write-access",
            Self::Sample => "manage-sample",
        }
    }
}

/// Sent to the board after a write lands, so it reloads pmd data without waiting for the next refresh.
pub enum ManageEvent {
    Wrote,
}

/// One control in a dialog or the menu that holds keyboard focus.
struct Control {
    focus: FocusHandle,
}

/// The unsaved-edit question, in place of the footer.
struct Confirm {
    keep: FocusHandle,
    discard: FocusHandle,
}

struct WriteAccessDialog {
    input: Entity<InputState>,
    storing: bool,
    error: Option<String>,
    save: Control,
}

struct SampleDialog {
    form: Option<Form>,
    load_error: Option<String>,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    save: Control,
    reapply: Control,
    discard_theirs: Control,
}

enum Content {
    WriteAccess(WriteAccessDialog),
    Sample(Box<SampleDialog>),
}

struct Dialog {
    /// Unique per opening, so work started by a dialog that has since closed can't touch a newer one.
    id: u64,
    entry: Entry,
    focus: FocusHandle,
    close: Control,
    cancel: Control,
    confirm: Option<Confirm>,
    content: Content,
}

pub struct Manage {
    backend: Arc<dyn WriteBackend>,
    /// The demo's fake row; `None` hides the sample entry.
    sample: Option<Arc<dyn WriteBackend>>,
    config_path: Option<PathBuf>,
    access: Access,
    /// Bumped when the backend changes, so an older whoami can't overwrite newer access.
    generation: u64,
    button: FocusHandle,
    button_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    menu_open: bool,
    menu_focus: FocusHandle,
    entries: Vec<(Entry, Control)>,
    dialog: Option<Dialog>,
    dialogs_opened: u64,
}

impl EventEmitter<ManageEvent> for Manage {}

impl Manage {
    pub fn new(
        backend: Arc<dyn WriteBackend>,
        sample: Option<Arc<dyn WriteBackend>>,
        config_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut entries: Vec<Entry> = Feature::ALL.into_iter().map(Entry::Feature).collect();
        entries.push(Entry::WriteAccess);
        if sample.is_some() {
            entries.push(Entry::Sample);
        }
        let access = initial_access(backend.as_ref());
        Self {
            backend,
            sample,
            config_path,
            access,
            generation: 0,
            button: cx.focus_handle().tab_stop(true),
            button_bounds: Rc::new(Cell::new(None)),
            menu_open: false,
            menu_focus: cx.focus_handle(),
            entries: entries.into_iter().map(|entry| (entry, control(cx))).collect(),
            dialog: None,
            dialogs_opened: 0,
        }
    }

    /// Follows an edited config: a new write-token path or pmd URL forgets what was known about access.
    pub fn set_backend(
        &mut self,
        backend: Arc<dyn WriteBackend>,
        config_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.backend = backend;
        self.config_path = config_path;
        self.generation += 1;
        self.access = initial_access(self.backend.as_ref());
        if self.dialog.as_ref().is_some_and(|d| d.entry == Entry::WriteAccess) {
            self.check_access(cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn access(&self) -> &Access {
        &self.access
    }

    #[cfg(test)]
    pub fn is_menu_open(&self) -> bool {
        self.menu_open
    }

    /// The open dialog's entry.
    #[cfg(test)]
    pub fn dialog(&self) -> Option<Entry> {
        self.dialog.as_ref().map(|dialog| dialog.entry)
    }

    #[cfg(test)]
    pub fn is_confirming(&self) -> bool {
        self.dialog.as_ref().is_some_and(|dialog| dialog.confirm.is_some())
    }

    #[cfg(test)]
    pub fn button_focus(&self) -> &FocusHandle {
        &self.button
    }

    /// The sample dialog's form, for tests.
    #[cfg(test)]
    pub fn sample_form(&self) -> Option<&Form> {
        match self.dialog.as_ref().map(|dialog| &dialog.content) {
            Some(Content::Sample(sample)) => sample.form.as_ref(),
            _ => None,
        }
    }

    /// The input for a dialog field (`token`, or a sample field), for tests and captures.
    #[cfg(test)]
    pub fn input(&self, field: &str) -> Option<Entity<InputState>> {
        match &self.dialog.as_ref()?.content {
            Content::WriteAccess(dialog) => (field == "token").then(|| dialog.input.clone()),
            Content::Sample(sample) => sample
                .inputs
                .iter()
                .find(|(name, _)| *name == field)
                .map(|(_, input)| input.clone()),
        }
    }

    fn enabled(&self, entry: Entry) -> Result<(), String> {
        match entry {
            Entry::Feature(feature) => entry_state(feature, BUILT.contains(&feature), &self.access),
            Entry::WriteAccess | Entry::Sample => Ok(()),
        }
    }

    /// Asks pmd who the write token is, unless that is known or already being asked.
    fn check_access(&mut self, cx: &mut Context<Self>) {
        if matches!(self.access, Access::Off | Access::Checking) {
            return;
        }
        self.access = Access::Checking;
        let backend = Arc::clone(&self.backend);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { backend.whoami() }).await;
            let _ = this.update(cx, |manage, cx| {
                if manage.generation == generation {
                    manage.access = match result {
                        Ok(whoami) => Access::Known(whoami),
                        Err(error) => Access::Failed(error),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn toggle_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu_open {
            self.close_menu(window, cx);
        } else {
            self.open_menu(window, cx);
        }
    }

    pub fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            return;
        }
        if matches!(self.access, Access::Unchecked | Access::Failed(_)) {
            self.check_access(cx);
        }
        self.menu_open = true;
        if let Some((_, first)) = self.entries.first() {
            first.focus.focus(window, cx);
        }
        cx.notify();
    }

    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.button.focus(window, cx);
        cx.notify();
    }

    fn move_in_menu(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.entries.len() as isize;
        let current = self
            .entries
            .iter()
            .position(|(_, control)| control.focus.contains_focused(window, cx))
            .map_or(-1, |ix| ix as isize);
        let next = (current + delta).rem_euclid(count) as usize;
        self.entries[next].1.focus.focus(window, cx);
        cx.notify();
    }

    /// Opens an entry's dialog. A disabled entry does nothing; its reason is already on screen.
    pub fn activate(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabled(entry).is_err() {
            return;
        }
        self.menu_open = false;
        self.open_dialog(entry, window, cx);
    }

    fn open_dialog(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.dialogs_opened += 1;
        let id = self.dialogs_opened;
        let content = match entry {
            Entry::WriteAccess => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder("Paste the token pmd printed")
                });
                cx.observe(&input, move |manage, input, cx| {
                    if let Some(Content::WriteAccess(dialog)) = manage.content_of(id)
                        && !input.read(cx).value().is_empty()
                    {
                        dialog.error = None;
                    }
                    cx.notify();
                })
                .detach();
                if matches!(self.access, Access::Unchecked | Access::Failed(_)) {
                    self.check_access(cx);
                }
                Content::WriteAccess(WriteAccessDialog {
                    input,
                    storing: false,
                    error: None,
                    save: control(cx),
                })
            }
            Entry::Sample => {
                let inputs = SAMPLE_FIELDS
                    .iter()
                    .map(|(field, _)| (*field, cx.new(|cx| InputState::new(window, cx))))
                    .collect::<Vec<_>>();
                for (field, input) in &inputs {
                    let field = *field;
                    cx.observe(input, move |manage, input, cx| {
                        if let Some(Content::Sample(sample)) = manage.content_of(id)
                            && let Some(form) = sample.form.as_mut()
                        {
                            let text = input.read(cx).value().to_string();
                            if form.value(field) != text {
                                form.set(field, text);
                                cx.notify();
                            }
                        }
                    })
                    .detach();
                }
                self.load_sample(id, window, cx);
                Content::Sample(Box::new(SampleDialog {
                    form: None,
                    load_error: None,
                    inputs,
                    save: control(cx),
                    reapply: control(cx),
                    discard_theirs: control(cx),
                }))
            }
            Entry::Feature(_) => return,
        };
        let dialog = Dialog {
            id,
            entry,
            focus: cx.focus_handle(),
            close: control(cx),
            cancel: control(cx),
            confirm: None,
            content,
        };
        // Focus goes to the dialog's first field, or its default button when it has no field.
        match &dialog.content {
            Content::WriteAccess(content) if self.backend.token_path().is_some() => {
                content.input.update(cx, |input, cx| input.focus(window, cx));
            }
            Content::WriteAccess(_) => dialog.cancel.focus.focus(window, cx),
            Content::Sample(sample) => {
                let first = sample.inputs[0].1.clone();
                first.update(cx, |input, cx| input.focus(window, cx));
            }
        }
        self.dialog = Some(dialog);
        cx.notify();
    }

    /// Esc or the close button: asks first when there are unsaved edits. Esc on the question keeps editing.
    pub fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.confirm.is_some() {
            self.keep_editing(window, cx);
            return;
        }
        if dialog.is_dirty(cx) {
            let confirm = Confirm {
                keep: cx.focus_handle().tab_stop(true),
                discard: cx.focus_handle().tab_stop(true),
            };
            confirm.keep.focus(window, cx);
            dialog.confirm = Some(confirm);
            cx.notify();
            return;
        }
        self.close_dialog(window, cx);
    }

    pub fn keep_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.confirm = None;
            dialog.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Closes the dialog without asking and gives focus back to the Manage button.
    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.button.focus(window, cx);
        cx.notify();
    }

    /// Enter in a dialog: its default action. On the unsaved-edit question that is "Keep editing", the safe choice.
    pub fn default_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_ref() else {
            return;
        };
        if dialog.confirm.is_some() {
            self.keep_editing(window, cx);
            return;
        }
        match &dialog.content {
            Content::WriteAccess(_) if self.backend.token_path().is_none() => self.close_dialog(window, cx),
            Content::WriteAccess(_) => self.store_token(window, cx),
            Content::Sample(sample) if sample.form.as_ref().is_some_and(|f| f.conflict_state().is_some()) => {
                self.reapply(window, cx)
            }
            Content::Sample(_) => self.save_sample(window, cx),
        }
    }

    /// Stores the pasted token, then asks pmd what it allows. The token moves to a background thread and is never
    /// kept, logged or shown; the field is cleared once it is stored.
    pub fn store_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let generation = self.generation;
        let Some(id) = self.dialog.as_ref().map(|dialog| dialog.id) else {
            return;
        };
        let Some(Content::WriteAccess(dialog)) = self.content_of(id) else {
            return;
        };
        if dialog.storing {
            return;
        }
        let token = dialog.input.read(cx).value().trim().to_owned();
        if token.is_empty() {
            dialog.error = Some("Paste a token first.".to_owned());
            cx.notify();
            return;
        }
        dialog.storing = true;
        dialog.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    backend.store_token(&token)?;
                    Ok::<_, WriteError>(backend.whoami())
                })
                .await;
            let _ = this.update_in(cx, |manage, window, cx| {
                if let Some(Content::WriteAccess(dialog)) = manage.content_of(id) {
                    dialog.storing = false;
                    match &result {
                        Ok(_) => dialog.input.update(cx, |input, cx| input.set_value("", window, cx)),
                        Err(error) => dialog.error = Some(error.to_string()),
                    }
                }
                if let Ok(whoami) = result
                    && manage.generation == generation
                {
                    manage.access = match whoami {
                        Ok(whoami) => Access::Known(whoami),
                        Err(error) => Access::Failed(error),
                    };
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The open dialog's content, only while it is still dialog `id`, the one that started the work now landing.
    fn content_of(&mut self, id: u64) -> Option<&mut Content> {
        self.dialog
            .as_mut()
            .filter(|dialog| dialog.id == id)
            .map(|dialog| &mut dialog.content)
    }

    fn load_sample(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.sample.clone() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.read(SAMPLE_PATH) })
                .await
                .and_then(|value| Row::from_json(&value, &sample_fields()));
            let _ = this.update_in(cx, |manage, window, cx| {
                let Some(Content::Sample(sample)) = manage.content_of(id) else {
                    return;
                };
                match result {
                    Ok(row) => {
                        sample.form = Some(Form::edit(row, &sample_fields()));
                        sample.sync_inputs(window, cx);
                    }
                    Err(error) => sample.load_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves the sample row: checks the fields, then one `PATCH` with `If-Match`. A second press while it runs does
    /// nothing.
    pub fn save_sample(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.sample.clone() else {
            return;
        };
        let Some(id) = self.dialog.as_ref().map(|dialog| dialog.id) else {
            return;
        };
        let Some(Content::Sample(sample)) = self.content_of(id) else {
            return;
        };
        let Some(form) = sample.form.as_mut() else {
            return;
        };
        if form.is_saving() {
            return;
        }
        let priority = match form.value("priority").trim() {
            "" => serde_json::Value::Null,
            text => match text.parse::<u64>() {
                Ok(number) => number.into(),
                Err(_) => {
                    form.reject("priority", "Priority is a number from 0 to 5, or empty.");
                    cx.notify();
                    return;
                }
            },
        };
        let body = serde_json::json!({"title": form.value("title"), "priority": priority});
        let Some(precondition) = form.begin_save() else {
            cx.notify();
            return;
        };
        sample.sync_disabled(cx);
        let request = WriteRequest {
            method: Method::Patch,
            path: SAMPLE_PATH.to_owned(),
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
                let Some(Content::Sample(sample)) = manage.content_of(id) else {
                    return Next::Show;
                };
                let Some(form) = sample.form.as_mut() else {
                    return Next::Show;
                };
                let next = match result.and_then(|value| Row::from_json(&value, &sample_fields())) {
                    Ok(row) => {
                        form.saved(row);
                        sample.sync_inputs(window, cx);
                        cx.emit(ManageEvent::Wrote);
                        Next::Show
                    }
                    Err(error) => form.failed(&error),
                };
                sample.sync_disabled(cx);
                cx.notify();
                next
            });
            if !matches!(next, Ok(Next::Reread)) {
                return;
            }
            // Changed elsewhere: read the row again with the read token, then show both.
            let fresh = cx
                .background_executor()
                .spawn(async move { reader.read(SAMPLE_PATH) })
                .await
                .and_then(|value| Row::from_json(&value, &sample_fields()));
            let _ = this.update_in(cx, |manage, window, cx| {
                let Some(Content::Sample(sample)) = manage.content_of(id) else {
                    return;
                };
                if let Some(form) = sample.form.as_mut() {
                    match fresh {
                        Ok(row) => form.conflict(row),
                        Err(error) => form.reread_failed(&error),
                    }
                }
                sample.sync_disabled(cx);
                sample.reapply.focus.focus(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Changed elsewhere": keeps the user's values on the new version and saves them.
    pub fn reapply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Content::Sample(sample)) = self.dialog.as_mut().map(|d| &mut d.content)
            && let Some(form) = sample.form.as_mut()
        {
            form.reapply();
            sample.sync_disabled(cx);
        }
        self.save_sample(window, cx);
    }

    /// "Changed elsewhere": drops the user's values for the new ones.
    pub fn discard_mine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Content::Sample(sample)) = self.dialog.as_mut().map(|d| &mut d.content)
            && let Some(form) = sample.form.as_mut()
        {
            form.discard();
            sample.sync_inputs(window, cx);
            sample.sync_disabled(cx);
            let first = sample.inputs[0].1.clone();
            first.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// The Manage button for the title bar. Enter, Space or Down opens the menu.
    pub fn render_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let focused_ring = theme.ring;
        let bounds = Rc::clone(&self.button_bounds);
        div()
            .id("manage-button")
            .test_support()
            .role(Role::Button)
            .aria_label("Manage")
            .aria_expanded(self.menu_open)
            .key_context(BUTTON)
            .track_focus(&self.button)
            .tab_index(0)
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(2.))
            .rounded(theme.radius)
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .text_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary_hover))
            .when(self.menu_open, |this| this.bg(theme.secondary))
            .focus(move |style| style.border_color(focused_ring))
            .child("Manage")
            .child(Icon::new(IconName::ChevronDown).xsmall())
            .on_action(cx.listener(|manage, _: &OpenMenu, window, cx| manage.open_menu(window, cx)))
            .on_click(cx.listener(|manage, _, window, cx| manage.toggle_menu(window, cx)))
            .child(
                canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .into_any_element()
    }
}

fn control(cx: &mut App) -> Control {
    Control {
        focus: cx.focus_handle().tab_stop(true),
    }
}

fn initial_access(backend: &dyn WriteBackend) -> Access {
    match backend.token_path() {
        Some(_) => Access::Unchecked,
        None => Access::Off,
    }
}

fn sample_fields() -> Vec<&'static str> {
    SAMPLE_FIELDS.iter().map(|(field, _)| *field).collect()
}

impl Dialog {
    fn is_dirty(&self, cx: &App) -> bool {
        match &self.content {
            Content::WriteAccess(dialog) => !dialog.input.read(cx).value().trim().is_empty(),
            Content::Sample(sample) => sample.form.as_ref().is_some_and(Form::is_dirty),
        }
    }
}

impl SampleDialog {
    /// Puts the form's values into the inputs, after a load, a save or a discard.
    fn sync_inputs(&self, window: &mut Window, cx: &mut Context<Manage>) {
        let Some(form) = &self.form else {
            return;
        };
        for (field, input) in &self.inputs {
            let text = form.value(field).to_owned();
            if input.read(cx).value() != text.as_str() {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    /// Fields can't change while a save runs or a conflict waits for the user's choice.
    fn sync_disabled(&self, cx: &mut Context<Manage>) {
        let locked = self
            .form
            .as_ref()
            .is_some_and(|form| form.is_saving() || form.conflict_state().is_some());
        for (_, input) in &self.inputs {
            input.update(cx, |input, cx| input.set_disabled(locked, cx));
        }
    }
}

/// A focusable button for the menu and the dialogs: Enter or Space activates it, and a ring shows focus.
#[allow(clippy::too_many_arguments)]
fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    focus: &FocusHandle,
    primary: bool,
    disabled: bool,
    busy: bool,
    on_activate: impl Fn(&mut Manage, &mut Window, &mut Context<Manage>) + 'static,
    cx: &mut Context<Manage>,
) -> AnyElement {
    button_with_icon(id, label, None, focus, primary, disabled, busy, on_activate, cx)
}

/// [`button`], showing `icon` in place of its label; the label still names it for assistive technology.
#[allow(clippy::too_many_arguments)]
fn button_with_icon(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    focus: &FocusHandle,
    primary: bool,
    disabled: bool,
    busy: bool,
    on_activate: impl Fn(&mut Manage, &mut Window, &mut Context<Manage>) + 'static,
    cx: &mut Context<Manage>,
) -> AnyElement {
    let theme = cx.theme();
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    // The ring stands out against the button's own fill: light on gold, gold on grey.
    let (bg, fg, hover, ring) = if primary {
        (
            theme.primary,
            theme.primary_foreground,
            theme.primary.opacity(0.85),
            theme.foreground,
        )
    } else {
        (theme.secondary, theme.foreground, theme.secondary_hover, theme.ring)
    };
    let inert = disabled || busy;
    let on_activate = Rc::new(on_activate);
    let activate = Rc::clone(&on_activate);
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label(label.clone())
        .key_context(CONTROL)
        .track_focus(focus)
        .tab_index(0)
        .flex()
        .items_center()
        .gap_1()
        .px_3()
        .py_1()
        .rounded(theme.radius)
        .border_2()
        .border_color(gpui_kit::transparent_black())
        .bg(bg)
        .text_color(fg)
        .text_sm()
        .when(inert, |this| this.opacity(0.6))
        .when(!inert, |this| this.cursor_pointer().hover(move |style| style.bg(hover)))
        .focus(move |style| style.border_color(ring))
        .when(busy, |this| this.child(Icon::new(IconName::LoaderCircle).xsmall()))
        .map(|this| match icon {
            Some(icon) => this.px_1().child(Icon::new(icon).small()),
            None => this.child(label),
        })
        .on_action(cx.listener(move |manage, _: &Activate, window, cx| {
            if !inert {
                activate(manage, window, cx);
            }
        }))
        .on_click(cx.listener(move |manage, _, window, cx| {
            if !inert {
                on_activate(manage, window, cx);
            }
        }))
        .into_any_element()
}

impl Manage {
    fn render_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let viewport = window.viewport_size();
        let width = rems(19.).to_pixels(window.rem_size());
        let anchor = self.button_bounds.get();
        let left = anchor.map_or(viewport.width - width - px(8.), |b| (b.right() - width).max(px(8.)));
        let top = anchor.map_or(px(36.), |b| b.bottom() + px(4.));
        let mut items = Vec::new();
        for (entry, control) in &self.entries {
            let entry = *entry;
            let state = self.enabled(entry);
            let focused = control.focus.is_focused(window);
            let disabled = state.is_err();
            let label = entry.label();
            items.push(
                v_flex()
                    .id(entry.id())
                    .test_support()
                    .role(Role::MenuItem)
                    .aria_label(label.clone())
                    .when_some(state.clone().err(), |this, reason| this.aria_description(reason))
                    .key_context(CONTROL)
                    .track_focus(&control.focus)
                    .tab_index(0)
                    .px_2()
                    .py_1()
                    .rounded(theme.radius)
                    .border_2()
                    .border_color(if focused {
                        theme.ring
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .when(focused && !disabled, |this| this.bg(theme.accent))
                    .when(!disabled, |this| this.cursor_pointer().hover(|s| s.bg(theme.accent)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(if disabled {
                                theme.muted_foreground
                            } else {
                                theme.foreground
                            })
                            .child(label),
                    )
                    .when_some(state.err(), |this, reason| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("{}-reason", entry.id())))
                                .test_support()
                                .aria_label(reason.clone())
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(reason),
                        )
                    })
                    .on_action(cx.listener(move |manage, _: &Activate, window, cx| manage.activate(entry, window, cx)))
                    .on_click(cx.listener(move |manage, _, window, cx| manage.activate(entry, window, cx)))
                    .into_any_element(),
            );
            if entry == Entry::Feature(Feature::Fleet) {
                items.push(div().my_1().h(px(1.)).bg(theme.border).into_any_element());
            }
        }
        div()
            .absolute()
            .size_full()
            // A click outside the menu closes it, and never reaches the table.
            .child(
                div()
                    .id("manage-menu-backdrop")
                    .test_support()
                    .absolute()
                    .size_full()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|manage, _, window, cx| manage.close_menu(window, cx)),
                    ),
            )
            .child(
                v_flex()
                    .id("manage-menu")
                    .test_support()
                    .role(Role::Menu)
                    .aria_label("Manage")
                    .key_context(MENU)
                    .track_focus(&self.menu_focus)
                    .occlude()
                    .absolute()
                    .left(left)
                    .top(top)
                    .w(width)
                    .p_1()
                    .gap(px(2.))
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .on_action(cx.listener(|manage, _: &Close, window, cx| manage.close_menu(window, cx)))
                    .on_action(cx.listener(|manage, _: &NextEntry, window, cx| manage.move_in_menu(1, window, cx)))
                    .on_action(cx.listener(|manage, _: &Previous, window, cx| manage.move_in_menu(-1, window, cx)))
                    .children(items),
            )
            .into_any_element()
    }

    /// The shared modal frame: a backdrop that swallows input, a focus-trapped panel with a title, a close button,
    /// the body, a dialog-level message and the footer, or the unsaved-edit question in its place.
    fn render_frame(&self, dialog: &Dialog, frame: Frame, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Frame {
            title,
            body,
            message,
            footer,
        } = frame;
        let title = title.as_str();
        let theme = cx.theme().clone();
        let close = button_with_icon(
            "dialog-close",
            "Close",
            Some(IconName::Close),
            &dialog.close.focus,
            false,
            false,
            false,
            |manage, window, cx| manage.request_close(window, cx),
            cx,
        );
        let bottom = match &dialog.confirm {
            Some(confirm) => h_flex()
                .id("unsaved-confirm")
                .test_support()
                .role(Role::AlertDialog)
                .aria_label("Discard unsaved changes?")
                .gap_2()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning)
                .bg(theme.warning.opacity(0.08))
                .child(Icon::new(IconName::TriangleAlert).text_color(theme.warning))
                .child(div().flex_1().text_sm().child("Discard unsaved changes?"))
                .child(button(
                    "confirm-keep",
                    "Keep editing",
                    &confirm.keep,
                    true,
                    false,
                    false,
                    |manage, window, cx| manage.keep_editing(window, cx),
                    cx,
                ))
                .child(button(
                    "confirm-discard",
                    "Discard",
                    &confirm.discard,
                    false,
                    false,
                    false,
                    |manage, window, cx| manage.close_dialog(window, cx),
                    cx,
                ))
                .into_any_element(),
            None => h_flex().justify_end().gap_2().children(footer).into_any_element(),
        };
        let max_height = (window.viewport_size().height - rems(6.).to_pixels(window.rem_size())).max(px(160.));
        div()
            .absolute()
            .size_full()
            .child(
                div()
                    .id("manage-dialog-backdrop")
                    .absolute()
                    .size_full()
                    .occlude()
                    .bg(theme.overlay),
            )
            .child(
                v_flex()
                    .absolute()
                    .size_full()
                    .items_center()
                    .pt(rems(3.))
                    // The labelled dialog element wraps the focus trap, which takes over its child's id.
                    .child(
                        div()
                            .id("manage-dialog")
                            .test_support()
                            .role(Role::Dialog)
                            .aria_label(title.to_owned())
                            .child(
                                v_flex()
                                    .id("manage-dialog-panel")
                                    .key_context(DIALOG)
                                    .track_focus(&dialog.focus)
                                    .occlude()
                                    .w(rems(32.))
                                    .max_w(window.viewport_size().width - px(32.))
                                    .max_h(max_height)
                                    .gap_3()
                                    .p_4()
                                    .rounded(theme.radius_lg)
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.popover)
                                    .shadow_lg()
                                    .on_action(
                                        cx.listener(|manage, _: &Close, window, cx| manage.request_close(window, cx)),
                                    )
                                    .on_action(cx.listener(|manage, _: &DefaultAction, window, cx| {
                                        manage.default_action(window, cx)
                                    }))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(div().flex_1().font_semibold().text_lg().child(title.to_owned()))
                                            .child(close),
                                    )
                                    .child(
                                        div()
                                            .id("manage-dialog-body")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .child(body),
                                    )
                                    .when_some(message, |this, (text, error)| {
                                        this.child(
                                            div()
                                                .id("dialog-message")
                                                .test_support()
                                                .role(Role::Status)
                                                .aria_label(text.clone())
                                                .text_sm()
                                                .text_color(if error { theme.danger } else { theme.warning })
                                                .child(text),
                                        )
                                    })
                                    .child(bottom)
                                    .focus_trap("manage-dialog-trap", &dialog.focus),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_write_access(
        &self,
        dialog: &Dialog,
        content: &WriteAccessDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let cancel_label = if self.backend.token_path().is_some() {
            "Cancel"
        } else {
            "Close"
        };
        let cancel = button(
            "dialog-cancel",
            cancel_label,
            &dialog.cancel.focus,
            self.backend.token_path().is_none(),
            false,
            false,
            |manage, window, cx| manage.request_close(window, cx),
            cx,
        );
        let Some(path) = self.backend.token_path() else {
            let config = self
                .config_path
                .as_ref()
                .map_or("config.toml".to_owned(), |path| path.display().to_string());
            let body = v_flex()
                .id("write-access-off")
                .test_support()
                .gap_2()
                .text_sm()
                .child("Writes are off. The Manage dialogs write with their own pmd token, kept in a file of its own.")
                .child(step(
                    "1",
                    "On mini, mint a token with the scopes the dialogs need:".into(),
                    cx,
                ))
                .child(code(
                    "pmd token add project-dashboard@home --scopes read,write:work,write:fleet,write:projects",
                    cx,
                ))
                .child(step("2", format!("Add this line to {config}:"), cx))
                .child(code("pmd_write_token_path = \"pmd-write-token\"", cx))
                .child(step(
                    "3",
                    "Open Write access… again and paste the token. It is stored owner-only.".into(),
                    cx,
                ))
                .into_any_element();
            return self.render_frame(
                dialog,
                Frame {
                    title: "Write access".into(),
                    body,
                    message: None,
                    footer: vec![cancel],
                },
                window,
                cx,
            );
        };
        let (name, scopes) = match &self.access {
            Access::Known(whoami) => (whoami.name.clone(), whoami.scopes.join(", ")),
            Access::Checking | Access::Unchecked => ("Checking…".to_owned(), "Checking…".to_owned()),
            Access::Failed(WriteError::MissingToken) => ("None stored yet".to_owned(), "—".to_owned()),
            Access::Failed(error) => (error.to_string(), "—".to_owned()),
            Access::Off => ("—".to_owned(), "—".to_owned()),
        };
        let allowed = Feature::ALL.map(|feature| {
            let state = match &self.access {
                Access::Known(whoami) if whoami.allows(feature) => ("Allowed", theme.success),
                Access::Known(_) => ("Missing", theme.danger),
                _ => ("Unknown", theme.muted_foreground),
            };
            h_flex()
                .id(SharedString::from(format!("scope-{}", feature.scope())))
                .test_support()
                .aria_label(format!("{}: {} {}", feature.label(), feature.scope(), state.0))
                .gap_2()
                .text_sm()
                .child(div().w(rems(7.)).child(feature.label()))
                .child(
                    div()
                        .w(rems(8.))
                        .text_color(theme.muted_foreground)
                        .child(feature.scope()),
                )
                .child(div().text_color(state.1).child(state.0))
                .into_any_element()
        });
        let body = v_flex()
            .id("write-access")
            .test_support()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(fact("write-token-path", "Token file", path.display().to_string(), cx))
                    .child(fact("write-token-name", "Token", name, cx))
                    .child(fact("write-token-scopes", "Scopes", scopes, cx)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Dialogs this token allows"),
                    )
                    .children(allowed),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Paste a new token"))
                    .child(
                        Input::new(&content.input)
                            .id("write-token-input")
                            .aria_label("New write token")
                            .disabled(content.storing),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Mint one on mini with pmd token add …; it replaces the stored token."),
                    ),
            )
            .into_any_element();
        let save = button(
            "write-token-save",
            if content.storing { "Saving…" } else { "Save token" },
            &content.save.focus,
            true,
            false,
            content.storing,
            |manage, window, cx| manage.store_token(window, cx),
            cx,
        );
        let message = content.error.clone().map(|error| (error, true));
        self.render_frame(
            dialog,
            Frame {
                title: "Write access".into(),
                body,
                message,
                footer: vec![cancel, save],
            },
            window,
            cx,
        )
    }

    fn render_sample(
        &self,
        dialog: &Dialog,
        sample: &SampleDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let cancel = button(
            "dialog-cancel",
            "Cancel",
            &dialog.cancel.focus,
            false,
            false,
            false,
            |manage, window, cx| manage.request_close(window, cx),
            cx,
        );
        let Some(form) = &sample.form else {
            let text = sample.load_error.clone().unwrap_or_else(|| "Loading…".to_owned());
            return self.render_frame(
                dialog,
                Frame {
                    title: "Sample row".into(),
                    body: div().text_sm().child(text).into_any_element(),
                    message: None,
                    footer: vec![cancel],
                },
                window,
                cx,
            );
        };
        let saving = form.is_saving();
        let mut body = v_flex().id("sample-form").test_support().gap_3();
        for ((field, label), (_, input)) in SAMPLE_FIELDS.iter().zip(&sample.inputs) {
            let error = form.field_error(field).map(str::to_owned);
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child(*label))
                    .child(
                        Input::new(input)
                            .id(SharedString::from(format!("field-{field}")))
                            .aria_label(*label),
                    )
                    .when_some(error, |this, error| {
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
            );
        }
        let footer = if let Some(conflict) = form.conflict_state() {
            body = body.child(conflict_panel(conflict, &SAMPLE_FIELDS, cx));
            vec![
                cancel,
                button(
                    "conflict-discard",
                    "Discard mine",
                    &sample.discard_theirs.focus,
                    false,
                    false,
                    false,
                    |manage, window, cx| manage.discard_mine(window, cx),
                    cx,
                ),
                button(
                    "conflict-reapply",
                    "Reapply mine",
                    &sample.reapply.focus,
                    true,
                    false,
                    false,
                    |manage, window, cx| manage.reapply(window, cx),
                    cx,
                ),
            ]
        } else {
            vec![
                cancel,
                button(
                    "sample-save",
                    if saving { "Saving…" } else { "Save" },
                    &sample.save.focus,
                    true,
                    false,
                    saving,
                    |manage, window, cx| manage.save_sample(window, cx),
                    cx,
                ),
            ]
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
                title: "Sample row".into(),
                body: body.into_any_element(),
                message,
                footer,
            },
            window,
            cx,
        )
    }
}

/// What a dialog puts in the shared frame: its title, its body, a dialog-level message (`true` for an error, else a
/// warning) and its footer buttons.
struct Frame {
    title: String,
    body: AnyElement,
    message: Option<(String, bool)>,
    footer: Vec<AnyElement>,
}

/// The reusable "Changed elsewhere" panel: for each field that differs, the user's value next to the new one.
fn conflict_panel(conflict: &dashboard_core::edit::Conflict, labels: &[(&str, &str)], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label_of = |field: &str| {
        labels
            .iter()
            .find(|(key, _)| *key == field)
            .map_or(field.to_owned(), |(_, label)| (*label).to_owned())
    };
    let shown = |text: &str| {
        if text.is_empty() {
            "(empty)".to_owned()
        } else {
            text.to_owned()
        }
    };
    // Equal columns, whatever the values' lengths, so "Yours" and "Now in pmd" line up row after row.
    let column = || div().flex_1().flex_basis(px(0.)).min_w_0();
    v_flex()
        .id("conflict")
        .test_support()
        .aria_label("Changed elsewhere")
        .gap_1()
        .p_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.warning)
        .child(
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().w(rems(9.)).flex_none().child("Field"))
                .child(column().child("Yours"))
                .child(column().child("Now in pmd")),
        )
        .children(conflict.differences().into_iter().map(|(field, mine, theirs)| {
            h_flex()
                .id(SharedString::from(format!("conflict-{field}")))
                .test_support()
                .aria_label(format!(
                    "{}: yours {}, now {}",
                    label_of(field),
                    shown(mine),
                    shown(theirs)
                ))
                .gap_2()
                .items_start()
                .text_sm()
                .child(
                    div()
                        .w(rems(9.))
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(label_of(field)),
                )
                .child(column().child(shown(mine)))
                .child(column().text_color(theme.warning).child(shown(theirs)))
        }))
        .into_any_element()
}

fn fact(id: &'static str, label: &str, value: String, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .test_support()
        .aria_label(format!("{label}: {value}"))
        .gap_2()
        .text_sm()
        .child(
            div()
                .w(rems(7.))
                .text_color(theme.muted_foreground)
                .child(label.to_owned()),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

fn step(number: &str, text: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .text_color(cx.theme().primary)
                .font_semibold()
                .child(number.to_owned()),
        )
        .child(div().flex_1().child(text))
        .into_any_element()
}

fn code(text: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .bg(theme.muted)
        .font_family("monospace")
        .text_xs()
        .child(text.to_owned())
        .into_any_element()
}

impl Render for Manage {
    /// The layer over the board: the open menu or dialog, or nothing.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layer = if let Some(dialog) = self.dialog.take() {
            let element = match &dialog.content {
                Content::WriteAccess(content) => self.render_write_access(&dialog, content, window, cx),
                Content::Sample(sample) => self.render_sample(&dialog, sample, window, cx),
            };
            self.dialog = Some(dialog);
            Some(element)
        } else if self.menu_open {
            Some(self.render_menu(window, cx))
        } else {
            None
        };
        div()
            .id("manage-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .when(layer.is_none(), |this| this.size_0())
            .children(layer)
    }
}
