//! The Projects dialog (#34): every pmd project, sorted by priority, with an editor that tracks a new repo, sets
//! P0–P5 or no tier, edits goal, notes and requires, untracks or re-tracks a project, and links a local checkout so
//! the dashboard shows it.
//!
//! pmd's projects table is the source of truth for priority (option C of project-manager#50), so there is no reset to
//! PORTFOLIO.md: a save is final, and the table's Priority column follows at once. The list reads with the read
//! token, so it opens without write access, read-only, naming the missing scope. Up and Down move through the list,
//! Enter edits, Ctrl+Enter saves, Esc closes with the unsaved-edit question, and Delete does nothing: projects are
//! never deleted. The data, checks and the checkout link live in `dashboard_core::projects`.
use std::path::PathBuf;
use std::sync::Arc;

use dashboard_core::config::Config;
use dashboard_core::edit::{Form, Next};
use dashboard_core::projects::{self, FIELDS, Project};
use dashboard_core::write::{Feature, Method, WriteRequest, write_state};
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
    Content, Control, Dialog, Frame, Manage, ManageEvent, OpenItem, WORK_LIST, WorkDown, WorkUp, button,
    conflict_panel, control,
};

/// The priority choices: P0 to P5, then no tier (empty).
const PRIORITY_CHOICES: [&str; 7] = ["0", "1", "2", "3", "4", "5", ""];
/// The fields typed as text; priority and tracked are chosen.
const TEXT_FIELDS: [&str; 5] = ["repo", "slug", "goal", "notes", "requires"];

pub struct ProjectsDialog {
    filter: Entity<InputState>,
    show_untracked: bool,
    untracked_toggle: Control,
    list: FocusHandle,
    scroll: ScrollHandle,
    rows: Vec<Project>,
    loading: bool,
    load_error: Option<String>,
    /// The configured checkouts' `owner/name`, lower case; `None` until read.
    linked: Option<Vec<String>>,
    selected: usize,
    add: Control,
    retry: Control,
    editor: Option<ProjectEditor>,
    editors_opened: u64,
}

pub struct ProjectEditor {
    key: u64,
    /// The project as pmd has it; `None` while adding one.
    project: Option<Project>,
    form: Form,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    priorities: Vec<Control>,
    folder: Entity<InputState>,
    link: Control,
    /// A checkout link's result: the message, and whether it is an error.
    link_result: Option<(String, bool)>,
    linking: bool,
    /// "Untrack …?" is showing in place of the footer.
    confirming_untrack: bool,
    untrack: Control,
    keep: Control,
    track: Control,
    back: Control,
    save: Control,
    reapply: Control,
    discard: Control,
}

impl ProjectsDialog {
    pub(super) fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(|editor| editor.form.is_dirty())
    }

    pub(super) fn focus_list(&self, window: &mut Window, cx: &mut Context<Manage>) {
        self.list.focus(window, cx);
    }

    /// The rows the list shows: filtered by text, and untracked ones only when asked.
    fn visible(&self, cx: &gpui_kit::App) -> Vec<usize> {
        let filter = self.filter.read(cx).value().to_string();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, project)| (self.show_untracked || project.tracked) && project.matches(&filter))
            .map(|(ix, _)| ix)
            .collect()
    }

    fn is_linked(&self, project: &Project) -> Option<bool> {
        self.linked
            .as_ref()
            .map(|linked| linked.iter().any(|repo| repo.eq_ignore_ascii_case(&project.repo)))
    }

    #[cfg(test)]
    pub fn visible_slugs(&self, cx: &gpui_kit::App) -> Vec<String> {
        self.visible(cx)
            .into_iter()
            .map(|ix| self.rows[ix].slug.clone())
            .collect()
    }

    #[cfg(test)]
    pub fn editor_form(&self) -> Option<&Form> {
        self.editor.as_ref().map(|editor| &editor.form)
    }

    #[cfg(test)]
    pub(super) fn input(&self, field: &str) -> Option<Entity<InputState>> {
        if field == "filter" {
            return Some(self.filter.clone());
        }
        let editor = self.editor.as_ref()?;
        if field == "folder" {
            return Some(editor.folder.clone());
        }
        editor
            .inputs
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, input)| input.clone())
    }
}

impl ProjectEditor {
    fn sync_inputs(&self, window: &mut Window, cx: &mut Context<Manage>) {
        for (field, input) in &self.inputs {
            let text = self.form.value(field).to_owned();
            if input.read(cx).value() != text.as_str() {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    fn locked(&self, writable: bool) -> bool {
        !writable || self.form.is_saving() || self.form.conflict_state().is_some()
    }

    fn sync_disabled(&self, writable: bool, cx: &mut Context<Manage>) {
        let locked = self.locked(writable);
        for (_, input) in &self.inputs {
            input.update(cx, |input, cx| input.set_disabled(locked, cx));
        }
    }
}

impl Manage {
    fn projects_writable(&self) -> bool {
        write_state(Feature::Projects, &self.access).is_ok()
    }

    fn projects_of(&mut self, id: u64) -> Option<&mut ProjectsDialog> {
        match self.content_of(id) {
            Some(Content::Projects(dialog)) => Some(dialog),
            _ => None,
        }
    }

    fn open_projects(&mut self) -> Option<(u64, &mut ProjectsDialog)> {
        let dialog = self.dialog.as_mut()?;
        match &mut dialog.content {
            Content::Projects(projects) => Some((dialog.id, projects)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn projects(&self) -> Option<&ProjectsDialog> {
        match &self.dialog.as_ref()?.content {
            Content::Projects(projects) => Some(projects),
            _ => None,
        }
    }

    /// config.toml's folder, where a checkout link writes.
    fn config_dir(&self) -> Option<PathBuf> {
        self.config_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from)
    }

    pub(super) fn new_projects(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> ProjectsDialog {
        if matches!(
            self.access,
            dashboard_core::write::Access::Unchecked | dashboard_core::write::Access::Failed(_)
        ) {
            self.check_access(cx);
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by slug, repository, goal or notes"));
        cx.observe(&filter, move |manage, _, cx| {
            if let Some(projects) = manage.projects_of(id) {
                projects.selected = 0;
            }
            cx.notify();
        })
        .detach();
        ProjectsDialog {
            filter,
            show_untracked: false,
            untracked_toggle: control(cx),
            list: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            rows: Vec::new(),
            loading: true,
            load_error: None,
            linked: None,
            selected: 0,
            add: control(cx),
            retry: control(cx),
            editor: None,
            editors_opened: 0,
        }
    }

    /// Loads every project, tracked or not, and which ones a configured checkout matches.
    pub(super) fn load_projects(&mut self, id: u64, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.backend);
        let config_dir = self.config_dir();
        let Some(projects) = self.projects_of(id) else {
            return;
        };
        projects.loading = true;
        cx.spawn(async move |this, cx| {
            let (result, linked) = cx
                .background_executor()
                .spawn(async move {
                    let result = backend
                        .list("v1/projects", &[("all".to_owned(), "1".to_owned())])
                        .and_then(|value| Project::list_from_json(&value));
                    let linked = config_dir
                        .and_then(|dir| Config::load(&dir).ok())
                        .map(|config| projects::linked_repos(&config.repositories));
                    (result, linked)
                })
                .await;
            let _ = this.update(cx, |manage, cx| {
                let Some(projects) = manage.projects_of(id) else {
                    return;
                };
                projects.loading = false;
                projects.linked = linked;
                match result {
                    Ok(rows) => {
                        // A project the dialog holds at a newer version, or added since the list was asked for,
                        // stays: an older answer never undoes a save.
                        let mut rows: Vec<Project> = rows
                            .into_iter()
                            .map(|row| match projects.rows.iter().find(|known| known.slug == row.slug) {
                                Some(known) if known.version > row.version => known.clone(),
                                _ => row,
                            })
                            .collect();
                        for known in &projects.rows {
                            if !rows.iter().any(|row| row.slug == known.slug) {
                                rows.push(known.clone());
                            }
                        }
                        projects::sort(&mut rows);
                        projects.rows = rows;
                        projects.load_error = None;
                    }
                    // The last good list stays beside the error.
                    Err(error) => projects.load_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_untracked(&mut self, cx: &mut Context<Self>) {
        if let Some((_, projects)) = self.open_projects() {
            projects.show_untracked = !projects.show_untracked;
            projects.selected = 0;
        }
        cx.notify();
    }

    fn move_in_projects(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible = match self.open_projects() {
            Some((_, projects)) => projects.visible(cx).len(),
            None => return,
        };
        let Some((_, projects)) = self.open_projects() else {
            return;
        };
        if visible == 0 {
            return;
        }
        projects.selected = (projects.selected as isize + delta).clamp(0, visible as isize - 1) as usize;
        projects.scroll.scroll_to_item(projects.selected);
        cx.notify();
    }

    /// Opens the editor on visible row `visible_ix`, or on a new project with `None`.
    pub fn open_project_editor(&mut self, visible_ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.projects_writable();
        if visible_ix.is_none() && !writable {
            return;
        }
        let Some((id, projects)) = self.open_projects() else {
            return;
        };
        let project = match visible_ix {
            Some(ix) => {
                let visible = projects.visible(cx);
                let Some(&row) = visible.get(ix) else {
                    return;
                };
                projects.selected = ix;
                Some(projects.rows[row].clone())
            }
            None => None,
        };
        projects.editors_opened += 1;
        let key = projects.editors_opened;
        let form = match &project {
            Some(found) => Form::edit(found.row(), &FIELDS),
            None => Form::create(&FIELDS, &projects::defaults()),
        };
        let inputs: Vec<(&'static str, Entity<InputState>)> = TEXT_FIELDS
            .iter()
            .copied()
            .filter(|field| project.is_none() || !matches!(*field, "repo" | "slug"))
            .map(|field| {
                let value = form.value(field).to_owned();
                let placeholder = match field {
                    "repo" => "owner/name, such as hemsoft-dev/gh-x",
                    "slug" => "The repository's name, lower case",
                    "goal" | "notes" => "One line, empty for none",
                    "requires" => "Machine capabilities it needs, such as apple",
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
                let Some(editor) = manage.projects_of(id).and_then(|projects| projects.editor.as_mut()) else {
                    return;
                };
                if editor.key == key && editor.form.value(field) != text {
                    editor.form.set(field, text);
                    cx.notify();
                }
            })
            .detach();
        }
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("The checkout's folder"));
        let controls = |count: usize, cx: &mut Context<Self>| (0..count).map(|_| control(cx)).collect::<Vec<_>>();
        let editor = ProjectEditor {
            key,
            project,
            form,
            inputs,
            priorities: controls(PRIORITY_CHOICES.len(), cx),
            folder,
            link: control(cx),
            link_result: None,
            linking: false,
            confirming_untrack: false,
            untrack: control(cx),
            keep: control(cx),
            track: control(cx),
            back: control(cx),
            save: control(cx),
            reapply: control(cx),
            discard: control(cx),
        };
        editor.sync_disabled(writable, cx);
        match editor.inputs.first() {
            Some((_, input)) if writable && editor.project.is_none() => {
                let input = input.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            _ if writable => {
                let current = PRIORITY_CHOICES
                    .iter()
                    .position(|p| *p == editor.form.value("priority"))
                    .unwrap_or(6);
                editor.priorities[current].focus.focus(window, cx);
            }
            _ => editor.back.focus.focus(window, cx),
        }
        projects.editor = Some(editor);
        cx.notify();
    }

    pub fn projects_back_to_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_projects().is_some_and(|(_, projects)| projects.is_dirty()) {
            self.ask_before_discarding(true, window, cx);
            return;
        }
        self.leave_project_editor(window, cx);
    }

    pub fn leave_project_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, projects)) = self.open_projects() {
            projects.editor = None;
            projects.list.focus(window, cx);
        }
        cx.notify();
    }

    fn set_project_priority(&mut self, priority: &str, cx: &mut Context<Self>) {
        let writable = self.projects_writable();
        let Some(editor) = self.open_projects().and_then(|(_, projects)| projects.editor.as_mut()) else {
            return;
        };
        if editor.locked(writable) {
            return;
        }
        // Picking only edits the form: nothing is written until Save.
        editor.form.set("priority", priority);
        if editor.form.value("tracked") == "false" {
            editor.form.clear_error("tracked");
        }
        cx.notify();
    }

    /// Ctrl+Enter or Save: checks the project, then one `PUT` of all of it.
    pub fn save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.put_project(None, window, cx);
    }

    /// One `PUT` of the editor's values, with `tracked` overridden for Untrack and Track again.
    fn put_project(&mut self, tracked: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.projects_writable();
        let backend = Arc::clone(&self.backend);
        let Some((id, projects)) = self.open_projects() else {
            return;
        };
        let Some(editor) = projects.editor.as_mut() else {
            return;
        };
        if editor.locked(writable) {
            return;
        }
        if let Some(tracked) = tracked {
            // Untrack and Track again change only that: edits must be saved or discarded first.
            if editor.form.is_dirty() {
                return;
            }
            editor.form.set("tracked", tracked.to_string());
        } else if !editor.form.is_dirty() {
            return;
        }
        // A new project's slug defaults to its repository's name, lower case.
        if editor.project.is_none() && editor.form.value("slug").trim().is_empty() {
            let slug = projects::slug_for(editor.form.value("repo"));
            editor.form.set("slug", slug);
            editor.sync_inputs(window, cx);
        }
        editor.confirming_untrack = false;
        let (slug, body) = match projects::body(editor.form.values()) {
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
        let key = editor.key;
        let path = projects::path(&slug);
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
                .await
                .and_then(|value| Project::from_json(&value));
            let next = this.update_in(cx, |manage, window, cx| {
                let writable = manage.projects_writable();
                if let Ok(saved) = &result {
                    // pmd took the write: the table's Priority column and the list follow, even when the editor or
                    // the dialog closed while it was on its way.
                    cx.emit(ManageEvent::Wrote);
                    if let Some(projects) = manage.projects_of(id) {
                        // A row the list holds at a newer version stays: a late answer never undoes a later write.
                        match projects.rows.iter_mut().find(|row| row.slug == saved.slug) {
                            Some(row) if row.version > saved.version => {}
                            Some(row) => *row = saved.clone(),
                            None => projects.rows.push(saved.clone()),
                        }
                        projects::sort(&mut projects.rows);
                    }
                }
                let projects = manage.projects_of(id)?;
                let editor = projects.editor.as_mut().filter(|editor| editor.key == key)?;
                let next = match result {
                    Ok(saved) => {
                        let created = editor.project.is_none();
                        editor.form.saved(saved.row());
                        editor.project = Some(saved);
                        editor.sync_inputs(window, cx);
                        created.then_some(Next::Show)
                    }
                    Err(error) => match projects::error_field(&error) {
                        Some(field) => {
                            editor.form.failed_on(field, &error);
                            None
                        }
                        None => (editor.form.failed(&error) == Next::Reread).then_some(Next::Reread),
                    },
                };
                editor.sync_disabled(writable, cx);
                cx.notify();
                next
            });
            match next {
                Ok(Some(Next::Show)) => {
                    // Created: reopen on the stored project, so its repo and slug are fixed.
                    let _ = this.update_in(cx, |manage, window, cx| {
                        manage.reopen_created_project(id, key, window, cx)
                    });
                    return;
                }
                Ok(Some(Next::Reread)) => {}
                _ => return,
            }
            let fresh = cx
                .background_executor()
                .spawn(async move { reader.read(&path) })
                .await
                .and_then(|value| Project::from_json(&value));
            let _ = this.update_in(cx, |manage, window, cx| {
                let writable = manage.projects_writable();
                let Some(projects) = manage.projects_of(id) else {
                    return;
                };
                if let Ok(fresh) = &fresh
                    && let Some(row) = projects.rows.iter_mut().find(|row| row.slug == fresh.slug)
                    && row.version <= fresh.version
                {
                    *row = fresh.clone();
                }
                let Some(editor) = projects.editor.as_mut().filter(|editor| editor.key == key) else {
                    cx.notify();
                    return;
                };
                match fresh {
                    Ok(project) => {
                        editor.form.conflict(project.row());
                        editor.project = Some(project);
                        editor.reapply.focus.focus(window, cx);
                    }
                    Err(error) => editor.form.reread_failed(&error),
                }
                editor.sync_disabled(writable, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reopen_created_project(&mut self, id: u64, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projects) = self.projects_of(id) else {
            return;
        };
        let Some(slug) = projects
            .editor
            .as_ref()
            .filter(|editor| editor.key == key && !editor.form.is_dirty())
            .and_then(|editor| editor.project.as_ref())
            .map(|project| project.slug.clone())
        else {
            return;
        };
        // Show untracked rows if the new one is untracked, so it can be found in the list.
        if projects.rows.iter().any(|row| row.slug == slug && !row.tracked) {
            projects.show_untracked = true;
        }
        projects.filter.update(cx, |input, cx| input.set_value("", window, cx));
        let visible = projects.visible(cx);
        let Some(ix) = visible.iter().position(|&row| projects.rows[row].slug == slug) else {
            return;
        };
        projects.editor = None;
        self.open_project_editor(Some(ix), window, cx);
    }

    pub fn ask_untrack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.open_projects().and_then(|(_, projects)| projects.editor.as_mut()) {
            editor.confirming_untrack = true;
            editor.keep.focus.focus(window, cx);
        }
        cx.notify();
    }

    pub fn keep_tracked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.open_projects().and_then(|(_, projects)| projects.editor.as_mut()) {
            editor.confirming_untrack = false;
            editor.back.focus.focus(window, cx);
        }
        cx.notify();
    }

    pub fn untrack_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.put_project(Some(false), window, cx);
    }

    pub fn track_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.put_project(Some(true), window, cx);
    }

    pub fn reapply_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.open_projects().and_then(|(_, projects)| projects.editor.as_mut()) {
            editor.form.reapply();
        }
        self.put_project(None, window, cx);
    }

    pub fn discard_project_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self.projects_writable();
        if let Some(editor) = self.open_projects().and_then(|(_, projects)| projects.editor.as_mut()) {
            editor.form.discard();
            editor.sync_inputs(window, cx);
            editor.sync_disabled(writable, cx);
            editor.back.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Links the folder typed in the editor to the project: its origin must be the project's repository.
    pub fn link_checkout(&mut self, cx: &mut Context<Self>) {
        let Some(config_dir) = self.config_dir() else {
            return;
        };
        let Some((id, projects)) = self.open_projects() else {
            return;
        };
        let Some(editor) = projects.editor.as_mut() else {
            return;
        };
        let Some(repo) = editor.project.as_ref().map(|project| project.repo.clone()) else {
            return;
        };
        let folder = editor.folder.read(cx).value().trim().to_owned();
        if folder.is_empty() || editor.linking {
            editor.link_result = Some(("Type the checkout's folder first.".to_owned(), true));
            cx.notify();
            return;
        }
        editor.linking = true;
        editor.link_result = None;
        let key = editor.key;
        cx.spawn(async move |this, cx| {
            let folder = PathBuf::from(folder);
            let linked_repo = repo.to_lowercase();
            let result = cx
                .background_executor()
                .spawn(async move { projects::link_checkout(&config_dir, &folder, &repo) })
                .await;
            let _ = this.update(cx, |manage, cx| {
                let Some(projects) = manage.projects_of(id) else {
                    return;
                };
                // The repository that was linked, whichever project the editor shows by now.
                if let (Ok(_), Some(linked)) = (&result, projects.linked.as_mut()) {
                    linked.push(linked_repo);
                }
                if let Some(editor) = projects.editor.as_mut().filter(|editor| editor.key == key) {
                    editor.linking = false;
                    editor.link_result = Some(match result {
                        Ok(path) => (
                            format!("Linked {}. The dashboard adds it to the table.", path.display()),
                            false,
                        ),
                        Err(error) => (error.to_string(), true),
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn projects_default_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, projects)) = self.open_projects() else {
            return;
        };
        if projects.editor.is_none() {
            let selected = projects.selected;
            self.open_project_editor(Some(selected), window, cx);
        }
    }

    pub(super) fn render_projects(
        &self,
        dialog: &Dialog,
        projects: &ProjectsDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let writable = write_state(Feature::Projects, &self.access);
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
        if let Some(editor) = &projects.editor {
            return self.render_project_editor(dialog, projects, editor, writable, close, window, cx);
        }
        let theme = cx.theme().clone();
        let mut body = v_flex().id("projects").test_support().gap_2().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div().flex_1().child(
                        Input::new(&projects.filter)
                            .id("projects-filter")
                            .aria_label("Filter projects"),
                    ),
                )
                .child(chip(
                    "projects-show-untracked",
                    "Show untracked",
                    &projects.untracked_toggle.focus,
                    projects.show_untracked,
                    false,
                    |manage, _, cx| manage.toggle_untracked(cx),
                    cx,
                )),
        );
        if let Err(reason) = &writable {
            body = body.child(read_only_note("projects-read-only", reason, cx));
        }
        body = body.child(
            h_flex()
                .gap_2()
                .px_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().w(rems(10.)).flex_none().child("Slug"))
                .child(div().flex_1().min_w_0().child("Repository"))
                .child(div().w(rems(5.)).flex_none().child("Priority"))
                .child(div().w(rems(6.)).flex_none().child("Tracked"))
                .child(div().w(rems(6.)).flex_none().child("Checkout")),
        );
        let focused = projects.list.is_focused(window);
        let visible = projects.visible(cx);
        let rows = visible
            .iter()
            .enumerate()
            .map(|(ix, &row)| {
                let project = &projects.rows[row];
                let selected = ix == projects.selected;
                let linked = match projects.is_linked(project) {
                    Some(true) => "Linked",
                    Some(false) => "—",
                    None => "…",
                };
                div()
                    .id(SharedString::from(format!("project-row-{ix}")))
                    .test_support()
                    .role(Role::ListBoxOption)
                    .aria_selected(selected)
                    .aria_label(format!(
                        "{}, {}, {}, {}, checkout {}",
                        project.slug,
                        project.repo,
                        project.priority_label(),
                        if project.tracked { "tracked" } else { "untracked" },
                        linked
                    ))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(!project.tracked, |this| this.text_color(theme.muted_foreground))
                    .when(selected, |this| {
                        this.bg(if focused { theme.accent } else { theme.list_active })
                    })
                    .when(!selected, |this| this.hover(|style| style.bg(theme.list_hover)))
                    .on_click(
                        cx.listener(move |manage, _, window, cx| manage.open_project_editor(Some(ix), window, cx)),
                    )
                    .child(div().w(rems(10.)).flex_none().truncate().child(project.slug.clone()))
                    .child(div().flex_1().min_w_0().truncate().child(project.repo.clone()))
                    .child(div().w(rems(5.)).flex_none().child(project.priority_label()))
                    .child(
                        div()
                            .w(rems(6.))
                            .flex_none()
                            .child(if project.tracked { "Tracked" } else { "Untracked" }),
                    )
                    .child(div().w(rems(6.)).flex_none().child(linked))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = if projects.loading && projects.rows.is_empty() {
            Some("Loading…".to_owned())
        } else if visible.is_empty() && projects.load_error.is_none() {
            Some("No projects match.".to_owned())
        } else {
            None
        };
        let ring = theme.ring;
        body = body.child(
            v_flex()
                .id("projects-list")
                .test_support()
                .role(Role::ListBox)
                .aria_label("Projects")
                .key_context(WORK_LIST)
                .track_focus(&projects.list)
                .tab_index(0)
                .h(rems(17.))
                .overflow_y_scroll()
                .track_scroll(&projects.scroll)
                .p_1()
                .rounded(theme.radius)
                .border_2()
                .border_color(theme.border)
                .focus(move |style| style.border_color(ring))
                .on_action(cx.listener(|manage, _: &WorkUp, _, cx| manage.move_in_projects(-1, cx)))
                .on_action(cx.listener(|manage, _: &WorkDown, _, cx| manage.move_in_projects(1, cx)))
                .on_action(cx.listener(|manage, _: &OpenItem, window, cx| manage.projects_default_action(window, cx)))
                .children(rows)
                .when_some(empty, |this, text| {
                    this.child(
                        div()
                            .id("projects-list-empty")
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
                .child("pmd's priority is final; PORTFOLIO.md is regenerated from it. No delete: untrack instead."),
        );
        let mut footer = Vec::new();
        if projects.load_error.is_some() && !projects.loading {
            footer.push(button(
                "projects-retry",
                "Retry",
                &projects.retry.focus,
                false,
                false,
                false,
                |manage, _, cx| {
                    if let Some((id, _)) = manage.open_projects() {
                        manage.load_projects(id, cx);
                    }
                },
                cx,
            ));
        }
        footer.push(close);
        if writable.is_ok() {
            footer.push(button(
                "projects-add",
                "Add project",
                &projects.add.focus,
                true,
                false,
                false,
                |manage, window, cx| manage.open_project_editor(None, window, cx),
                cx,
            ));
        }
        self.render_frame(
            dialog,
            Frame {
                title: "Projects".into(),
                body: body.into_any_element(),
                message: projects.load_error.clone().map(|error| (error, true)),
                footer,
                wide: true,
            },
            window,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_project_editor(
        &self,
        dialog: &Dialog,
        projects: &ProjectsDialog,
        editor: &ProjectEditor,
        writable: Result<(), String>,
        close: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let form = &editor.form;
        let locked = editor.locked(writable.is_ok());
        let creating = editor.project.is_none();
        let title = match &editor.project {
            Some(project) => format!("{} ({})", project.slug, project.repo),
            None => "New project".to_owned(),
        };
        let mut body = v_flex().id("project-editor").test_support().gap_3().child(
            div()
                .id("project-editor-title")
                .test_support()
                .aria_label(title.clone())
                .font_semibold()
                .child(title),
        );
        if let Err(reason) = &writable {
            body = body.child(read_only_note("projects-read-only", reason, cx));
        }
        for field in FIELDS {
            let label = projects::label(field);
            let error = form.field_error(field);
            let control: AnyElement = if let Some((_, input)) = editor.inputs.iter().find(|(name, _)| *name == field) {
                let suggested = projects::slug_for(form.value("repo"));
                v_flex()
                    .gap_1()
                    .child(
                        Input::new(input)
                            .id(SharedString::from(format!("project-field-{field}")))
                            .aria_label(label)
                            .disabled(locked),
                    )
                    .when(
                        field == "slug" && form.value("slug").is_empty() && !suggested.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .id("project-slug-default")
                                    .test_support()
                                    .aria_label(format!("Empty uses {suggested}"))
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("Empty uses {suggested}, the repository's name.")),
                            )
                        },
                    )
                    .into_any_element()
            } else if field == "priority" {
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(
                        PRIORITY_CHOICES
                            .iter()
                            .zip(&editor.priorities)
                            .map(|(priority, control)| {
                                let priority = *priority;
                                chip(
                                    format!(
                                        "project-priority-{}",
                                        if priority.is_empty() { "none" } else { priority }
                                    ),
                                    if priority.is_empty() {
                                        "No tier".to_owned()
                                    } else {
                                        format!("P{priority}")
                                    },
                                    &control.focus,
                                    form.value("priority") == priority,
                                    locked,
                                    move |manage, _, cx| manage.set_project_priority(priority, cx),
                                    cx,
                                )
                            }),
                    )
                    .into_any_element()
            } else if field == "tracked" {
                div()
                    .id("project-tracked")
                    .test_support()
                    .aria_label(if form.value("tracked") == "false" {
                        "Untracked"
                    } else {
                        "Tracked"
                    })
                    .py_1()
                    .text_sm()
                    .child(if form.value("tracked") == "false" {
                        "Untracked: paused, with its data kept."
                    } else {
                        "Tracked"
                    })
                    .into_any_element()
            } else {
                div()
                    .py_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(form.value(field).to_owned())
                    .into_any_element()
            };
            body = body.child(field_row(label, field, control, error, cx));
        }
        // Said before Save; once a save was refused, the field's own error says it.
        if form.value("priority") == "5" && form.value("tracked") != "false" && form.field_error("priority").is_none() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme.warning)
                    .child("P5 is dormant: it must be untracked. Untrack the project, or pick P0 to P4."),
            );
        }
        // Link checkout: only for a stored project no configured checkout matches.
        if let Some(project) = editor.project.as_ref()
            && projects.is_linked(project) == Some(false)
            && self.config_dir().is_some()
        {
            body = body.child(
                v_flex()
                    .id("project-link")
                    .test_support()
                    .gap_1()
                    .p_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .child(div().text_sm().child(format!(
                        "No configured checkout is {}. Link one to show it in the table:",
                        project.repo
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div().flex_1().child(
                                    Input::new(&editor.folder)
                                        .id("project-folder")
                                        .aria_label("Checkout folder"),
                                ),
                            )
                            .child(button(
                                "project-link-checkout",
                                if editor.linking { "Linking…" } else { "Link checkout" },
                                &editor.link.focus,
                                false,
                                false,
                                editor.linking,
                                |manage, _, cx| manage.link_checkout(cx),
                                cx,
                            )),
                    ),
            );
        }
        if let Some((text, error)) = &editor.link_result {
            body = body.child(
                h_flex()
                    .id("project-link-result")
                    .test_support()
                    .aria_label(text.clone())
                    .gap_1()
                    .text_sm()
                    .text_color(if *error { theme.danger } else { theme.success })
                    .child(
                        Icon::new(if *error {
                            IconName::CircleAlert
                        } else {
                            IconName::CircleCheck
                        })
                        .xsmall(),
                    )
                    .child(text.clone()),
            );
        }
        let back = button(
            "projects-back",
            "Back to list",
            &editor.back.focus,
            false,
            false,
            false,
            |manage, window, cx| manage.projects_back_to_list(window, cx),
            cx,
        );
        let footer = if editor.confirming_untrack {
            let slug = editor.project.as_ref().map_or(String::new(), |p| p.slug.clone());
            let text = format!(
                "Untrack {slug}? It stops being worked on; its data and work items are kept, and you can track it again."
            );
            vec![
                h_flex()
                    .id("untrack-confirm")
                    .test_support()
                    .role(Role::AlertDialog)
                    .aria_label(text.clone())
                    .flex_1()
                    .gap_2()
                    .text_sm()
                    .child(Icon::new(IconName::TriangleAlert).text_color(theme.warning))
                    .child(div().flex_1().child(text))
                    .into_any_element(),
                button(
                    "untrack-keep",
                    "Keep tracked",
                    &editor.keep.focus,
                    true,
                    false,
                    false,
                    |manage, window, cx| manage.keep_tracked(window, cx),
                    cx,
                ),
                button(
                    "untrack-confirm-button",
                    "Untrack",
                    &editor.untrack.focus,
                    false,
                    false,
                    false,
                    |manage, window, cx| manage.untrack_project(window, cx),
                    cx,
                ),
            ]
        } else if let Some(conflict) = form.conflict_state() {
            let labels: Vec<(&str, &str)> = FIELDS.iter().map(|f| (*f, projects::label(f))).collect();
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
                    |manage, window, cx| manage.discard_project_edits(window, cx),
                    cx,
                ),
                button(
                    "conflict-reapply",
                    "Reapply mine",
                    &editor.reapply.focus,
                    true,
                    false,
                    false,
                    |manage, window, cx| manage.reapply_project(window, cx),
                    cx,
                ),
            ]
        } else if writable.is_ok() {
            let mut footer = vec![back];
            if let Some(project) = &editor.project {
                footer.push(if project.tracked {
                    // Untrack and Track again write the project as stored: save or discard edits first.
                    button(
                        "project-untrack",
                        "Untrack…",
                        &editor.untrack.focus,
                        false,
                        locked || form.is_dirty(),
                        false,
                        |manage, window, cx| manage.ask_untrack(window, cx),
                        cx,
                    )
                } else {
                    button(
                        "project-track",
                        "Track again",
                        &editor.track.focus,
                        false,
                        locked || form.is_dirty(),
                        false,
                        |manage, window, cx| manage.track_project(window, cx),
                        cx,
                    )
                });
            }
            footer.push(close);
            footer.push(button(
                "project-save",
                if form.is_saving() {
                    "Saving…"
                } else if creating {
                    "Add (Ctrl+Enter)"
                } else {
                    "Save (Ctrl+Enter)"
                },
                &editor.save.focus,
                true,
                !form.is_dirty(),
                form.is_saving(),
                |manage, window, cx| manage.save_project(window, cx),
                cx,
            ));
            footer
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
                title: "Projects".into(),
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
