//! Headless UI tests for the Work items dialog (#35), against an in-memory pmd. No desktop window, OS input or network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Lists in pmd's order (effective priority, rank, then newest) instead of the order the items were given in, so
    /// a priority or rank save moves the item.
    pub in_pmd_order: AtomicBool,
    /// The next listing of a later page (one with a cursor) fails with this.
    pub fail_next_page: Mutex<Option<WriteError>>,
    /// Later pages answer with the cursor they were asked for, so the listing never moves on.
    pub stuck_cursor: AtomicBool,
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
            in_pmd_order: AtomicBool::new(false),
            fail_next_page: Mutex::default(),
            stuck_cursor: AtomicBool::new(false),
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
        let paged = query.iter().any(|(key, _)| key == "cursor");
        if paged && let Some(error) = self.fail_next_page.lock().unwrap().take() {
            return Err(error);
        }
        let get = |key: &str| query.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let items = self.items.lock().unwrap();
        let mut matching: Vec<Value> = items
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
        if self.in_pmd_order.load(Ordering::SeqCst) {
            matching.sort_by(|a, b| {
                let key = |item: &Value| (item["priority"].as_u64(), item["rank"].as_i64());
                key(a)
                    .cmp(&key(b))
                    .then_with(|| b["ghUpdatedAt"].as_str().cmp(&a["ghUpdatedAt"].as_str()))
            });
        }
        let limit: usize = get("limit").unwrap().parse().unwrap();
        let start: usize = get("cursor").map_or(0, |cursor| cursor.parse().unwrap());
        let page: Vec<Value> = matching.iter().skip(start).take(limit).cloned().collect();
        let next = if start + limit < matching.len() {
            if paged && self.stuck_cursor.load(Ordering::SeqCst) {
                start.to_string()
            } else {
                (start + limit).to_string()
            }
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

#[gpui_kit::test]
async fn an_unknown_priority_shows_as_a_dash(cx: &mut TestAppContext) {
    let mut list = sample_items();
    list[0]["priority"] = json!(6);
    let backend = FakeWork::new(&["read", "write:work"], list);
    let (handle, view, _) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert!(text_of(cx, handle, "work-row-0").contains("Ready · auto, —, rank 0"));
}

#[gpui_kit::test]
async fn an_item_that_closed_before_the_conflict_reread_shows_as_closed(cx: &mut TestAppContext) {
    let backend = FakeWork::new(&["read", "write:work"], sample_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-blocked");
    backend.fail_next(WriteError::StaleVersion, |items| {
        items[0]["status"] = json!("done");
        items[0]["ghState"] = json!("merged");
        items[0]["version"] = json!(8);
    });
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(cx.update(|cx| manage.read(cx).work_items().unwrap().is_closed_item()));
    assert!(!exists(cx, handle, "conflict"));
    assert!(!exists(cx, handle, "conflict-reapply"));
    assert!(exists(cx, handle, "work-closed"));
}

/// A pmd listing in its own order, over `items`.
fn in_pmd_order(items: Vec<Value>) -> Arc<FakeWork> {
    let backend = FakeWork::new(&["read", "write:work"], items);
    backend.in_pmd_order.store(true, Ordering::SeqCst);
    backend
}

/// Three open gh-x items, #200 to #202, all P2 at rank 0.
fn three_items() -> Vec<Value> {
    (0..3).map(|ix| work_item(ix, "gh-x", "issue", "An item")).collect()
}

fn numbers(cx: &mut TestAppContext, manage: &Entity<Manage>) -> Vec<u64> {
    items(cx, manage).iter().map(|item| item.number).collect()
}

fn selected(cx: &mut TestAppContext, manage: &Entity<Manage>) -> usize {
    cx.update(|cx| manage.read(cx).work_items().unwrap().selected())
}

fn editing(cx: &mut TestAppContext, manage: &Entity<Manage>) -> Option<u64> {
    cx.update(|cx| {
        manage
            .read(cx)
            .work_items()
            .unwrap()
            .editor_item()
            .map(|item| item.number)
    })
}

/// The test's one write went to the item it edited, at the version it showed.
fn only_write_is_to(backend: &FakeWork, number: u64) {
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(writes[0].path, format!("v1/work/01JBXWORK{:04}", number - 200));
    assert_eq!(writes[0].precondition, Precondition::IfMatch(3));
}

#[gpui_kit::test]
async fn a_priority_save_that_moves_the_item_keeps_it_selected_after_back(cx: &mut TestAppContext) {
    let backend = in_pmd_order(three_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    press(cx, handle, "down");
    press(cx, handle, "down");
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(202));
    click(cx, handle, "work-priority-0");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 202);
    assert_eq!(numbers(cx, &manage), [202, 200, 201], "P0 sorts #202 first");
    click(cx, handle, "work-back");
    assert!(focused(cx, handle, "work-list"));
    assert_eq!(selected(cx, &manage), 0, "the highlight moved with #202");
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(202), "Enter reopens the item just saved");
}

#[gpui_kit::test]
async fn a_rank_save_that_moves_the_item_down_keeps_it_selected_after_back(cx: &mut TestAppContext) {
    let mut items = three_items();
    items[2]["rank"] = json!(10);
    let backend = in_pmd_order(items);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(200));
    set_input(cx, handle, &manage, "rank", "5");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 200);
    assert_eq!(
        numbers(cx, &manage),
        [201, 200, 202],
        "rank 5 sorts #200 between 0 and 10"
    );
    click(cx, handle, "work-back");
    assert_eq!(selected(cx, &manage), 1);
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(200));
}

#[gpui_kit::test]
async fn a_save_that_keeps_the_order_keeps_the_same_row_selected(cx: &mut TestAppContext) {
    let backend = in_pmd_order(three_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-row-2");
    click(cx, handle, "work-status-blocked");
    set_input(cx, handle, &manage, "statusReason", "waiting on review");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 202);
    assert_eq!(numbers(cx, &manage), [200, 201, 202]);
    click(cx, handle, "work-back");
    assert_eq!(selected(cx, &manage), 2);
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(202));
}

#[gpui_kit::test]
async fn a_moved_item_stays_selected_when_the_reload_brings_back_only_the_first_page(cx: &mut TestAppContext) {
    let many = (0..62).map(|ix| work_item(ix, "gh-x", "issue", "An item")).collect();
    let backend = in_pmd_order(many);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-load-more");
    settle(cx, handle, &view).await;
    assert_eq!(items(cx, &manage).len(), 62);
    // Load more is gone with the last page, and its focus with it. Back from an editor focuses the list again; the
    // keyboard then reaches row 55, scrolling as it goes.
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-back");
    assert!(focused(cx, handle, "work-list"));
    for _ in 0..55 {
        press(cx, handle, "down");
    }
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(255));
    click(cx, handle, "work-priority-0");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].path, "v1/work/01JBXWORK0055");
    assert_eq!(
        items(cx, &manage).len(),
        50,
        "the reload starts again from the first page"
    );
    click(cx, handle, "work-back");
    assert_eq!(selected(cx, &manage), 0);
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(255));
}

#[gpui_kit::test]
async fn an_item_that_leaves_the_status_filter_hands_the_selection_to_a_listed_row(cx: &mut TestAppContext) {
    let backend = in_pmd_order(three_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-filter-status-ready");
    settle(cx, handle, &view).await;
    click(cx, handle, "work-row-1");
    click(cx, handle, "work-status-blocked");
    set_input(cx, handle, &manage, "statusReason", "waiting on review");
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 201);
    assert_eq!(numbers(cx, &manage), [200, 202], "#201 is blocked now, not ready");
    click(cx, handle, "work-back");
    assert_eq!(selected(cx, &manage), 1, "the same position, on a listed row");
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(202));
}

#[gpui_kit::test]
async fn when_the_last_item_leaves_the_list_enter_opens_nothing(cx: &mut TestAppContext) {
    let mut items = three_items();
    items[0]["status"] = json!("blocked");
    items[1]["status"] = json!("blocked");
    let backend = in_pmd_order(items);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-filter-status-ready");
    settle(cx, handle, &view).await;
    assert_eq!(numbers(cx, &manage), [202]);
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-status-blocked");
    set_input(cx, handle, &manage, "statusReason", "waiting on review");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 202);
    click(cx, handle, "work-back");
    assert!(exists(cx, handle, "work-list-empty"));
    assert_eq!(selected(cx, &manage), 0);
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), None, "no row, no editor");
    assert_eq!(backend.writes.lock().unwrap().len(), 1, "and nothing else is written");
}

#[gpui_kit::test]
async fn a_filter_change_selects_the_first_row_even_when_the_selected_item_is_still_listed(cx: &mut TestAppContext) {
    let backend = in_pmd_order(three_items());
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    press(cx, handle, "down");
    press(cx, handle, "down");
    assert_eq!(selected(cx, &manage), 2);
    click(cx, handle, "work-filter-kind-issue");
    settle(cx, handle, &view).await;
    assert_eq!(numbers(cx, &manage), [200, 201, 202], "#202 is still listed");
    assert_eq!(selected(cx, &manage), 0);
    assert!(backend.writes.lock().unwrap().is_empty());
}

/// `count` open gh-x items, #200 on, all P2, with `rank` setting each one's rank.
fn ranked(count: usize, rank: impl Fn(usize) -> i64) -> Vec<Value> {
    (0..count)
        .map(|ix| {
            let mut item = work_item(ix, "gh-x", "issue", "An item");
            item["rank"] = json!(rank(ix));
            item
        })
        .collect()
}

/// Loads every page, then the keyboard opens row `row`: Back from an editor focuses the list, then Down reaches it.
async fn open_deep_row(
    cx: &mut TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<BoardView>,
    manage: &Entity<Manage>,
    row: usize,
) {
    while cx.update(|cx| manage.read(cx).work_items().unwrap().has_more()) {
        click(cx, handle, "work-load-more");
        settle(cx, handle, view).await;
    }
    click(cx, handle, "work-row-0");
    click(cx, handle, "work-back");
    assert!(focused(cx, handle, "work-list"));
    for _ in 0..row {
        press(cx, handle, "down");
    }
    press(cx, handle, "enter");
}

fn lists(backend: &FakeWork) -> usize {
    backend.lists.lock().unwrap().len()
}

/// The cursors of the listings made since `from`: `None` for a first page.
fn cursors_since(backend: &FakeWork, from: usize) -> Vec<Option<String>> {
    backend.lists.lock().unwrap()[from..]
        .iter()
        .map(|query| query.iter().find(|(k, _)| k == "cursor").map(|(_, v)| v.clone()))
        .collect()
}

#[gpui_kit::test]
async fn a_save_that_keeps_the_item_past_the_first_page_keeps_it_selected(cx: &mut TestAppContext) {
    let backend = in_pmd_order(ranked(62, |_| 0));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 55).await;
    assert_eq!(editing(cx, &manage), Some(255));
    set_input(cx, handle, &manage, "rank", "5");
    let before = lists(&backend);
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 255);
    click(cx, handle, "work-back");
    assert!(focused(cx, handle, "work-list"));
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(255), "Enter reopens the item just saved");
    click(cx, handle, "work-back");
    assert_eq!(selected(cx, &manage), 61, "the highlight stayed on #255");
    assert_eq!(numbers(cx, &manage)[61], 255, "rank 5 sorts #255 last");
    assert_eq!(items(cx, &manage).len(), 62);
    assert_eq!(
        cursors_since(&backend, before),
        [None, Some("50".to_owned())],
        "the reload reads on to the page #255 is on now, and no further"
    );
}

#[gpui_kit::test]
async fn a_save_that_moves_the_item_from_the_third_page_to_the_second_keeps_it_selected(cx: &mut TestAppContext) {
    // Ranks 0, 10 and 20 fill the three pages; rank 5 puts the item first on the second page.
    let backend = in_pmd_order(ranked(130, |ix| (ix / 50) as i64 * 10));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 110).await;
    assert_eq!(editing(cx, &manage), Some(310));
    set_input(cx, handle, &manage, "rank", "5");
    let before = lists(&backend);
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 310);
    click(cx, handle, "work-back");
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(310));
    assert_eq!(selected(cx, &manage), 50);
    assert_eq!(numbers(cx, &manage)[50], 310);
    assert_eq!(items(cx, &manage).len(), 100, "two pages, up to the one with #310");
    assert_eq!(cursors_since(&backend, before), [None, Some("50".to_owned())]);
}

#[gpui_kit::test]
async fn a_deep_item_that_leaves_the_status_filter_hands_the_selection_to_the_row_in_its_place(
    cx: &mut TestAppContext,
) {
    let backend = in_pmd_order(ranked(62, |_| 0));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "work-filter-status-ready");
    settle(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 55).await;
    assert_eq!(editing(cx, &manage), Some(255));
    click(cx, handle, "work-status-blocked");
    set_input(cx, handle, &manage, "statusReason", "waiting on review");
    let before = lists(&backend);
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 255);
    click(cx, handle, "work-back");
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(256), "the row in #255's old place");
    assert_eq!(selected(cx, &manage), 55, "the same position, on a listed row");
    assert_eq!(items(cx, &manage).len(), 61);
    assert!(!numbers(cx, &manage).contains(&255));
    assert_eq!(
        cursors_since(&backend, before),
        [None, Some("50".to_owned())],
        "read to the end: only then is #255 known to be gone"
    );
}

#[gpui_kit::test]
async fn a_later_page_that_fails_keeps_the_rows_and_retry_finds_the_item(cx: &mut TestAppContext) {
    let backend = in_pmd_order(ranked(62, |_| 0));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 55).await;
    set_input(cx, handle, &manage, "rank", "5");
    *backend.fail_next_page.lock().unwrap() = Some(WriteError::Network);
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 255);
    click(cx, handle, "work-back");
    // The rows on screen stay, as the last good data, with #255 still selected and nothing more to load from them.
    assert_eq!(items(cx, &manage).len(), 62);
    assert_eq!(selected(cx, &manage), 55);
    assert_eq!(numbers(cx, &manage)[55], 255);
    assert!(exists(cx, handle, "work-retry"));
    assert!(!exists(cx, handle, "work-load-more"));
    click(cx, handle, "work-retry");
    settle(cx, handle, &view).await;
    assert_eq!(items(cx, &manage).len(), 62);
    assert!(!exists(cx, handle, "work-retry"));
    assert_eq!(selected(cx, &manage), 61);
    assert_eq!(
        numbers(cx, &manage)[61],
        255,
        "Retry's reload found #255 on the second page"
    );
    assert_eq!(
        backend.writes.lock().unwrap().len(),
        1,
        "a failed read never repeats the write"
    );
}

#[gpui_kit::test]
async fn a_listing_that_does_not_move_on_ends_the_reload(cx: &mut TestAppContext) {
    let backend = in_pmd_order(ranked(130, |_| 0));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 55).await;
    set_input(cx, handle, &manage, "rank", "5");
    backend.stuck_cursor.store(true, Ordering::SeqCst);
    let before = lists(&backend);
    click(cx, handle, "work-save");
    settle(cx, handle, &view).await;
    only_write_is_to(&backend, 255);
    assert_eq!(
        cursors_since(&backend, before),
        [None, Some("50".to_owned())],
        "a page that hands back its own cursor ends the reload"
    );
    assert_eq!(items(cx, &manage).len(), 100);
    click(cx, handle, "work-back");
    assert_eq!(selected(cx, &manage), 55, "a listed row");
    press(cx, handle, "enter");
    assert_eq!(editing(cx, &manage), Some(256));
}

/// Runs one task at a time until `done`, so a test can act between a request and its answer.
fn step_until(cx: &mut TestAppContext, mut done: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if done() {
            return;
        }
        if !cx.executor().tick() {
            break;
        }
    }
    assert!(done(), "the awaited request never came");
}

/// Clicks or presses without running the tasks that starts.
fn act_only(cx: &mut TestAppContext, handle: AnyWindowHandle, click: Option<&str>, key: Option<&str>) {
    let click = click.map(|id| gpui_kit::SharedString::from(id.to_owned()));
    cx.update_window(handle, |_, window, cx| {
        if let Some(id) = click {
            window.click(id, cx);
        }
        if let Some(key) = key {
            window.press(key, cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn a_filter_change_while_a_reload_reads_on_drops_its_late_page(cx: &mut TestAppContext) {
    let backend = in_pmd_order(ranked(62, |_| 0));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 55).await;
    set_input(cx, handle, &manage, "rank", "5");
    let before = lists(&backend);
    act_only(cx, handle, Some("work-save"), None);
    // Stop once the reload has asked for its second page, before that page lands.
    step_until(cx, || lists(&backend) == before + 2);
    act_only(cx, handle, Some("work-back"), None);
    act_only(cx, handle, Some("work-filter-kind-issue"), None);
    settle(cx, handle, &view).await;
    frame(cx, handle);
    only_write_is_to(&backend, 255);
    assert_eq!(
        cursors_since(&backend, before),
        [None, Some("50".to_owned()), None],
        "the new filters list from their first page and the old reload reads no further"
    );
    assert!(has(&last_query(&backend), "kind", "issue"));
    assert_eq!(
        items(cx, &manage).len(),
        50,
        "the new listing's first page, not the late one mixed in"
    );
    assert_eq!(selected(cx, &manage), 0, "new filters start at the first row");
    assert!(cx.update(|cx| manage.read(cx).work_items().unwrap().has_more()));
}

#[gpui_kit::test]
async fn closing_the_dialog_while_a_reload_reads_on_drops_its_late_page(cx: &mut TestAppContext) {
    let backend = in_pmd_order(ranked(62, |_| 0));
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    open_deep_row(cx, handle, &view, &manage, 55).await;
    set_input(cx, handle, &manage, "rank", "5");
    let before = lists(&backend);
    act_only(cx, handle, Some("work-save"), None);
    step_until(cx, || lists(&backend) == before + 2);
    act_only(cx, handle, None, Some("escape"));
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
    only_write_is_to(&backend, 255);
    assert_eq!(lists(&backend), before + 2, "nothing more is read for a closed dialog");
}
