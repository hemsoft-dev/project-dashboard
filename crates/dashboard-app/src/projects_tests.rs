//! Headless UI tests for the Projects dialog (#34), against an in-memory pmd and a temporary config folder. No desktop
//! window, OS input or network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashboard_core::cache::StatusCache;
use dashboard_core::write::{Precondition, Whoami, WriteBackend, WriteError, WriteRequest};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use crate::board_view::{BoardView, Options};
use crate::demo;
use crate::manage::{Entry, Manage};
use crate::ui_tests::{four, open_board, options, settle};

fn project(slug: &str, priority: Option<u8>, tracked: bool, goal: &str) -> Value {
    json!({"slug": slug, "repo": format!("hemsoft-dev/{slug}"), "priority": priority, "goal": goal, "notes": "",
           "requires": [], "tracked": tracked, "source": "portfolio", "version": 2})
}

/// A believable portfolio: tiers P0 to P4, one without a tier, one untracked P5.
pub(crate) fn sample_projects() -> Vec<Value> {
    vec![
        project("set-it-free-loop", Some(1), true, "Ship the pause control"),
        project("gh-x", Some(3), true, "Maintenance: keep healthy, no new initiatives."),
        project("chief", None, true, "Run the fleet"),
        project("project-dashboard", Some(0), true, "Manage pmd from the dashboard"),
        project("hs-buddy", Some(2), true, "Sign the Windows installer"),
        project("weather-cli", Some(5), false, "Dormant"),
        project("tickdown", Some(4), false, "Paused for now"),
    ]
}

type Change = Box<dyn FnOnce(&mut Vec<Value>) + Send>;

/// An in-memory pmd for projects: the list with `all=1`, single rows, and `PUT` with `If-Match` or `If-None-Match`.
pub(crate) struct FakeProjects {
    scopes: Vec<String>,
    pub rows: Mutex<Vec<Value>>,
    pub writes: Mutex<Vec<WriteRequest>>,
    pub fail_next: Mutex<Option<(WriteError, Option<Change>)>>,
}

impl FakeProjects {
    pub(crate) fn new(scopes: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            rows: Mutex::new(sample_projects()),
            writes: Mutex::default(),
            fail_next: Mutex::default(),
        })
    }

    pub(crate) fn fail_next(&self, error: WriteError, elsewhere: impl FnOnce(&mut Vec<Value>) + Send + 'static) {
        *self.fail_next.lock().unwrap() = Some((error, Some(Box::new(elsewhere))));
    }

    pub(crate) fn row(&self, slug: &str) -> Value {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|row| row["slug"] == slug)
            .unwrap()
            .clone()
    }
}

impl WriteBackend for FakeProjects {
    fn token_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/config/pmd-write-token"))
    }
    fn whoami(&self) -> Result<Whoami, WriteError> {
        Ok(Whoami {
            name: "project-dashboard@home".into(),
            scopes: self.scopes.clone(),
        })
    }
    fn store_token(&self, _: &str) -> Result<(), WriteError> {
        Ok(())
    }
    fn read(&self, path: &str) -> Result<Value, WriteError> {
        let slug = path.strip_prefix("v1/projects/").ok_or(WriteError::NotFound)?;
        let rows = self.rows.lock().unwrap();
        rows.iter()
            .find(|row| row["slug"] == slug)
            .cloned()
            .ok_or(WriteError::NotFound)
    }
    fn list(&self, path: &str, query: &[(String, String)]) -> Result<Value, WriteError> {
        assert_eq!(path, "v1/projects");
        assert_eq!(query, [("all".to_owned(), "1".to_owned())]);
        Ok(json!({"projects": *self.rows.lock().unwrap()}))
    }
    fn write(&self, request: &WriteRequest) -> Result<Value, WriteError> {
        self.writes.lock().unwrap().push(request.clone());
        let mut rows = self.rows.lock().unwrap();
        if let Some((error, elsewhere)) = self.fail_next.lock().unwrap().take() {
            if let Some(change) = elsewhere {
                change(&mut rows);
            }
            return Err(error);
        }
        let slug = request.path.strip_prefix("v1/projects/").unwrap();
        let existing = rows.iter().position(|row| row["slug"] == slug);
        match (request.precondition, existing) {
            (Precondition::IfNoneMatch, Some(_)) => Err(WriteError::AlreadyExists),
            (Precondition::IfNoneMatch, None) => {
                let mut row = request.body.clone();
                row["slug"] = json!(slug);
                row["source"] = json!("api");
                row["version"] = json!(1);
                rows.push(row.clone());
                Ok(row)
            }
            (Precondition::IfMatch(_), None) => Err(WriteError::NotFound),
            (Precondition::IfMatch(version), Some(at)) => {
                let row = &mut rows[at];
                if row["version"] != version {
                    return Err(WriteError::StaleVersion);
                }
                let mut changed = false;
                for (field, value) in request.body.as_object().unwrap() {
                    if row[field] != *value {
                        row[field] = value.clone();
                        changed = true;
                    }
                }
                if changed {
                    row["source"] = json!("api");
                    row["version"] = json!(version + 1);
                }
                Ok(row.clone())
            }
        }
    }
}

/// A temporary folder with a config.toml listing one checkout (gh-x) and other checkouts to link.
pub(crate) struct Folder(pub PathBuf);
impl Folder {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dashboard-projects-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let folder = Self(path);
        let gh_x = folder.checkout("gh-x", "https://github.com/hemsoft-dev/gh-x.git");
        std::fs::write(
            folder.config_dir().join("config.toml"),
            format!(
                "# Franz's dashboard\nrepositories = [\n  {:?},\n]\n",
                gh_x.to_string_lossy()
            ),
        )
        .unwrap();
        folder
    }
    pub(crate) fn config_dir(&self) -> PathBuf {
        let dir = self.0.join("config");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    pub(crate) fn checkout(&self, name: &str, origin: &str) -> PathBuf {
        let checkout = self.0.join(name);
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::write(
            checkout.join(".git/config"),
            format!("[remote \"origin\"]\n\turl = {origin}\n"),
        )
        .unwrap();
        checkout
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_with(
    cx: &mut TestAppContext,
    backend: Arc<FakeProjects>,
    folder: &Folder,
) -> (AnyWindowHandle, Entity<BoardView>, Entity<Manage>) {
    let (handle, view) = open_board(
        cx,
        four(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            write: backend,
            config_path: Some(folder.config_dir().join("config.toml")),
            ..options()
        },
    );
    let manage = cx.update(|cx| view.read(cx).manage().clone());
    (handle, view, manage)
}

fn frame(cx: &mut TestAppContext, handle: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn press(cx: &mut TestAppContext, handle: AnyWindowHandle, key: &str) {
    cx.update_window(handle, |_, window, cx| window.press(key, cx)).unwrap();
    frame(cx, handle);
}

fn click(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &str) {
    let id = gpui_kit::SharedString::from(id.to_owned());
    cx.update_window(handle, |_, window, cx| window.click(id, cx)).unwrap();
    frame(cx, handle);
}

fn exists(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &str) -> bool {
    let id = gpui_kit::SharedString::from(id.to_owned());
    cx.update_window(handle, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

fn focused(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> bool {
    cx.update_window(handle, |_, window, _| window.find(id).focused() == Some(true))
        .unwrap()
}

fn text_of(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &str) -> String {
    let id = gpui_kit::SharedString::from(id.to_owned());
    cx.update_window(handle, |_, window, _| {
        window.find(id).label().unwrap_or_default().to_owned()
    })
    .unwrap()
}

fn set_input(cx: &mut TestAppContext, handle: AnyWindowHandle, manage: &Entity<Manage>, field: &str, text: &str) {
    let input = cx.update(|cx| manage.read(cx).input(field)).unwrap();
    let text = text.to_owned();
    cx.update_window(handle, |_, window, cx| {
        input.update(cx, |input, cx| input.set_value(text, window, cx))
    })
    .unwrap();
    frame(cx, handle);
}

async fn open_dialog(cx: &mut TestAppContext, handle: AnyWindowHandle, view: &Entity<BoardView>) {
    settle(cx, handle, view).await;
    click(cx, handle, "manage-button");
    settle(cx, handle, view).await;
    click(cx, handle, "manage-projects");
    settle(cx, handle, view).await;
    frame(cx, handle);
}

fn visible(cx: &mut TestAppContext, manage: &Entity<Manage>) -> Vec<String> {
    cx.update(|cx| manage.read(cx).projects().unwrap().visible_slugs(cx))
}

fn form_value(cx: &mut TestAppContext, manage: &Entity<Manage>, field: &str) -> String {
    cx.update(|cx| {
        manage
            .read(cx)
            .projects()
            .unwrap()
            .editor_form()
            .unwrap()
            .value(field)
            .to_owned()
    })
}

/// Opens the editor on the visible row for `slug`.
fn edit(cx: &mut TestAppContext, handle: AnyWindowHandle, manage: &Entity<Manage>, slug: &str) {
    let ix = visible(cx, manage).iter().position(|s| s == slug).unwrap();
    click(cx, handle, &format!("project-row-{ix}"));
}

#[gpui_kit::test]
async fn the_list_sorts_by_priority_hides_untracked_and_shows_which_have_a_checkout(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    assert_eq!(
        cx.update(|cx| manage.read(cx).dialog()),
        Some(Entry::Feature(dashboard_core::write::Feature::Projects))
    );
    assert!(focused(cx, handle, "projects-list"));
    assert_eq!(
        visible(cx, &manage),
        ["project-dashboard", "set-it-free-loop", "hs-buddy", "gh-x", "chief"]
    );
    assert_eq!(
        text_of(cx, handle, "project-row-3"),
        "gh-x, hemsoft-dev/gh-x, P3, tracked, checkout Linked"
    );
    assert_eq!(
        text_of(cx, handle, "project-row-4"),
        "chief, hemsoft-dev/chief, —, tracked, checkout —"
    );
    click(cx, handle, "projects-show-untracked");
    assert_eq!(visible(cx, &manage).len(), 7);
    assert!(text_of(cx, handle, "project-row-4").starts_with("tickdown, hemsoft-dev/tickdown, P4, untracked"));
    set_input(cx, handle, &manage, "filter", "PAUSE");
    assert_eq!(
        visible(cx, &manage),
        ["set-it-free-loop", "tickdown"],
        "goals match too"
    );
    // Delete isn't bound: nothing happens, and there is no delete anywhere.
    press(cx, handle, "delete");
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn a_priority_pick_writes_only_on_save_and_the_table_hears_of_it(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    let wrote = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = wrote.clone();
    cx.update(|cx| {
        cx.subscribe(&manage, move |_, _: &crate::manage::ManageEvent, _| {
            seen.set(seen.get() + 1)
        })
        .detach()
    });
    open_dialog(cx, handle, &view).await;
    // Keyboard: Down to gh-x, Enter edits it with focus on its priority.
    for _ in 0..3 {
        press(cx, handle, "down");
    }
    press(cx, handle, "enter");
    assert!(focused(cx, handle, "project-priority-3"));
    click(cx, handle, "project-priority-0");
    assert!(
        backend.writes.lock().unwrap().is_empty(),
        "picking alone writes nothing"
    );
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].path, "v1/projects/gh-x");
    assert_eq!(writes[0].precondition, Precondition::IfMatch(2));
    assert_eq!(
        writes[0].body,
        json!({"repo": "hemsoft-dev/gh-x", "priority": 0, "goal": "Maintenance: keep healthy, no new initiatives.",
               "notes": "", "requires": [], "tracked": true})
    );
    assert_eq!(wrote.get(), 1, "the table reloads its priorities");
    // No tier is null.
    click(cx, handle, "project-priority-none");
    click(cx, handle, "project-save");
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap()[1].body["priority"], Value::Null);
    click(cx, handle, "projects-back");
    assert_eq!(
        visible(cx, &manage).last().map(String::as_str),
        Some("gh-x"),
        "no tier sorts last"
    );
}

#[gpui_kit::test]
async fn p5_must_be_untracked_and_untrack_asks_first(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    edit(cx, handle, &manage, "hs-buddy");
    click(cx, handle, "project-priority-5");
    click(cx, handle, "project-save");
    assert!(text_of(cx, handle, "field-priority-error").starts_with("P5 is dormant"));
    assert!(backend.writes.lock().unwrap().is_empty());
    click(cx, handle, "project-priority-2");
    click(cx, handle, "project-untrack");
    let confirm = text_of(cx, handle, "untrack-confirm");
    assert!(confirm.contains("data and work items are kept"), "{confirm}");
    assert!(confirm.contains("track it again"), "{confirm}");
    assert!(focused(cx, handle, "untrack-keep"));
    click(cx, handle, "untrack-keep");
    assert!(!exists(cx, handle, "untrack-confirm"));
    assert!(backend.writes.lock().unwrap().is_empty());
    click(cx, handle, "project-untrack");
    click(cx, handle, "untrack-confirm-button");
    settle(cx, handle, &view).await;
    assert_eq!(backend.row("hs-buddy")["tracked"], json!(false));
    assert!(exists(cx, handle, "project-track"), "Track again is offered now");
    click(cx, handle, "project-track");
    settle(cx, handle, &view).await;
    assert_eq!(backend.row("hs-buddy")["tracked"], json!(true));
    // Tracking a P5 again is refused before sending.
    click(cx, handle, "projects-back");
    click(cx, handle, "projects-show-untracked");
    edit(cx, handle, &manage, "weather-cli");
    let writes = backend.writes.lock().unwrap().len();
    click(cx, handle, "project-track");
    assert!(text_of(cx, handle, "field-priority-error").starts_with("P5 is dormant"));
    assert_eq!(backend.writes.lock().unwrap().len(), writes);
}

#[gpui_kit::test]
async fn adding_a_project_defaults_its_slug_and_shows_pmds_errors_on_the_field(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "projects-add");
    assert_eq!(text_of(cx, handle, "project-editor-title"), "New project");
    set_input(cx, handle, &manage, "repo", "hemsoft-dev/Yahtzee");
    assert_eq!(text_of(cx, handle, "project-slug-default"), "Empty uses yahtzee");
    click(cx, handle, "project-priority-3");
    set_input(cx, handle, &manage, "goal", "Ship the scoreboard");
    backend.fail_next(
        WriteError::BadRequest {
            code: "unknown_repo".into(),
            message: "hemsoft-dev/Yahtzee doesn't exist".into(),
        },
        |_| {},
    );
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    assert_eq!(
        text_of(cx, handle, "field-repo-error"),
        "hemsoft-dev/Yahtzee doesn't exist"
    );
    assert!(!exists(cx, handle, "dialog-message"), "on the field only");
    assert_eq!(form_value(cx, &manage, "slug"), "yahtzee");
    set_input(cx, handle, &manage, "repo", "hemsoft-dev/yahtzee");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes[1].path, "v1/projects/yahtzee");
    assert_eq!(writes[1].precondition, Precondition::IfNoneMatch);
    assert_eq!(writes[1].body["goal"], "Ship the scoreboard");
    // Created: the editor now edits the stored project, with repo and slug fixed.
    assert_eq!(
        text_of(cx, handle, "project-editor-title"),
        "yahtzee (hemsoft-dev/yahtzee)"
    );
    assert!(!exists(cx, handle, "project-field-repo"));
    // A taken slug says so on the slug.
    click(cx, handle, "projects-back");
    click(cx, handle, "projects-add");
    set_input(cx, handle, &manage, "repo", "hemsoft-dev/gh-x");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    assert_eq!(text_of(cx, handle, "field-slug-error"), "Already exists in pmd.");
}

#[gpui_kit::test]
async fn changed_elsewhere_shows_both_values_and_reapplies_on_the_new_version(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    edit(cx, handle, &manage, "gh-x");
    click(cx, handle, "project-priority-1");
    backend.fail_next(WriteError::StaleVersion, |rows| {
        let row = rows.iter_mut().find(|row| row["slug"] == "gh-x").unwrap();
        row["priority"] = json!(2);
        row["version"] = json!(6);
    });
    click(cx, handle, "project-save");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(text_of(cx, handle, "conflict-priority"), "Priority: yours 1, now 2");
    click(cx, handle, "conflict-reapply");
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap()[1].precondition, Precondition::IfMatch(6));
    assert_eq!(backend.row("gh-x")["priority"], json!(1));
}

fn config_text(folder: &Folder) -> String {
    std::fs::read_to_string(folder.config_dir().join("config.toml")).unwrap()
}

#[gpui_kit::test]
async fn link_checkout_checks_the_origin_and_appends_to_config_toml(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let wrong = folder.checkout("fork", "https://github.com/someone/hs-buddy.git");
    let right = folder.checkout("hs-buddy", "git@github.com:hemsoft-dev/hs-buddy.git");
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    // gh-x has a checkout already: no Link checkout there.
    edit(cx, handle, &manage, "gh-x");
    assert!(!exists(cx, handle, "project-link"));
    click(cx, handle, "projects-back");
    edit(cx, handle, &manage, "hs-buddy");
    assert!(exists(cx, handle, "project-link"));
    let before = config_text(&folder);
    set_input(cx, handle, &manage, "folder", &wrong.to_string_lossy());
    click(cx, handle, "project-link-checkout");
    settle(cx, handle, &view).await;
    assert_eq!(
        text_of(cx, handle, "project-link-result"),
        "That folder's origin is someone/hs-buddy; nothing was written."
    );
    assert_eq!(config_text(&folder), before);
    set_input(cx, handle, &manage, "folder", &right.to_string_lossy());
    click(cx, handle, "project-link-checkout");
    settle(cx, handle, &view).await;
    assert!(text_of(cx, handle, "project-link-result").starts_with("Linked "));
    let after = config_text(&folder);
    assert!(after.starts_with("# Franz's dashboard\n"), "comments stay: {after}");
    // However the platform quotes the path (Windows paths get a literal string), it reads back as the folder.
    let config = dashboard_core::config::Config::parse(&after, std::path::Path::new("config.toml")).unwrap();
    assert_eq!(config.repositories.last(), Some(&right), "{after}");
    click(cx, handle, "projects-back");
    let ix = visible(cx, &manage).iter().position(|s| s == "hs-buddy").unwrap();
    assert!(text_of(cx, handle, &format!("project-row-{ix}")).ends_with("checkout Linked"));
    assert!(
        backend.writes.lock().unwrap().is_empty(),
        "linking writes config.toml, not pmd"
    );
}

#[gpui_kit::test]
async fn without_write_projects_the_list_opens_read_only_with_the_reason(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:work"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    assert_eq!(visible(cx, &manage).len(), 5, "the list still loads");
    assert_eq!(
        text_of(cx, handle, "projects-read-only"),
        "Read-only. The token lacks write:projects."
    );
    assert!(!exists(cx, handle, "projects-add"));
    press(cx, handle, "enter");
    assert!(exists(cx, handle, "project-editor"));
    assert!(!exists(cx, handle, "project-save"));
    assert!(!exists(cx, handle, "project-untrack"));
    click(cx, handle, "project-priority-0");
    assert_eq!(form_value(cx, &manage, "priority"), "0", "the first row is P0 already");
    press(cx, handle, "ctrl-enter");
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn unsaved_edits_ask_before_leaving(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    edit(cx, handle, &manage, "chief");
    set_input(cx, handle, &manage, "notes", "Runs on mini");
    click(cx, handle, "projects-back");
    assert!(exists(cx, handle, "unsaved-confirm"));
    click(cx, handle, "confirm-discard");
    assert!(exists(cx, handle, "projects-list"));
    edit(cx, handle, &manage, "chief");
    set_input(cx, handle, &manage, "notes", "Runs on mini");
    press(cx, handle, "escape");
    click(cx, handle, "confirm-discard");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn untrack_waits_until_edits_are_saved_or_discarded(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    edit(cx, handle, &manage, "hs-buddy");
    set_input(cx, handle, &manage, "notes", "unsaved");
    click(cx, handle, "project-untrack");
    assert!(
        !exists(cx, handle, "untrack-confirm"),
        "disabled while edits are unsaved"
    );
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| m.untrack_project(window, cx))
    })
    .unwrap();
    settle(cx, handle, &view).await;
    assert!(
        backend.writes.lock().unwrap().is_empty(),
        "nothing written with the edits"
    );
}

#[gpui_kit::test]
async fn a_create_that_timed_out_but_landed_becomes_an_edit_without_a_second_create(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "projects-add");
    set_input(cx, handle, &manage, "repo", "hemsoft-dev/yahtzee");
    click(cx, handle, "project-priority-3");
    backend.fail_next(
        WriteError::Timeout {
            create: Some(dashboard_core::write::CreateCheck::Exists),
        },
        |rows| rows.push(project("yahtzee", Some(3), true, "")),
    );
    click(cx, handle, "project-save");
    settle(cx, handle, &view).await;
    assert_eq!(
        text_of(cx, handle, "project-editor-title"),
        "yahtzee (hemsoft-dev/yahtzee)"
    );
    assert!(
        !exists(cx, handle, "project-field-repo"),
        "the stored project is edited now"
    );
    press(cx, handle, "ctrl-enter");
    assert_eq!(backend.writes.lock().unwrap().len(), 1, "never a second create");
    click(cx, handle, "projects-back");
    assert!(visible(cx, &manage).contains(&"yahtzee".to_owned()));
}

#[gpui_kit::test]
async fn fixing_the_repository_settles_a_slug_mismatch(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "projects-add");
    set_input(cx, handle, &manage, "slug", "foo");
    set_input(cx, handle, &manage, "repo", "hemsoft-dev/bar");
    click(cx, handle, "project-save");
    assert!(text_of(cx, handle, "field-slug-error").starts_with("The slug is the repository's name"));
    set_input(cx, handle, &manage, "repo", "hemsoft-dev/foo");
    click(cx, handle, "project-save");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1, "the save goes through once the slug matches");
    assert_eq!(writes[0].path, "v1/projects/foo");
}

#[gpui_kit::test]
async fn a_priority_changed_elsewhere_moves_the_row_after_the_conflict(cx: &mut TestAppContext) {
    let folder = Folder::new();
    let backend = FakeProjects::new(&["read", "write:projects"]);
    let (handle, view, manage) = open_with(cx, backend.clone(), &folder);
    open_dialog(cx, handle, &view).await;
    edit(cx, handle, &manage, "chief");
    set_input(cx, handle, &manage, "notes", "mine");
    backend.fail_next(WriteError::StaleVersion, |rows| {
        let row = rows.iter_mut().find(|row| row["slug"] == "chief").unwrap();
        row["priority"] = json!(0);
        row["version"] = json!(7);
    });
    click(cx, handle, "project-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "conflict-discard");
    click(cx, handle, "projects-back");
    let order = visible(cx, &manage);
    assert_eq!(
        &order[..2],
        ["chief", "project-dashboard"],
        "P0 chief sorts first now: {order:?}"
    );
}
