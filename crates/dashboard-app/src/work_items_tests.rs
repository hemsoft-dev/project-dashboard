//! Headless UI tests for the Work items dialog (#35), against an in-memory pmd. No desktop window, OS input or network.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dashboard_core::cache::StatusCache;
use dashboard_core::work::WorkItem;
use dashboard_core::write::{Precondition, Whoami, WriteBackend, WriteError, WriteRequest};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use crate::board_view::{BoardView, Options};
use crate::demo;
use crate::manage::{Entry, Manage};
use crate::ui_tests::{four, open_board, options, settle};

/// One open item as pmd lists it, at the project's priority (P2) unless it has its own.
pub(crate) fn work_item(ix: usize, project: &str, kind: &str, title: &str) -> Value {
    json!({
        "id": format!("01JBXWORK{ix:04}"), "project": project, "kind": kind, "number": 200 + ix, "title": title,
        "url": "", "ghState": "open", "labels": [], "author": "franz", "status": "ready", "statusSource": "auto",
        "statusReason": null, "priority": 2, "priorityOverride": null, "rank": 0, "version": 3, "links": [],
        "assignment": null, "ghUpdatedAt": "2026-10-27T05:00:00Z"
    })
}

/// A believable list for screenshots: a few projects, kinds and statuses.
pub(crate) fn sample_items() -> Vec<Value> {
    let mut items = vec![
        work_item(0, "gh-x", "issue", "Show the tray icon on Linux"),
        work_item(1, "project-dashboard", "issue", "Add a Work items dialog"),
        work_item(2, "codexbar", "pr", "chore(deps): bump the cargo group"),
        work_item(3, "hs-buddy", "issue", "Sign the Windows installer"),
        work_item(4, "chief", "issue", "Move dispatch onto pmd priorities"),
        work_item(5, "set-it-free-loop", "pr", "feat: pause a loop from the tray"),
    ];
    items[1]["status"] = json!("in_progress");
    items[3]["status"] = json!("blocked");
    items[3]["statusSource"] = json!("manual");
    items[3]["statusReason"] = json!("needs the code-signing certificate");
    items[3]["priority"] = json!(0);
    items[3]["priorityOverride"] = json!(0);
    items[4]["status"] = json!("waiting_on_franz");
    items[4]["statusSource"] = json!("manual");
    items[5]["rank"] = json!(-10);
    items
}

/// An in-memory pmd for the Work items dialog: lists with filters and paging, reads, and patches with `If-Match`.
pub(crate) struct FakeWork {
    scopes: Vec<String>,
    pub items: Mutex<Vec<Value>>,
    pub lists: Mutex<Vec<Vec<(String, String)>>>,
    pub writes: Mutex<Vec<WriteRequest>>,
    /// The next write fails with this, after an optional change to the items made "elsewhere".
    #[allow(clippy::type_complexity)]
    pub fail_next: Mutex<Option<(WriteError, Option<Box<dyn FnOnce(&mut Vec<Value>) + Send>>)>>,
    /// Listings fail while this is set.
    pub fail_lists: Mutex<Option<WriteError>>,
}

impl FakeWork {
    pub(crate) fn new(scopes: &[&str], items: Vec<Value>) -> Arc<Self> {
        Arc::new(Self {
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            items: Mutex::new(items),
            lists: Mutex::default(),
            writes: Mutex::default(),
            fail_next: Mutex::default(),
            fail_lists: Mutex::default(),
        })
    }

    pub(crate) fn fail_next(&self, error: WriteError, elsewhere: impl FnOnce(&mut Vec<Value>) + Send + 'static) {
        *self.fail_next.lock().unwrap() = Some((error, Some(Box::new(elsewhere))));
    }

    fn find(items: &mut [Value], path: &str) -> Option<usize> {
        let id = path.strip_prefix("v1/work/")?;
        items.iter().position(|item| item["id"] == id)
    }
}

impl WriteBackend for FakeWork {
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
        let mut items = self.items.lock().unwrap();
        let ix = Self::find(&mut items, path).ok_or(WriteError::NotFound)?;
        Ok(items[ix].clone())
    }
    fn list(&self, path: &str, query: &[(String, String)]) -> Result<Value, WriteError> {
        assert_eq!(path, "v1/work");
        self.lists.lock().unwrap().push(query.to_vec());
        if let Some(error) = self.fail_lists.lock().unwrap().clone() {
            return Err(error);
        }
        let get = |key: &str| query.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let items = self.items.lock().unwrap();
        let matching: Vec<Value> = items
            .iter()
            .filter(|item| item["ghState"] == "open")
            .filter(|item| get("status").is_none_or(|status| item["status"] == status.as_str()))
            .filter(|item| get("kind").is_none_or(|kind| item["kind"] == kind.as_str()))
            .filter(|item| get("project").is_none_or(|projects| projects.split(',').any(|p| item["project"] == p)))
            .filter(|item| {
                get("maxPriority")
                    .is_none_or(|max| item["priority"].as_u64().is_some_and(|p| p <= max.parse().unwrap()))
            })
            .cloned()
            .collect();
        let limit: usize = get("limit").unwrap().parse().unwrap();
        let start: usize = get("cursor").map_or(0, |cursor| cursor.parse().unwrap());
        let page: Vec<Value> = matching.iter().skip(start).take(limit).cloned().collect();
        let next = if start + limit < matching.len() {
            (start + limit).to_string()
        } else {
            String::new()
        };
        Ok(json!({"items": page, "nextCursor": next}))
    }
    fn write(&self, request: &WriteRequest) -> Result<Value, WriteError> {
        self.writes.lock().unwrap().push(request.clone());
        let mut items = self.items.lock().unwrap();
        if let Some((error, elsewhere)) = self.fail_next.lock().unwrap().take() {
            if let Some(change) = elsewhere {
                change(&mut items);
            }
            return Err(error);
        }
        let ix = Self::find(&mut items, &request.path).ok_or(WriteError::NotFound)?;
        let item = &mut items[ix];
        let Precondition::IfMatch(version) = request.precondition else {
            return Err(WriteError::PreconditionRequired);
        };
        if item["version"] != version {
            return Err(WriteError::StaleVersion);
        }
        let body = request.body.as_object().unwrap();
        if let Some(status) = body.get("status") {
            if status == "auto" {
                item["status"] = json!("ready");
                item["statusSource"] = json!("auto");
            } else {
                item["status"] = status.clone();
                item["statusSource"] = json!("manual");
            }
            item["statusReason"] = body.get("statusReason").cloned().unwrap_or(Value::Null);
        } else if let Some(reason) = body.get("statusReason") {
            item["statusReason"] = reason.clone();
        }
        if let Some(priority) = body.get("priority") {
            item["priorityOverride"] = priority.clone();
            item["priority"] = if priority.is_null() { json!(2) } else { priority.clone() };
        }
        if let Some(rank) = body.get("rank") {
            item["rank"] = rank.clone();
        }
        item["version"] = json!(item["version"].as_u64().unwrap() + 1);
        Ok(item.clone())
    }
}

fn open_with(cx: &mut TestAppContext, backend: Arc<FakeWork>) -> (AnyWindowHandle, Entity<BoardView>, Entity<Manage>) {
    let (handle, view) = open_board(
        cx,
        four(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            write: backend,
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
    let id = id.to_owned();
    cx.update_window(handle, |_, window, cx| {
        window.click(gpui_kit::SharedString::from(id), cx)
    })
    .unwrap();
    frame(cx, handle);
}

fn exists(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> bool {
    cx.update_window(handle, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

fn focused(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> bool {
    cx.update_window(handle, |_, window, _| window.find(id).focused() == Some(true))
        .unwrap()
}

fn text_of(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> String {
    cx.update_window(handle, |_, window, _| {
        window.find(id).label().unwrap_or_default().to_owned()
    })
    .unwrap()
}

/// Replaces an input's text, as typing would.
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
    click(cx, handle, "manage-work-items");
    settle(cx, handle, view).await;
    frame(cx, handle);
}

fn items(cx: &mut TestAppContext, manage: &Entity<Manage>) -> Vec<WorkItem> {
    cx.update(|cx| manage.read(cx).work_items().unwrap().items().to_vec())
}

fn form_value(cx: &mut TestAppContext, manage: &Entity<Manage>, field: &str) -> String {
    cx.update(|cx| {
        manage
            .read(cx)
            .work_items()
            .unwrap()
            .editor_form()
            .unwrap()
            .value(field)
            .to_owned()
    })
}

fn last_query(backend: &FakeWork) -> Vec<(String, String)> {
    backend.lists.lock().unwrap().last().unwrap().clone()
}

fn has(query: &[(String, String)], key: &str, value: &str) -> bool {
    query.iter().any(|(k, v)| k == key && v == value)
}

#[gpui_kit::test]
async fn the_list_shows_open_work_and_each_filter_reloads_it_from_the_first_page(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert_eq!(
        cx.update(|cx| manage.read(cx).dialog()),
        Some(Entry::Feature(dashboard_core::write::Feature::WorkItems))
    );
    assert_eq!(items(cx, &manage).len(), 6);
    assert!(focused(cx, handle, "work-list"), "focus starts on the list");
    let blocked = text_of(cx, handle, "work-row-3");
    assert_eq!(
        blocked,
        "hs-buddy #203 Sign the Windows installer, Issue, Blocked · manual, P0 · own, rank 0"
    );
    assert!(text_of(cx, handle, "work-row-0").contains("Ready · auto, P2, rank 0"));

    click(cx, handle, "work-filter-status-blocked");
    settle(cx, handle, &view).await;
    assert!(has(&last_query(&backend), "status", "blocked"));
    assert!(!last_query(&backend).iter().any(|(k, _)| k == "cursor"));
    assert_eq!(items(cx, &manage).len(), 1);

    click(cx, handle, "work-filter-status-open");
    click(cx, handle, "work-filter-kind-pr");
    settle(cx, handle, &view).await;
    assert!(has(&last_query(&backend), "kind", "pr"));
    assert_eq!(items(cx, &manage).len(), 2);

    click(cx, handle, "work-filter-kind-all");
    click(cx, handle, "work-filter-priority-0");
    settle(cx, handle, &view).await;
    assert!(has(&last_query(&backend), "maxPriority", "0"));
    assert_eq!(items(cx, &manage).len(), 1);

    click(cx, handle, "work-filter-priority-any");
    set_input(cx, handle, &manage, "project", "gh-x, chief");
    settle(cx, handle, &view).await;
    assert!(has(&last_query(&backend), "project", "gh-x,chief"));
    assert_eq!(items(cx, &manage).len(), 2);

    set_input(cx, handle, &manage, "project", "nothing-here");
    settle(cx, handle, &view).await;
    assert_eq!(
        text_of(cx, handle, "work-list-empty"),
        "No open work matches these filters."
    );
}

#[gpui_kit::test]
async fn load_more_follows_the_next_cursor(cx: &mut TestAppContext) {
    let many = (0..62).map(|ix| work_item(ix, "gh-x", "issue", "An item")).collect();
    let backend = FakeWork::new(&["read", "write:work"], many);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert_eq!(items(cx, &manage).len(), 50);
    assert!(cx.update(|cx| manage.read(cx).work_items().unwrap().has_more()));
    assert_eq!(text_of(cx, handle, "work-count"), "50 items, more to load");
    click(cx, handle, "work-load-more");
    settle(cx, handle, &view).await;
    assert!(has(&last_query(&backend), "cursor", "50"));
    assert_eq!(items(cx, &manage).len(), 62);
    assert!(!cx.update(|cx| manage.read(cx).work_items().unwrap().has_more()));
    assert!(!exists(cx, handle, "work-load-more"));
}

#[gpui_kit::test]
async fn the_keyboard_moves_through_the_list_opens_the_editor_and_saves(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    press(cx, handle, "down");
    press(cx, handle, "down");
    press(cx, handle, "up");
    assert_eq!(cx.update(|cx| manage.read(cx).work_items().unwrap().selected()), 1);
    press(cx, handle, "enter");
    let item = cx.update(|cx| manage.read(cx).work_items().unwrap().editor_item().cloned().unwrap());
    assert_eq!(item.number, 201);
    assert!(
        focused(cx, handle, "work-status-auto"),
        "focus starts on the current status"
    );
    // Tab moves to the next status; Enter picks it.
    press(cx, handle, "tab");
    assert!(focused(cx, handle, "work-status-ready") || focused(cx, handle, "work-reason"));
    cx.update_window(handle, |_, window, cx| window.click("work-status-blocked", cx))
        .unwrap();
    frame(cx, handle);
    set_input(cx, handle, &manage, "statusReason", "waiting on review");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].body,
        json!({"status": "blocked", "statusReason": "waiting on review"})
    );
    assert_eq!(writes[0].precondition, Precondition::IfMatch(3));
    // The row updates from pmd's answer.
    assert_eq!(items(cx, &manage)[1].status, "blocked");
    assert_eq!(items(cx, &manage)[1].version, 4);
    // Back to the list, then Esc closes the clean dialog.
    click(cx, handle, "work-back");
    assert!(focused(cx, handle, "work-list"));
    press(cx, handle, "escape");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
}

#[gpui_kit::test]
async fn priority_and_rank_save_only_what_changed_and_reload_the_list(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    // Nothing changed yet: Save does nothing.
    press(cx, handle, "ctrl-enter");
    assert!(backend.writes.lock().unwrap().is_empty());
    click(cx, handle, "work-priority-0");
    set_input(cx, handle, &manage, "rank", "-5");
    let lists = backend.lists.lock().unwrap().len();
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes[0].body, json!({"priority": 0, "rank": -5}));
    assert!(
        backend.lists.lock().unwrap().len() > lists,
        "a rank or priority change reloads the list"
    );
    assert!(
        !last_query(&backend).iter().any(|(k, _)| k == "cursor"),
        "from the first page"
    );
    // Inherit the project's priority again.
    click(cx, handle, "work-priority-inherit");
    assert_eq!(form_value(cx, &manage, "priority"), "");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap()[1].body, json!({"priority": null}));
    assert!(text_of(cx, handle, "work-editor-now").contains("P2"));
}

#[gpui_kit::test]
async fn back_to_automatic_disables_the_reason_and_says_it_will_clear(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-3");
    assert_eq!(
        form_value(cx, &manage, "statusReason"),
        "needs the code-signing certificate"
    );
    assert!(!exists(cx, handle, "work-reason-note"));
    assert_eq!(text_of(cx, handle, "work-reason-count"), "34 of 500 characters");
    click(cx, handle, "work-status-auto");
    assert_eq!(
        text_of(cx, handle, "work-reason-note"),
        "Back to automatic clears the current reason."
    );
    let reason = cx.update(|cx| manage.read(cx).input("statusReason")).unwrap();
    assert!(cx.update(|cx| !reason.read(cx).is_editable()));
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap()[0].body, json!({"status": "auto"}));
    assert_eq!(items(cx, &manage)[3].status_reason, None);
}

#[gpui_kit::test]
async fn field_problems_show_on_the_field_and_nothing_is_sent(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-3");
    set_input(cx, handle, &manage, "statusReason", &"x".repeat(501));
    assert_eq!(text_of(cx, handle, "work-reason-count"), "501 of 500 characters");
    set_input(cx, handle, &manage, "rank", "first");
    press(cx, handle, "ctrl-enter");
    assert_eq!(
        text_of(cx, handle, "field-statusReason-error"),
        "At most 500 characters."
    );
    assert_eq!(
        text_of(cx, handle, "field-rank-error"),
        "Rank is a whole number from -1000000000 to 1000000000."
    );
    assert!(backend.writes.lock().unwrap().is_empty());
    // pmd's own 400 naming a field lands there too.
    set_input(cx, handle, &manage, "statusReason", "ok");
    set_input(cx, handle, &manage, "rank", "3");
    backend.fail_next(
        WriteError::BadRequest {
            code: "bad_request".into(),
            message: "rank must be between -1000000000 and 1000000000".into(),
        },
        |_| {},
    );
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    assert!(text_of(cx, handle, "field-rank-error").starts_with("rank must be between"));
}

#[gpui_kit::test]
async fn changed_elsewhere_shows_both_values_then_reapplies_on_the_new_version(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-blocked");
    set_input(cx, handle, &manage, "statusReason", "mine");
    backend.fail_next(WriteError::StaleVersion, |items| {
        items[0]["status"] = json!("in_progress");
        items[0]["statusSource"] = json!("manual");
        items[0]["version"] = json!(9);
    });
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(exists(cx, handle, "conflict"));
    assert_eq!(
        text_of(cx, handle, "conflict-status"),
        "Status: yours Blocked, now In progress"
    );
    assert!(focused(cx, handle, "conflict-reapply"));
    click(cx, handle, "conflict-reapply");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[1].precondition, Precondition::IfMatch(9), "on the new version");
    assert_eq!(items(cx, &manage)[0].status, "blocked");
}

#[gpui_kit::test]
async fn changed_elsewhere_can_discard_mine(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    set_input(cx, handle, &manage, "rank", "4");
    backend.fail_next(WriteError::StaleVersion, |items| {
        items[0]["rank"] = json!(8);
        items[0]["version"] = json!(5);
    });
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "conflict-discard");
    assert_eq!(form_value(cx, &manage, "rank"), "8");
    assert!(!exists(cx, handle, "conflict"));
    let dirty = cx.update(|cx| manage.read(cx).work_items().unwrap().is_dirty());
    assert!(!dirty);
}

#[gpui_kit::test]
async fn an_item_closed_on_github_explains_and_leaves_nothing_to_edit(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-blocked");
    backend.fail_next(WriteError::ItemClosed, |items| {
        items[0]["status"] = json!("done");
        items[0]["ghState"] = json!("closed");
        items[0]["version"] = json!(7);
    });
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(cx.update(|cx| manage.read(cx).work_items().unwrap().is_closed_item()));
    assert!(text_of(cx, handle, "work-closed").starts_with("This item closed on GitHub."));
    assert!(!exists(cx, handle, "work-status-blocked"));
    assert!(!exists(cx, handle, "work-save"));
    assert!(focused(cx, handle, "work-back"));
    // Nothing is unsaved: Esc closes at once.
    press(cx, handle, "escape");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
}

#[gpui_kit::test]
async fn a_428_is_shown_as_the_dashboards_bug_and_not_retried(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, _) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-ready");
    backend.fail_next(WriteError::PreconditionRequired, |_| {});
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    assert!(text_of(cx, handle, "dialog-message").starts_with("A dashboard bug: pmd says the save carried no version"));
    assert_eq!(backend.writes.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
async fn without_write_work_the_dialog_is_read_only_and_names_the_scope(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:fleet"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert_eq!(items(cx, &manage).len(), 6, "the list still loads");
    assert_eq!(
        text_of(cx, handle, "work-read-only"),
        "Read-only. The token lacks write:work."
    );
    press(cx, handle, "enter");
    assert!(exists(cx, handle, "work-editor"));
    assert!(exists(cx, handle, "work-read-only"));
    assert!(!exists(cx, handle, "work-save"));
    click(cx, handle, "work-status-blocked");
    assert_eq!(
        form_value(cx, &manage, "status"),
        "auto",
        "a disabled choice does nothing"
    );
    press(cx, handle, "ctrl-enter");
    assert!(backend.writes.lock().unwrap().is_empty());
    let rank = cx.update(|cx| manage.read(cx).input("rank")).unwrap();
    assert!(cx.update(|cx| !rank.read(cx).is_editable()));
}

#[gpui_kit::test]
async fn unsaved_edits_ask_before_back_or_escape_discards_them(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-priority-1");
    click(cx, handle, "work-back");
    assert!(exists(cx, handle, "unsaved-confirm"));
    assert!(focused(cx, handle, "confirm-keep"));
    click(cx, handle, "confirm-keep");
    assert!(exists(cx, handle, "work-editor"));
    press(cx, handle, "escape");
    assert!(exists(cx, handle, "unsaved-confirm"));
    click(cx, handle, "confirm-discard");
    assert_eq!(
        cx.update(|cx| manage.read(cx).dialog()),
        None,
        "Esc then Discard closes"
    );
    // Back then Discard returns to the list instead.
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-work-items");
    settle(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-priority-1");
    click(cx, handle, "work-back");
    click(cx, handle, "confirm-discard");
    assert!(exists(cx, handle, "work-list"));
    assert!(cx.update(|cx| manage.read(cx).dialog()).is_some());
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn back_to_automatic_drops_a_reason_error_so_the_save_goes_through(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-3");
    set_input(cx, handle, &manage, "statusReason", &"x".repeat(501));
    press(cx, handle, "ctrl-enter");
    assert!(exists(cx, handle, "field-statusReason-error"));
    click(cx, handle, "work-status-auto");
    assert!(!exists(cx, handle, "field-statusReason-error"));
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap()[0].body, json!({"status": "auto"}));
}

#[gpui_kit::test]
async fn a_failed_reload_drops_the_old_listing_and_retries_from_the_first_page(cx: &mut TestAppContext) {
    let many = (0..62).map(|ix| work_item(ix, "gh-x", "issue", "An item")).collect();
    let backend = FakeWork::new(&["read", "write:work"], many);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert!(exists(cx, handle, "work-load-more"));
    *backend.fail_lists.lock().unwrap() = Some(WriteError::Network);
    click(cx, handle, "work-filter-kind-pr");
    settle(cx, handle, &view).await;
    assert!(items(cx, &manage).is_empty(), "no rows from the old filters");
    assert!(!exists(cx, handle, "work-load-more"), "no cursor from the old listing");
    assert_eq!(text_of(cx, handle, "dialog-message"), "pmd is unavailable.");
    *backend.fail_lists.lock().unwrap() = None;
    click(cx, handle, "work-retry");
    settle(cx, handle, &view).await;
    assert!(has(&last_query(&backend), "kind", "pr"));
    assert!(!last_query(&backend).iter().any(|(k, _)| k == "cursor"));
    assert!(!exists(cx, handle, "work-retry"));
}

#[gpui_kit::test]
async fn a_save_that_lands_after_the_editor_or_dialog_closed_still_reaches_the_list_and_the_board(
    cx: &mut TestAppContext,
) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    let wrote = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = wrote.clone();
    cx.update(|cx| {
        cx.subscribe(&manage, move |_, _: &crate::manage::ManageEvent, _| {
            seen.set(seen.get() + 1)
        })
        .detach()
    });
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-blocked");
    // Save, then leave the editor before pmd answers.
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| {
            m.save_work_item(window, cx);
            m.leave_work_editor(window, cx);
        })
    })
    .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(wrote.get(), 1);
    assert_eq!(items(cx, &manage)[0].status, "blocked", "the list shows pmd's answer");
    // Save, then close the whole dialog before pmd answers.
    click(cx, handle, "work-row-1");
    click(cx, handle, "work-status-ready");
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| {
            m.save_work_item(window, cx);
            m.close_dialog(window, cx);
        })
    })
    .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(wrote.get(), 2, "the Board still reloads");
    assert_eq!(backend.writes.lock().unwrap().len(), 2);
}

#[gpui_kit::test]
async fn a_status_change_under_a_status_filter_reloads_the_list(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-filter-status-blocked");
    settle(cx, handle, &view).await;
    assert_eq!(items(cx, &manage).len(), 1);
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-ready");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "work-back");
    assert!(items(cx, &manage).is_empty(), "the item left the Blocked filter");
    assert!(has(&last_query(&backend), "status", "blocked"));
    // Without a status filter, a status change keeps the list as it is.
    click(cx, handle, "work-filter-status-open");
    settle(cx, handle, &view).await;
    let lists = backend.lists.lock().unwrap().len();
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-blocked");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    assert_eq!(backend.lists.lock().unwrap().len(), lists);
}

#[gpui_kit::test]
async fn discarding_mine_after_a_conflict_leaves_the_list_on_pmds_row(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-priority-1");
    backend.fail_next(WriteError::StaleVersion, |items| {
        items[0]["rank"] = json!(-50);
        items[0]["version"] = json!(9);
    });
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "conflict-discard");
    click(cx, handle, "work-back");
    settle(cx, handle, &view).await;
    let row = items(cx, &manage).into_iter().find(|item| item.number == 200).unwrap();
    assert_eq!(
        (row.rank, row.version),
        (-50, 9),
        "the list shows pmd's row, not the old one"
    );
    // Reopening it edits the current version: no second, avoidable conflict.
    let ix = items(cx, &manage).iter().position(|item| item.number == 200).unwrap();
    click(cx, handle, &format!("work-row-{ix}"));
    click(cx, handle, "work-priority-1");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    assert_eq!(
        backend.writes.lock().unwrap().last().unwrap().precondition,
        Precondition::IfMatch(9)
    );
    assert!(!exists(cx, handle, "conflict"));
}

#[gpui_kit::test]
async fn a_new_write_token_path_rechecks_access_for_the_open_dialog(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-priority-1");
    // config.toml changed: a new backend, and access starts over.
    let replacement = FakeWork::new(&["read", "write:work"], sample_items());
    cx.update(|cx| manage.update(cx, |m, cx| m.set_backend(replacement.clone(), None, cx)));
    settle(cx, handle, &view).await;
    assert!(!exists(cx, handle, "work-read-only"), "access is known again");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    assert_eq!(replacement.writes.lock().unwrap().len(), 1, "the unsaved edit saves");
}

#[gpui_kit::test]
async fn a_failed_reload_under_the_same_filters_keeps_the_last_good_rows(cx: &mut TestAppContext) {
    let many = (0..62).map(|ix| work_item(ix, "gh-x", "issue", "An item")).collect();
    let backend = FakeWork::new(&["read", "write:work"], many);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    set_input(cx, handle, &manage, "rank", "-7");
    // The save lands; the reload it starts fails.
    *backend.fail_lists.lock().unwrap() = Some(WriteError::Network);
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "work-back");
    assert_eq!(items(cx, &manage).len(), 50, "the last good rows stay");
    assert_eq!(items(cx, &manage)[0].rank, -7, "with the saved row");
    assert_eq!(text_of(cx, handle, "dialog-message"), "pmd is unavailable.");
    assert!(!exists(cx, handle, "work-load-more"), "no cursor until a reload works");
    *backend.fail_lists.lock().unwrap() = None;
    click(cx, handle, "work-retry");
    settle(cx, handle, &view).await;
    assert!(exists(cx, handle, "work-load-more"));
}
