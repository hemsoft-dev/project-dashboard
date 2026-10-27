//! Headless UI tests for the Manage menu and its dialogs (#33), in in-process test windows. No desktop window, OS
//! input or network: write access comes from in-memory backends.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashboard_core::cache::StatusCache;
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use dashboard_core::write::{Access, Feature, Whoami, WriteBackend, WriteError, WriteRequest};
use gpui_kit::test::{ClickOptions, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, Modifiers, MouseButton, MouseDownEvent, PlatformInput, TestAppContext,
    point, px,
};
use serde_json::Value;

use crate::board_view::{BoardView, Options};
use crate::demo;
use crate::manage::{Entry, Manage};
use crate::ui_tests::{four, open_board, options, settle};

const TOKEN: &str = "pmd-secret-write-token-0123456789";

/// A write token file at a fixed path, with the given scopes, that remembers what was stored.
pub(crate) struct FakeAccess {
    scopes: Vec<String>,
    stored: Mutex<Vec<String>>,
    /// Refuse to store, as a full disk or a locked file would.
    fail_store: AtomicBool,
}

impl FakeAccess {
    pub(crate) fn new(scopes: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            stored: Mutex::new(Vec::new()),
            fail_store: AtomicBool::new(false),
        })
    }
}

impl WriteBackend for FakeAccess {
    fn token_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/config/pmd-write-token"))
    }
    fn whoami(&self) -> Result<Whoami, WriteError> {
        Ok(Whoami {
            name: "project-dashboard@home".into(),
            scopes: self.scopes.clone(),
        })
    }
    fn store_token(&self, token: &str) -> Result<(), WriteError> {
        if self.fail_store.load(Ordering::SeqCst) {
            return Err(WriteError::Storage("the disk is full".into()));
        }
        self.stored.lock().unwrap().push(token.to_owned());
        Ok(())
    }
    fn read(&self, _: &str) -> Result<Value, WriteError> {
        Err(WriteError::NotFound)
    }
    fn write(&self, _: &WriteRequest) -> Result<Value, WriteError> {
        Err(WriteError::NotFound)
    }
}

/// The demo's statuses, counting reads.
#[derive(Default)]
struct CountingSource(AtomicUsize);

impl StatusSource for CountingSource {
    fn fetch(&self, checkout: &std::path::Path, refresh: bool) -> Result<StatusDocument, FetchError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        demo::DemoSource.fetch(checkout, refresh)
    }
}

/// The demo's sample row, counting the writes that reach it.
#[derive(Default)]
struct Counting {
    row: demo::DemoSample,
    writes: AtomicUsize,
}

impl WriteBackend for Counting {
    fn token_path(&self) -> Option<PathBuf> {
        None
    }
    fn whoami(&self) -> Result<Whoami, WriteError> {
        self.row.whoami()
    }
    fn store_token(&self, token: &str) -> Result<(), WriteError> {
        self.row.store_token(token)
    }
    fn read(&self, path: &str) -> Result<Value, WriteError> {
        self.row.read(path)
    }
    fn write(&self, request: &WriteRequest) -> Result<Value, WriteError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.row.write(request)
    }
}

fn open(
    cx: &mut TestAppContext,
    write: Arc<dyn WriteBackend>,
    sample: Option<Arc<dyn WriteBackend>>,
) -> (AnyWindowHandle, Entity<BoardView>) {
    open_board(
        cx,
        four(),
        StatusCache::default(),
        Arc::new(demo::DemoSource),
        Options {
            write,
            sample,
            ..options()
        },
    )
}

fn manage(cx: &mut TestAppContext, view: &Entity<BoardView>) -> Entity<Manage> {
    cx.update(|cx| view.read(cx).manage().clone())
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

fn click(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) {
    cx.update_window(handle, |_, window, cx| window.click(id, cx)).unwrap();
    frame(cx, handle);
}

fn focused(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> bool {
    cx.update_window(handle, |_, window, _| window.find(id).focused() == Some(true))
        .unwrap()
}

fn exists(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> bool {
    cx.update_window(handle, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

fn text_of(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) -> String {
    cx.update_window(handle, |_, window, _| {
        window.find(id).label().unwrap_or_default().to_owned()
    })
    .unwrap()
}

/// Everything the window's elements expose as labels and values.
fn rendered_text(cx: &mut TestAppContext, handle: AnyWindowHandle) -> String {
    use gpui_kit::test::TestQueryExt as _;
    cx.update_window(handle, |_, window, _| {
        window
            .elements()
            .iter()
            .flat_map(|element| [element.label(), element.value()])
            .flatten()
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap()
}

fn type_text(cx: &mut TestAppContext, handle: AnyWindowHandle, text: &str) {
    cx.update_window(handle, |_, window, cx| window.input(text, cx))
        .unwrap();
    frame(cx, handle);
}

fn focus_button(cx: &mut TestAppContext, handle: AnyWindowHandle, manage: &Entity<Manage>) {
    let focus = cx.update(|cx| manage.read(cx).button_focus().clone());
    cx.update_window(handle, |_, window, cx| focus.focus(window, cx))
        .unwrap();
    frame(cx, handle);
}

#[gpui_kit::test]
fn manage_menu_opens_with_the_mouse_and_escape_returns_focus_to_its_button(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    assert!(!exists(cx, handle, "manage-menu"));
    click(cx, handle, "manage-button");
    assert!(cx.update(|cx| manage.read(cx).is_menu_open()));
    for entry in [
        Entry::Feature(Feature::Projects),
        Entry::Feature(Feature::WorkItems),
        Entry::Feature(Feature::Fleet),
        Entry::WriteAccess,
    ] {
        assert!(exists(cx, handle, entry.id()), "{entry:?}");
    }
    // The demo's sample entry only shows with a sample backend.
    assert!(!exists(cx, handle, Entry::Sample.id()));
    assert!(focused(cx, handle, "manage-projects"), "focus moves into the menu");
    press(cx, handle, "escape");
    assert!(!cx.update(|cx| manage.read(cx).is_menu_open()));
    assert!(focused(cx, handle, "manage-button"));
    // A click outside the menu closes it too, without reaching the table.
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-menu-backdrop");
    assert!(!cx.update(|cx| manage.read(cx).is_menu_open()));
    assert!(!cx.update(|cx| view.read(cx).is_expanded(std::path::Path::new("/demo/gh-x"))));
}

#[gpui_kit::test]
fn manage_menu_works_by_keyboard(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    focus_button(cx, handle, &manage);
    press(cx, handle, "enter");
    assert!(focused(cx, handle, "manage-projects"));
    press(cx, handle, "down");
    assert!(focused(cx, handle, "manage-work-items"));
    press(cx, handle, "up");
    press(cx, handle, "up");
    assert!(focused(cx, handle, "manage-write-access"), "arrows wrap around");
    press(cx, handle, "enter");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), Some(Entry::WriteAccess));
    assert!(focused(cx, handle, "dialog-cancel"), "focus moves into the dialog");
    press(cx, handle, "escape");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
    assert!(focused(cx, handle, "manage-button"), "focus returns to the menu button");
    // Space and Down open the menu too; Tab leaves it.
    press(cx, handle, "space");
    assert!(cx.update(|cx| manage.read(cx).is_menu_open()));
    press(cx, handle, "tab");
    assert!(!cx.update(|cx| manage.read(cx).is_menu_open()));
    assert!(focused(cx, handle, "manage-button"));
    press(cx, handle, "down");
    assert!(cx.update(|cx| manage.read(cx).is_menu_open()));
}

#[gpui_kit::test]
fn without_write_access_every_dialog_opens_read_only_and_its_entry_says_why(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    let text = rendered_text(cx, handle);
    assert_eq!(
        text.matches("Read-only: writes are off: set pmd_write_token_path")
            .count(),
        3,
        "{text}"
    );
    assert!(!text.contains("Not built yet"), "{text}");
    for (entry, feature) in [
        ("manage-projects", Feature::Projects),
        ("manage-work-items", Feature::WorkItems),
        ("manage-fleet", Feature::Fleet),
    ] {
        click(cx, handle, entry);
        assert_eq!(cx.update(|cx| manage.read(cx).dialog()), Some(Entry::Feature(feature)));
        press(cx, handle, "escape");
        click(cx, handle, "manage-button");
    }
}

#[gpui_kit::test]
fn write_access_with_writes_off_explains_the_setup(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    assert_eq!(cx.update(|cx| manage.read(cx).access().clone()), Access::Off);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    assert!(exists(cx, handle, "write-access-off"));
    assert!(
        !exists(cx, handle, "write-token-input"),
        "no token to store without a path"
    );
    let label = cx
        .update_window(handle, |_, window, _| {
            window.find("manage-dialog").label().map(str::to_owned)
        })
        .unwrap();
    assert_eq!(label.as_deref(), Some("Write access"));
    // Enter runs the default action: Close.
    press(cx, handle, "enter");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
}

#[gpui_kit::test]
async fn write_access_shows_the_token_scopes_and_what_they_allow(cx: &mut TestAppContext) {
    let access = FakeAccess::new(&["read", "write:fleet"]);
    let (handle, view) = open(cx, access.clone(), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    let text = rendered_text(cx, handle);
    assert!(text.contains("Read-only: the token lacks write:work"), "{text}");
    assert!(text.contains("Read-only: the token lacks write:projects"), "{text}");
    assert!(
        !text.contains("Read-only: the token lacks write:fleet"),
        "fleet is allowed: {text}"
    );
    click(cx, handle, "manage-write-access");
    frame(cx, handle);
    assert_eq!(
        text_of(cx, handle, "write-token-path"),
        "Token file: /config/pmd-write-token"
    );
    assert_eq!(text_of(cx, handle, "write-token-name"), "Token: project-dashboard@home");
    assert_eq!(text_of(cx, handle, "write-token-scopes"), "Scopes: read, write:fleet");
    let text = rendered_text(cx, handle);
    assert!(text.contains("Work items: write:work Missing"), "{text}");
    assert!(text.contains("Fleet: write:fleet Allowed"), "{text}");
    assert!(text.contains("Projects: write:projects Missing"), "{text}");
    assert!(cx.update(|cx| matches!(manage.read(cx).access(), Access::Known(_))));
}

#[gpui_kit::test]
async fn a_pasted_token_is_masked_stored_and_never_rendered(cx: &mut TestAppContext) {
    let access = FakeAccess::new(&["read", "write:work"]);
    let (handle, view) = open(cx, access.clone(), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    assert!(
        focused(cx, handle, "write-token-input"),
        "focus starts in the token field"
    );
    type_text(cx, handle, TOKEN);
    assert!(!rendered_text(cx, handle).contains(TOKEN));
    // Enter saves.
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(*access.stored.lock().unwrap(), [TOKEN]);
    let input = cx.update(|cx| manage.read(cx).input("token").unwrap().read(cx).value().to_string());
    assert_eq!(input, "", "the field is cleared once stored");
    assert!(!rendered_text(cx, handle).contains(TOKEN));
    // An empty field asks for a token instead of storing nothing.
    press(cx, handle, "enter");
    assert_eq!(text_of(cx, handle, "dialog-message"), "Paste a token first.");
    assert_eq!(access.stored.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
fn tab_and_shift_tab_stay_inside_the_dialog(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, FakeAccess::new(&["read"]), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    let stops = ["dialog-close", "write-token-input", "dialog-cancel", "write-token-save"];
    let mut seen = Vec::new();
    for _ in 0..8 {
        press(cx, handle, "tab");
        let current = stops.iter().find(|id| focused(cx, handle, id)).copied();
        assert!(current.is_some(), "focus left the dialog");
        seen.push(current.unwrap());
    }
    for stop in stops {
        assert!(seen.contains(&stop), "{stop} never focused: {seen:?}");
    }
    for _ in 0..8 {
        press(cx, handle, "shift-tab");
        assert!(stops.iter().any(|id| focused(cx, handle, id)), "focus left the dialog");
    }
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), Some(Entry::WriteAccess));
}

#[gpui_kit::test]
fn unsaved_edits_ask_before_escape_or_the_close_button_discards_them(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, FakeAccess::new(&["read"]), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    type_text(cx, handle, "half-pasted");
    press(cx, handle, "escape");
    assert!(cx.update(|cx| manage.read(cx).is_confirming()));
    assert!(exists(cx, handle, "unsaved-confirm"));
    assert!(focused(cx, handle, "confirm-keep"));
    // Esc on the question keeps editing.
    press(cx, handle, "escape");
    assert!(!cx.update(|cx| manage.read(cx).is_confirming()));
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), Some(Entry::WriteAccess));
    // The close button asks too; Discard closes and focus returns to the button.
    click(cx, handle, "dialog-close");
    assert!(cx.update(|cx| manage.read(cx).is_confirming()));
    click(cx, handle, "confirm-discard");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
    assert!(focused(cx, handle, "manage-button"));
    // Without edits, Esc closes at once.
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    press(cx, handle, "escape");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
}

#[gpui_kit::test]
async fn the_table_ignores_input_behind_a_dialog_but_f5_still_refreshes(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, FakeAccess::new(&["read"]), None);
    let manage = manage(cx, &view);
    settle(cx, handle, &view).await;
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    cx.update_window(handle, |_, window, cx| window.click(("expand", 0usize), cx))
        .unwrap();
    frame(cx, handle);
    assert!(!cx.update(|cx| view.read(cx).is_expanded(std::path::Path::new("/demo/gh-x"))));
    let before = cx.update(|cx| view.read(cx).pending_refreshes());
    press(cx, handle, "f5");
    assert!(
        cx.update(|cx| view.read(cx).board().is_refreshing()),
        "F5 refreshes behind the dialog"
    );
    assert!(cx.update(|cx| view.read(cx).pending_refreshes()) > before);
    settle(cx, handle, &view).await;
    // Only one dialog at a time: the menu won't open over it.
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |m, cx| m.open_menu(window, cx))
    })
    .unwrap();
    assert!(!cx.update(|cx| manage.read(cx).is_menu_open()));
}

fn sample_title(cx: &mut TestAppContext, manage: &Entity<Manage>) -> String {
    cx.update(|cx| manage.read(cx).sample_form().unwrap().value("title").to_owned())
}

#[gpui_kit::test]
async fn sample_dialog_shows_field_errors_conflicts_and_reapplies(cx: &mut TestAppContext) {
    let source = Arc::new(CountingSource::default());
    let (handle, view) = open_board(
        cx,
        four(),
        StatusCache::default(),
        source.clone(),
        Options {
            sample: Some(Arc::new(demo::DemoSample::default())),
            ..options()
        },
    );
    let manage = manage(cx, &view);
    settle(cx, handle, &view).await;
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-sample");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(sample_title(cx, &manage), "Ship the Manage menu");
    assert!(focused(cx, handle, "field-title"));

    // A local check: text where a number goes, next to the field, and nothing is sent.
    cx.update_window(handle, |_, window, cx| window.click("field-priority", cx))
        .unwrap();
    type_text(cx, handle, "x");
    press(cx, handle, "enter");
    assert_eq!(
        text_of(cx, handle, "field-priority-error"),
        "Priority is a number from 0 to 5, or empty."
    );

    // pmd's 400 naming the field lands next to it too.
    let input = cx.update(|cx| manage.read(cx).input("priority").unwrap());
    cx.update_window(handle, |_, window, cx| {
        input.update(cx, |i, cx| i.set_value("9", window, cx))
    })
    .unwrap();
    frame(cx, handle);
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(
        text_of(cx, handle, "field-priority-error"),
        "priority must be 0-5, or null to inherit"
    );

    // A valid save meets a change made elsewhere: both values show, then Reapply saves mine on the new version.
    cx.update_window(handle, |_, window, cx| {
        input.update(cx, |i, cx| i.set_value("2", window, cx))
    })
    .unwrap();
    let title = cx.update(|cx| manage.read(cx).input("title").unwrap());
    cx.update_window(handle, |_, window, cx| {
        title.update(cx, |i, cx| i.set_value("Mine", window, cx))
    })
    .unwrap();
    frame(cx, handle);
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(exists(cx, handle, "conflict"));
    assert!(text_of(cx, handle, "dialog-message").starts_with("Changed elsewhere"));
    let text = rendered_text(cx, handle);
    assert!(
        text.contains("Title: yours Mine, now Ship the Manage menu and dialogs"),
        "{text}"
    );
    assert!(focused(cx, handle, "conflict-reapply"));
    let reads_before = source.0.load(Ordering::SeqCst);
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(!exists(cx, handle, "conflict"));
    let form = cx.update(|cx| manage.read(cx).sample_form().cloned().unwrap());
    assert_eq!(form.value("title"), "Mine");
    assert_eq!(form.value("priority"), "2");
    assert!(!form.is_dirty());
    assert_eq!(form.version(), Some(5));
    // The write reloaded pmd data at once: every card was read again.
    assert_eq!(source.0.load(Ordering::SeqCst), reads_before + 4);
}

#[gpui_kit::test]
async fn changed_elsewhere_can_discard_my_edits(cx: &mut TestAppContext) {
    let sample = Arc::new(Counting::default());
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), Some(sample.clone()));
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-sample");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    type_text(cx, handle, " now");
    // A second press while the first save runs sends nothing more.
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |manage, cx| {
            manage.save_sample(window, cx);
            manage.save_sample(window, cx);
        })
    })
    .unwrap();
    settle(cx, handle, &view).await;
    assert_eq!(sample.writes.load(Ordering::SeqCst), 1);
    frame(cx, handle);
    assert!(exists(cx, handle, "conflict"));
    click(cx, handle, "conflict-discard");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(sample_title(cx, &manage), "Ship the Manage menu and dialogs");
    let input = cx.update(|cx| manage.read(cx).input("title").unwrap().read(cx).value().to_string());
    assert_eq!(input, "Ship the Manage menu and dialogs");
    assert!(!cx.update(|cx| manage.read(cx).sample_form().unwrap().is_dirty()));
    // Nothing unsaved: Esc closes without asking.
    press(cx, handle, "escape");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), None);
}

#[gpui_kit::test]
async fn a_token_save_that_lands_after_its_dialog_closed_leaves_a_newer_dialog_alone(cx: &mut TestAppContext) {
    let access = FakeAccess::new(&["read", "write:work"]);
    let (handle, view) = open(cx, access.clone(), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    type_text(cx, handle, "first-token");
    // Save, then discard the dialog and open a new one before the save lands.
    cx.update_window(handle, |_, window, cx| {
        manage.update(cx, |manage, cx| {
            manage.store_token(window, cx);
            manage.close_dialog(window, cx);
            manage.activate(Entry::WriteAccess, window, cx);
        });
        let input = manage.read(cx).input("token").unwrap();
        input.update(cx, |input, cx| input.set_value("second-token", window, cx));
    })
    .unwrap();
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert_eq!(*access.stored.lock().unwrap(), ["first-token"]);
    let input = cx.update(|cx| manage.read(cx).input("token").unwrap().read(cx).value().to_string());
    assert_eq!(
        input, "second-token",
        "a late save must not clear the newer dialog's field"
    );
    // The new dialog isn't stuck "saving": its own save goes through.
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    assert_eq!(*access.stored.lock().unwrap(), ["first-token", "second-token"]);
}

#[gpui_kit::test]
async fn an_access_check_about_the_replaced_token_cannot_overwrite_the_new_one(cx: &mut TestAppContext) {
    let access = FakeAccess::new(&["read", "write:work"]);
    let (handle, view) = open(cx, access.clone(), None);
    let manage = manage(cx, &view);
    // Opening the menu starts a check of the token that is about to be replaced.
    click(cx, handle, "manage-button");
    let old_check = cx.update(|cx| manage.read(cx).access_generation());
    click(cx, handle, "manage-write-access");
    type_text(cx, handle, "replacement-token");
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    let new_access = cx.update(|cx| manage.read(cx).access().clone());
    assert!(matches!(new_access, Access::Known(_)));
    // The old check answers last, with the replaced token's name and scopes: it is dropped.
    cx.update(|cx| {
        manage.update(cx, |manage, cx| {
            manage.finish_access(
                old_check,
                Ok(Whoami {
                    name: "old-token@home".into(),
                    scopes: vec!["read".into()],
                }),
                cx,
            )
        })
    });
    assert_eq!(cx.update(|cx| manage.read(cx).access().clone()), new_access);
}

#[gpui_kit::test]
async fn a_failed_token_store_says_why_and_rechecks_the_token_still_in_place(cx: &mut TestAppContext) {
    let access = FakeAccess::new(&["read", "write:work"]);
    access.fail_store.store(true, Ordering::SeqCst);
    let (handle, view) = open(cx, access.clone(), None);
    let manage = manage(cx, &view);
    click(cx, handle, "manage-button");
    click(cx, handle, "manage-write-access");
    type_text(cx, handle, "replacement-token");
    press(cx, handle, "enter");
    settle(cx, handle, &view).await;
    frame(cx, handle);
    assert!(text_of(cx, handle, "dialog-message").starts_with("Can't store the token"));
    assert!(access.stored.lock().unwrap().is_empty());
    // Access isn't left at "Checking…": the old token was checked again.
    assert!(cx.update(|cx| matches!(manage.read(cx).access(), Access::Known(_))));
    let input = cx.update(|cx| manage.read(cx).input("token").unwrap().read(cx).value().to_string());
    assert_eq!(input, "replacement-token", "the pasted token stays for another try");
}

fn menu_open(cx: &mut TestAppContext, manage: &Entity<Manage>) -> bool {
    cx.update(|cx| manage.read(cx).is_menu_open())
}

/// Clicks the Manage button `inset` pixels in from its left edge (the label) or its right edge (the arrow).
fn click_button_at(cx: &mut TestAppContext, handle: AnyWindowHandle, from_right: bool, button: MouseButton) {
    cx.update_window(handle, |_, window, cx| {
        let size = window.find("manage-button").bounds().size;
        let x = if from_right { size.width - px(4.) } else { px(4.) };
        let options = ClickOptions::new()
            .with_offset(point(x, size.height / 2.))
            .with_button(button);
        window.click_with_options("manage-button", options, cx)
    })
    .unwrap();
    frame(cx, handle);
}

#[gpui_kit::test]
fn a_left_click_on_the_manage_label_or_arrow_opens_the_menu_and_a_second_click_closes_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    for from_right in [false, true] {
        click_button_at(cx, handle, from_right, MouseButton::Left);
        assert!(
            menu_open(cx, &manage),
            "a left click opens the menu (arrow: {from_right})"
        );
        assert!(focused(cx, handle, "manage-projects"));
        click_button_at(cx, handle, from_right, MouseButton::Left);
        assert!(
            !menu_open(cx, &manage),
            "a second left click closes it (arrow: {from_right})"
        );
        assert!(focused(cx, handle, "manage-button"));
    }
}

/// Presses the left button on the Manage button with no release, as on Windows when the OS swallowed the release.
fn left_press_without_release(cx: &mut TestAppContext, handle: AnyWindowHandle) {
    cx.update_window(handle, |_, window, cx| {
        let position = window.find("manage-button").bounds().center();
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
    })
    .unwrap();
    frame(cx, handle);
}

#[gpui_kit::test]
fn a_right_click_on_the_manage_button_opens_nothing(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    for from_right in [false, true] {
        click_button_at(cx, handle, from_right, MouseButton::Right);
        assert!(
            !menu_open(cx, &manage),
            "a right click leaves the menu closed (arrow: {from_right})"
        );
    }
    // v0.5.0 on Windows: the OS took the left release to move the window, and a later right release completed the
    // pending left press as a click. Only a left press and release, or the keyboard, may open the menu.
    left_press_without_release(cx, handle);
    click_button_at(cx, handle, true, MouseButton::Right);
    assert!(
        !menu_open(cx, &manage),
        "a right release never completes a left press into a click"
    );
    assert!(!exists(cx, handle, "manage-menu"));
}

#[gpui_kit::test]
fn alt_m_opens_the_manage_menu_from_anywhere_and_escape_closes_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, Arc::new(dashboard_core::write::Off), None);
    let manage = manage(cx, &view);
    assert!(!focused(cx, handle, "manage-button"));
    press(cx, handle, "alt-m");
    assert!(menu_open(cx, &manage));
    assert!(focused(cx, handle, "manage-projects"), "focus moves into the menu");
    press(cx, handle, "escape");
    assert!(!menu_open(cx, &manage));
    assert!(focused(cx, handle, "manage-button"));
    // From the focused button too, and never over an open dialog.
    press(cx, handle, "alt-m");
    assert!(menu_open(cx, &manage));
    press(cx, handle, "up");
    press(cx, handle, "enter");
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), Some(Entry::WriteAccess));
    press(cx, handle, "alt-m");
    assert!(!menu_open(cx, &manage));
    assert_eq!(cx.update(|cx| manage.read(cx).dialog()), Some(Entry::WriteAccess));
}
