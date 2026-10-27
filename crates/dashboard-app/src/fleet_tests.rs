//! Headless UI tests for the Fleet dialog (#36), against an in-memory pmd. No desktop window, OS input or network.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dashboard_core::cache::StatusCache;
use dashboard_core::fleet::Kind;
use dashboard_core::write::{CreateCheck, Precondition, Whoami, WriteBackend, WriteError, WriteRequest};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use crate::board_view::{BoardView, Options};
use crate::demo;
use crate::manage::{Entry, Manage};
use crate::ui_tests::{four, open_board, options, settle};

/// The fleet as pmd's live API returned it (#36): four machines, three subscriptions and a few agents.
pub(crate) fn sample_fleet() -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let machine = |name: &str, os: &str, ip: &str, sessions: u64, caps: &[&str], source: &str| {
        json!({"name": name, "os": os, "tailnetIp": ip, "maxSessions": sessions, "capabilities": caps,
               "source": source, "version": 1})
    };
    let machines = vec![
        machine(
            "air",
            "macos",
            "100.101.12.4",
            2,
            &["apple", "browser", "computer_use"],
            "config",
        ),
        machine("home", "windows", "100.88.20.7", 2, &["browser", "windows"], "config"),
        machine("laptop", "windows", "100.70.3.9", 1, &["windows"], "api"),
        machine("mini", "linux", "100.97.164.73", 3, &["browser", "linux"], "config"),
    ];
    let subscription = |id: &str, provider: &str, plan: &str, metered: bool| {
        json!({"id": id, "provider": provider, "plan": plan, "metered": metered, "usageSource": null,
               "source": "config", "version": 1})
    };
    let subscriptions = vec![
        subscription("chatgpt-pro", "openai", "ChatGPT Pro 100", false),
        subscription("claude-max", "anthropic", "Max 20x", false),
        subscription("gemini-api", "google", "", true),
    ];
    let agent = |id: &str, model: &str, sub: &str, roles: &[&str], enabled: bool| {
        let parts: Vec<&str> = id.split('/').collect();
        json!({"id": id, "machine": parts[0], "provider": parts[1], "model": model, "subscription": sub,
               "roles": roles, "enabled": enabled, "source": "config", "version": 1})
    };
    let agents = vec![
        agent(
            "air/claude/opus-5.5",
            "claude-opus-5-5",
            "claude-max",
            &["implement", "review"],
            true,
        ),
        agent(
            "home/codex/gpt-6.1-sol",
            "gpt-6.1-sol",
            "chatgpt-pro",
            &["implement"],
            true,
        ),
        agent(
            "mini/codex/gpt-6.1-sol",
            "gpt-6.1-sol",
            "chatgpt-pro",
            &["implement", "review"],
            true,
        ),
        agent(
            "mini/gemini/flash",
            "gemini-3-flash",
            "gemini-api",
            &["research"],
            false,
        ),
    ];
    (machines, subscriptions, agents)
}

type Change = Box<dyn FnOnce(&mut [Vec<Value>; 3]) + Send>;

/// An in-memory pmd fleet: lists, rows, and `PUT` with `If-Match` or `If-None-Match`.
pub(crate) struct FakeFleet {
    scopes: Vec<String>,
    pub rows: Mutex<[Vec<Value>; 3]>,
    pub writes: Mutex<Vec<WriteRequest>>,
    pub fail_next: Mutex<Option<(WriteError, Option<Change>)>>,
    /// Lists fail while this is set.
    pub fail_lists: Mutex<Option<WriteError>>,
    /// The next list of this kind (0, 1, 2) answers with these rows, an older snapshot.
    pub stale_list: Mutex<Option<(usize, Vec<Value>)>>,
}

fn kind_of(path: &str) -> Option<(usize, &str, &str)> {
    for (ix, (prefix, key)) in [("v1/machines", "name"), ("v1/subscriptions", "id"), ("v1/agents", "id")]
        .into_iter()
        .enumerate()
    {
        if path == prefix {
            return Some((ix, "", key));
        }
        if let Some(rest) = path.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('/')) {
            return Some((ix, rest, key));
        }
    }
    None
}

impl FakeFleet {
    pub(crate) fn new(scopes: &[&str]) -> Arc<Self> {
        let (machines, subscriptions, agents) = sample_fleet();
        Arc::new(Self {
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            rows: Mutex::new([machines, subscriptions, agents]),
            writes: Mutex::default(),
            fail_next: Mutex::default(),
            fail_lists: Mutex::default(),
            stale_list: Mutex::default(),
        })
    }

    pub(crate) fn fail_next(&self, error: WriteError, elsewhere: impl FnOnce(&mut [Vec<Value>; 3]) + Send + 'static) {
        *self.fail_next.lock().unwrap() = Some((error, Some(Box::new(elsewhere))));
    }
}

impl WriteBackend for FakeFleet {
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
        let (ix, key, key_name) = kind_of(path).ok_or(WriteError::NotFound)?;
        let rows = self.rows.lock().unwrap();
        if key.is_empty() {
            if let Some(error) = self.fail_lists.lock().unwrap().clone() {
                return Err(error);
            }
            let mut stale = self.stale_list.lock().unwrap();
            if stale.as_ref().is_some_and(|(kind, _)| *kind == ix) {
                let (_, old) = stale.take().unwrap();
                let list = ["machines", "subscriptions", "agents"][ix];
                return Ok(json!({ list: old }));
            }
            let list = ["machines", "subscriptions", "agents"][ix];
            return Ok(json!({ list: rows[ix] }));
        }
        rows[ix]
            .iter()
            .find(|row| row[key_name] == key)
            .cloned()
            .ok_or(WriteError::NotFound)
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
        let (ix, key, key_name) = kind_of(&request.path).ok_or(WriteError::NotFound)?;
        let existing = rows[ix].iter().position(|row| row[key_name] == key);
        match (request.precondition, existing) {
            (Precondition::IfNoneMatch, Some(_)) => Err(WriteError::AlreadyExists),
            (Precondition::IfNoneMatch, None) => {
                let mut row = request.body.clone();
                row[key_name] = json!(key);
                if ix == 2 {
                    let parts: Vec<&str> = key.split('/').collect();
                    row["machine"] = json!(parts[0]);
                    row["provider"] = json!(parts[1]);
                }
                row["source"] = json!("api");
                row["version"] = json!(1);
                rows[ix].push(row.clone());
                Ok(row)
            }
            (Precondition::IfMatch(_), None) => Err(WriteError::NotFound),
            (Precondition::IfMatch(version), Some(at)) => {
                let row = &mut rows[ix][at];
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

fn open_with(cx: &mut TestAppContext, backend: Arc<FakeFleet>) -> (AnyWindowHandle, Entity<BoardView>, Entity<Manage>) {
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
    click(cx, handle, "manage-fleet");
    settle(cx, handle, view).await;
    frame(cx, handle);
}

fn tab(cx: &mut TestAppContext, manage: &Entity<Manage>) -> Kind {
    cx.update(|cx| manage.read(cx).fleet().unwrap().tab())
}

fn value_of(backend: &FakeFleet, ix: usize, key: &str, field: &str) -> Value {
    let rows = backend.rows.lock().unwrap();
    let key_name = if ix == 0 { "name" } else { "id" };
    rows[ix].iter().find(|row| row[key_name] == key).unwrap()[field].clone()
}

#[gpui_kit::test]
async fn each_tab_lists_its_rows_with_their_source_and_left_right_switch_tabs(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert_eq!(
        cx.update(|cx| manage.read(cx).dialog()),
        Some(Entry::Feature(dashboard_core::write::Feature::Fleet))
    );
    assert!(focused(cx, handle, "fleet-tabs"));
    assert_eq!(tab(cx, &manage), Kind::Machine);
    assert_eq!(
        text_of(cx, handle, "fleet-row-3"),
        "mini, linux, 100.97.164.73, 3, browser, linux, source config"
    );
    press(cx, handle, "right");
    assert_eq!(tab(cx, &manage), Kind::Subscription);
    assert_eq!(
        text_of(cx, handle, "fleet-row-2"),
        "gemini-api, google, —, Yes, source config"
    );
    press(cx, handle, "right");
    assert_eq!(tab(cx, &manage), Kind::Agent);
    assert_eq!(
        text_of(cx, handle, "fleet-row-3"),
        "mini/gemini/flash, gemini-3-flash, gemini-api, research, No, source config"
    );
    press(cx, handle, "right");
    assert_eq!(tab(cx, &manage), Kind::Machine, "wraps around");
    press(cx, handle, "left");
    assert_eq!(tab(cx, &manage), Kind::Agent);
    assert_eq!(text_of(cx, handle, "fleet-tab-agent"), "Agents");
    // No delete anywhere.
    assert!(!exists(cx, handle, "fleet-delete"));
}

#[gpui_kit::test]
async fn max_sessions_saves_the_whole_machine_with_if_match_and_says_it_becomes_api_owned(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    press(cx, handle, "tab");
    assert!(focused(cx, handle, "fleet-list"));
    for _ in 0..3 {
        press(cx, handle, "down");
    }
    press(cx, handle, "enter");
    assert_eq!(text_of(cx, handle, "fleet-editor-title"), "Machine mini");
    assert!(text_of(cx, handle, "fleet-config-note").starts_with("pmd's config file owns this machine."));
    // Nothing changed: Save is off and Ctrl+Enter sends nothing.
    press(cx, handle, "ctrl-enter");
    assert!(backend.writes.lock().unwrap().is_empty());
    set_input(cx, handle, &manage, "maxSessions", "2");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].path, "v1/machines/mini");
    assert_eq!(writes[0].precondition, Precondition::IfMatch(1));
    assert_eq!(
        writes[0].body,
        json!({"os": "linux", "tailnetIp": "100.97.164.73", "maxSessions": 2, "capabilities": ["browser", "linux"]})
    );
    assert_eq!(value_of(&backend, 0, "mini", "source"), json!("api"));
    assert!(!exists(cx, handle, "fleet-config-note"), "it is API-owned now");
    click(cx, handle, "fleet-back");
    assert!(text_of(cx, handle, "fleet-row-3").contains(", 2, browser, linux, source api"));
}

#[gpui_kit::test]
async fn an_out_of_range_max_sessions_is_caught_before_sending(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-row-0");
    set_input(cx, handle, &manage, "maxSessions", "65");
    set_input(cx, handle, &manage, "capabilities", "apple, Browser");
    click(cx, handle, "fleet-save");
    assert_eq!(
        text_of(cx, handle, "field-maxSessions-error"),
        "A whole number from 0 to 64."
    );
    assert!(text_of(cx, handle, "field-capabilities-error").starts_with("\"Browser\""));
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn the_agent_toggle_disables_and_enables_with_the_whole_row(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, _) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-agent");
    assert_eq!(text_of(cx, handle, "fleet-toggle-2"), "Disable");
    click(cx, handle, "fleet-toggle-2");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes[0].path, "v1/agents/mini/codex/gpt-6.1-sol");
    assert_eq!(writes[0].precondition, Precondition::IfMatch(1));
    assert_eq!(
        writes[0].body,
        json!({"model": "gpt-6.1-sol", "subscription": "chatgpt-pro", "roles": ["implement", "review"], "enabled": false})
    );
    assert_eq!(text_of(cx, handle, "fleet-toggle-2"), "Enable");
    click(cx, handle, "fleet-toggle-2");
    settle(cx, handle, &view).await;
    assert_eq!(value_of(&backend, 2, "mini/codex/gpt-6.1-sol", "enabled"), json!(true));
    // A toggle that loses a race reloads the agents and says so.
    backend.fail_next(WriteError::StaleVersion, |_| {});
    click(cx, handle, "fleet-toggle-0");
    settle(cx, handle, &view).await;
    assert!(text_of(cx, handle, "dialog-message").contains("changed elsewhere; reloaded it"));
}

#[gpui_kit::test]
async fn adding_an_agent_builds_its_id_creates_it_and_then_fixes_the_key(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-agent");
    click(cx, handle, "fleet-add");
    assert_eq!(text_of(cx, handle, "fleet-editor-title"), "New agent");
    click(cx, handle, "fleet-machine-laptop");
    set_input(cx, handle, &manage, "provider", "claude");
    set_input(cx, handle, &manage, "alias", "sonnet-5.5");
    set_input(cx, handle, &manage, "model", "claude-sonnet-5-5");
    click(cx, handle, "fleet-subscription-claude-max");
    click(cx, handle, "fleet-suggest-roles-review");
    assert_eq!(text_of(cx, handle, "fleet-agent-id"), "Id: laptop/claude/sonnet-5.5");
    // pmd's 400 unknown_subscription lands on the picker.
    backend.fail_next(
        WriteError::BadRequest {
            code: "unknown_subscription".into(),
            message: "agent uses subscription \"claude-max\", which pmd does not have".into(),
        },
        |_| {},
    );
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    assert!(text_of(cx, handle, "field-subscription-error").contains("which pmd does not have"));
    // The error stays until the field changes; picking the subscription again clears it.
    press(cx, handle, "ctrl-enter");
    assert_eq!(
        backend.writes.lock().unwrap().len(),
        1,
        "not resent while the field is wrong"
    );
    click(cx, handle, "fleet-subscription-claude-max");
    press(cx, handle, "ctrl-enter");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes[1].path, "v1/agents/laptop/claude/sonnet-5.5");
    assert_eq!(writes[1].precondition, Precondition::IfNoneMatch);
    assert_eq!(
        writes[1].body,
        json!({"model": "claude-sonnet-5-5", "subscription": "claude-max", "roles": ["review"], "enabled": true})
    );
    // Created: the editor now edits the stored row, with its id read-only.
    assert_eq!(
        text_of(cx, handle, "fleet-editor-title"),
        "Agent laptop/claude/sonnet-5.5"
    );
    assert!(!exists(cx, handle, "fleet-field-alias"));
    assert_eq!(
        cx.update(|cx| manage.read(cx).fleet().unwrap().rows(Kind::Agent).len()),
        5
    );
}

#[gpui_kit::test]
async fn a_new_row_with_a_taken_key_says_so_on_the_key(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-add");
    set_input(cx, handle, &manage, "name", "mini");
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    assert_eq!(text_of(cx, handle, "field-name-error"), "Already exists in pmd.");
}

#[gpui_kit::test]
async fn a_create_that_timed_out_but_landed_reloads_instead_of_retrying(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-subscription");
    click(cx, handle, "fleet-add");
    set_input(cx, handle, &manage, "id", "grok-heavy");
    set_input(cx, handle, &manage, "provider", "xai");
    backend.fail_next(
        WriteError::Timeout {
            create: Some(CreateCheck::Exists),
        },
        |rows| {
            rows[1].push(
                json!({"id": "grok-heavy", "provider": "xai", "plan": "", "metered": false,
                                "usageSource": null, "source": "api", "version": 1}),
            );
        },
    );
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    assert_eq!(
        cx.update(|cx| manage.read(cx).fleet().unwrap().rows(Kind::Subscription).len()),
        4
    );
    // The stored row is read back: the editor edits it now, so there is no second create to send.
    assert_eq!(text_of(cx, handle, "fleet-editor-title"), "Subscription grok-heavy");
    assert!(!exists(cx, handle, "fleet-field-id"));
    assert_eq!(text_of(cx, handle, "fleet-save"), "Save (Ctrl+Enter)");
    press(cx, handle, "ctrl-enter");
    assert_eq!(backend.writes.lock().unwrap().len(), 1, "not resent");
}

#[gpui_kit::test]
async fn changed_elsewhere_shows_both_values_and_reapplies_on_the_new_version(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-row-1");
    set_input(cx, handle, &manage, "maxSessions", "4");
    backend.fail_next(WriteError::StaleVersion, |rows| {
        rows[0][1]["maxSessions"] = json!(1);
        rows[0][1]["version"] = json!(5);
    });
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(
        text_of(cx, handle, "conflict-maxSessions"),
        "Max sessions: yours 4, now 1"
    );
    click(cx, handle, "conflict-reapply");
    settle(cx, handle, &view).await;
    let writes = backend.writes.lock().unwrap().clone();
    assert_eq!(writes[1].precondition, Precondition::IfMatch(5));
    assert_eq!(value_of(&backend, 0, "home", "maxSessions"), json!(4));
}

#[gpui_kit::test]
async fn without_write_fleet_the_dialog_is_read_only_and_names_the_scope(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:work"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert_eq!(
        text_of(cx, handle, "fleet-read-only"),
        "Read-only. The token lacks write:fleet."
    );
    assert!(!exists(cx, handle, "fleet-add"));
    click(cx, handle, "fleet-tab-agent");
    click(cx, handle, "fleet-toggle-0");
    settle(cx, handle, &view).await;
    click(cx, handle, "fleet-row-0");
    assert!(exists(cx, handle, "fleet-read-only"));
    assert!(!exists(cx, handle, "fleet-save"));
    click(cx, handle, "fleet-enabled-false");
    assert_eq!(
        cx.update(|cx| manage
            .read(cx)
            .fleet()
            .unwrap()
            .editor_form()
            .unwrap()
            .value("enabled")
            .to_owned()),
        "true"
    );
    press(cx, handle, "ctrl-enter");
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn unsaved_edits_ask_before_leaving(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-row-0");
    click(cx, handle, "fleet-os-linux");
    click(cx, handle, "fleet-back");
    assert!(exists(cx, handle, "unsaved-confirm"));
    click(cx, handle, "confirm-discard");
    assert!(exists(cx, handle, "fleet-list"), "back to the list");
    click(cx, handle, "fleet-row-0");
    click(cx, handle, "fleet-os-linux");
    press(cx, handle, "escape");
    click(cx, handle, "confirm-discard");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
    assert!(backend.writes.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn a_save_that_lands_after_the_editor_closed_still_reaches_the_list(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-row-2");
    set_input(cx, handle, &manage, "maxSessions", "0");
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| {
            m.save_fleet_row(window, cx);
            m.leave_fleet_editor(window, cx);
        })
    })
    .unwrap();
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(text_of(cx, handle, "fleet-row-2").contains("laptop, windows, 100.70.3.9, 0,"));
}

#[gpui_kit::test]
async fn a_second_toggle_click_waits_for_the_first(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-agent");
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| {
            m.toggle_agent(1, window, cx);
            m.toggle_agent(1, window, cx);
        })
    })
    .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap().len(), 1);
    assert_eq!(value_of(&backend, 2, "home/codex/gpt-6.1-sol", "enabled"), json!(false));
}

#[gpui_kit::test]
async fn a_list_that_failed_to_load_offers_retry(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    *backend.fail_lists.lock().unwrap() = Some(WriteError::Network);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    assert_eq!(text_of(cx, handle, "dialog-message"), "pmd is unavailable.");
    *backend.fail_lists.lock().unwrap() = None;
    click(cx, handle, "fleet-retry");
    settle(cx, handle, &view).await;
    assert_eq!(
        cx.update(|cx| manage.read(cx).fleet().unwrap().rows(Kind::Machine).len()),
        4
    );
    assert!(!exists(cx, handle, "fleet-retry"));
}

#[gpui_kit::test]
async fn discarding_mine_after_a_conflict_leaves_the_list_on_pmds_row(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-row-0");
    set_input(cx, handle, &manage, "maxSessions", "5");
    backend.fail_next(WriteError::StaleVersion, |rows| {
        rows[0][0]["maxSessions"] = json!(1);
        rows[0][0]["version"] = json!(7);
    });
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "conflict-discard");
    click(cx, handle, "fleet-back");
    assert!(text_of(cx, handle, "fleet-row-0").contains("100.101.12.4, 1,"));
    click(cx, handle, "fleet-row-0");
    set_input(cx, handle, &manage, "maxSessions", "2");
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    assert_eq!(
        backend.writes.lock().unwrap().last().unwrap().precondition,
        Precondition::IfMatch(7)
    );
}

#[gpui_kit::test]
async fn an_agent_editor_opened_early_gets_pickers_as_the_lists_arrive(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-agent");
    click(cx, handle, "fleet-add");
    assert!(!exists(cx, handle, "fleet-subscription-grok-heavy"));
    // Another subscription arrives with a later load: its picker shows and works.
    backend.rows.lock().unwrap()[1].push(
        json!({"id": "grok-heavy", "provider": "xai", "plan": "", "metered": false,
        "usageSource": null, "source": "api", "version": 1}),
    );
    cx.update(|cx| manage.update(cx, |m, cx| m.reload_fleet(&[Kind::Subscription], cx)));
    settle(cx, handle, &view).await;
    click(cx, handle, "fleet-subscription-grok-heavy");
    assert_eq!(
        cx.update(|cx| manage
            .read(cx)
            .fleet()
            .unwrap()
            .editor_form()
            .unwrap()
            .value("subscription")
            .to_owned()),
        "grok-heavy"
    );
}

#[gpui_kit::test]
async fn a_new_agent_gets_its_quick_toggle_at_once(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-agent");
    click(cx, handle, "fleet-add");
    click(cx, handle, "fleet-machine-air");
    set_input(cx, handle, &manage, "provider", "codex");
    set_input(cx, handle, &manage, "alias", "mini-model");
    set_input(cx, handle, &manage, "model", "gpt-6.1-mini");
    click(cx, handle, "fleet-subscription-chatgpt-pro");
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    click(cx, handle, "fleet-back");
    assert_eq!(text_of(cx, handle, "fleet-toggle-4"), "Disable");
    click(cx, handle, "fleet-toggle-4");
    settle(cx, handle, &view).await;
    assert_eq!(value_of(&backend, 2, "air/codex/mini-model", "enabled"), json!(false));
}

#[gpui_kit::test]
async fn an_older_list_never_undoes_a_newer_write_or_drops_a_created_row(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-agent");
    // The snapshot a slow reload would bring: from before the toggle and the create below.
    let before = backend.rows.lock().unwrap()[2].clone();
    click(cx, handle, "fleet-toggle-1");
    settle(cx, handle, &view).await;
    click(cx, handle, "fleet-add");
    click(cx, handle, "fleet-machine-air");
    set_input(cx, handle, &manage, "provider", "codex");
    set_input(cx, handle, &manage, "alias", "model..preview");
    set_input(cx, handle, &manage, "model", "gpt-6.2-preview");
    click(cx, handle, "fleet-subscription-chatgpt-pro");
    click(cx, handle, "fleet-save");
    settle(cx, handle, &view).await;
    assert_eq!(backend.writes.lock().unwrap().len(), 2, "a dotted alias is sent");
    click(cx, handle, "fleet-back");
    *backend.stale_list.lock().unwrap() = Some((2, before));
    cx.update(|cx| manage.update(cx, |m, cx| m.reload_fleet(&[Kind::Agent], cx)));
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(
        text_of(cx, handle, "fleet-toggle-1"),
        "Enable",
        "the toggle's newer version stays"
    );
    let rows = cx.update(|cx| manage.read(cx).fleet().unwrap().rows(Kind::Agent).to_vec());
    assert!(
        rows.iter().any(|row| row.key == "air/codex/model..preview"),
        "the created row stays"
    );
}

#[gpui_kit::test]
async fn a_create_found_after_a_timeout_reaches_the_list_even_with_its_editor_closed(cx: &mut TestAppContext) {
    let backend = FakeFleet::new(&["read", "write:fleet"]);
    let (handle, view, manage) = open_with(cx, backend.clone());
    open_dialog(cx, handle, &view).await;
    click(cx, handle, "fleet-tab-subscription");
    click(cx, handle, "fleet-add");
    set_input(cx, handle, &manage, "id", "grok-heavy");
    set_input(cx, handle, &manage, "provider", "xai");
    backend.fail_next(
        WriteError::Timeout {
            create: Some(CreateCheck::Exists),
        },
        |rows| {
            rows[1].push(
                json!({"id": "grok-heavy", "provider": "xai", "plan": "", "metered": false,
                "usageSource": null, "source": "api", "version": 1}),
            );
        },
    );
    // Save, then go back to the list before pmd answers.
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| {
            m.save_fleet_row(window, cx);
            m.leave_fleet_editor(window, cx);
        })
    })
    .unwrap();
    settle(cx, handle, &view).await;
    let rows = cx.update(|cx| manage.read(cx).fleet().unwrap().rows(Kind::Subscription).to_vec());
    assert!(rows.iter().any(|row| row.key == "grok-heavy"));
    assert_eq!(backend.writes.lock().unwrap().len(), 1);
}
