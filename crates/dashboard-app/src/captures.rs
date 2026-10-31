//! Screenshots for pull requests, rendered offscreen by GPUI's headless renderer (wgpu on Linux, Metal on macOS), so no
//! window ever appears on a desktop. Windows has no headless renderer in GPUI.
//!
//! `CAPTURE_DIR=<absolute folder> cargo test -p dashboard-app captures -- --ignored` writes one PNG per state. Cargo
//! runs the test from this crate's folder, so a relative folder lands under `crates/dashboard-app`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashboard_core::cache::StatusCache;
use dashboard_core::scheduler::Outcome;
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, px, size};

use crate::board_view::{BoardView, Options};
use crate::{demo, theme};

fn save(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, dir: &Path, name: &str) {
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let image = cx
        .capture_screenshot(handle)
        .expect("the headless renderer draws the window");
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

fn options() -> Options {
    Options {
        cache_dir: None,
        parallelism: 4,
        auto_refresh: None,
        config_problem: None,
        config_path: Some(PathBuf::from("~/.config/project-dashboard/config.toml")),
        config_dir: None,
        now: Some(demo::fixture_now()),
        columns: Default::default(),
        ui_state_dir: None,
        threads: Arc::new(demo::DemoThreads),
        priorities: Arc::new(demo::DemoPriorities),
        pmd: None,
        write: Arc::new(dashboard_core::write::Off),
        sample: None,
    }
}

fn open(cx: &mut HeadlessAppContext, checkouts: Vec<PathBuf>, height: f32) -> (AnyWindowHandle, Entity<BoardView>) {
    open_sized(cx, checkouts, 1100., height)
}

fn open_sized(
    cx: &mut HeadlessAppContext,
    checkouts: Vec<PathBuf>,
    width: f32,
    height: f32,
) -> (AnyWindowHandle, Entity<BoardView>) {
    open_with(cx, checkouts, width, height, options())
}

fn open_with(
    cx: &mut HeadlessAppContext,
    checkouts: Vec<PathBuf>,
    width: f32,
    height: f32,
    options: Options,
) -> (AnyWindowHandle, Entity<BoardView>) {
    let mut view = None;
    let handle = cx
        .open_window(size(px(width), px(height)), |window, cx| {
            let board = cx.new(|cx| {
                BoardView::new(
                    &checkouts,
                    StatusCache::default(),
                    Arc::new(demo::DemoSource),
                    options,
                    window,
                    cx,
                )
            });
            view = Some(board.clone());
            cx.new(|cx| Root::new(board, window, cx))
        })
        .unwrap();
    (handle.into(), view.unwrap())
}

/// Shows each checkout's result from `source` as a refresh would, without the scheduler's threads.
fn fill(cx: &mut HeadlessAppContext, view: &Entity<BoardView>, source: &dyn StatusSource) {
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            use dashboard_core::priority::PrioritySource as _;
            view.apply_priorities(demo::DemoPriorities.fetch(), cx);
            for checkout in view.board().checkouts() {
                let result = source.fetch(&checkout, false);
                view.apply(Outcome { checkout, result }, cx);
            }
        })
    });
    cx.run_until_parked();
}

/// Lets background work (a whoami, a save, a re-read) finish: it runs on real threads.
fn settle(cx: &mut HeadlessAppContext) {
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        cx.run_until_parked();
    }
}

fn key(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, keys: &[&str]) {
    for key in keys {
        cx.update_window(handle, |_, window, cx| window.press(key, cx)).unwrap();
        settle(cx);
    }
}

fn manage(cx: &mut HeadlessAppContext, out: &Path, suffix: &str) {
    use crate::manage::Manage;
    use crate::manage_tests::FakeAccess;
    fn manage_of(cx: &mut HeadlessAppContext, view: &Entity<BoardView>) -> Entity<Manage> {
        cx.update(|cx| view.read(cx).manage().clone())
    }
    let name = |state: &str| format!("manage-{state}{suffix}");

    // Writes off: the menu opened by keyboard, its entries disabled with reasons, then Write access… explaining setup.
    let (off, off_view) = open_with(
        cx,
        demo::checkouts(),
        1100.,
        760.,
        Options {
            sample: Some(Arc::new(demo::DemoSample::default())),
            ..options()
        },
    );
    fill(cx, &off_view, &demo::DemoSource);
    let off_manage = manage_of(cx, &off_view);
    let button = cx.update(|cx| off_manage.read(cx).button_focus().clone());
    cx.update_window(off, |_, window, cx| button.focus(window, cx)).unwrap();
    key(cx, off, &["enter", "down"]);
    save(cx, off, out, &name("menu"));
    key(cx, off, &["down", "down", "enter"]);
    save(cx, off, out, &name("write-access-off"));
    key(cx, off, &["escape"]);

    // The sample dialog: open, an inline validation error, the unsaved-edit question, then "Changed elsewhere".
    key(cx, off, &["enter", "up", "enter"]);
    settle(cx);
    save(cx, off, out, &name("dialog"));
    let input = |cx: &mut HeadlessAppContext, field: &str| cx.update(|cx| off_manage.read(cx).input(field).unwrap());
    let priority = input(cx, "priority");
    cx.update_window(off, |_, window, cx| {
        priority.update(cx, |i, cx| i.set_value("9", window, cx))
    })
    .unwrap();
    key(cx, off, &["enter"]);
    save(cx, off, out, &name("validation-error"));
    key(cx, off, &["escape"]);
    save(cx, off, out, &name("unsaved-confirm"));
    key(cx, off, &["escape"]);
    let title = input(cx, "title");
    cx.update_window(off, |_, window, cx| {
        priority.update(cx, |i, cx| i.set_value("2", window, cx));
        title.update(cx, |i, cx| i.set_value("Ship the Manage menu first", window, cx));
    })
    .unwrap();
    settle(cx);
    key(cx, off, &["enter"]);
    save(cx, off, out, &name("conflict"));

    // A token with scopes: Work items allowed, Fleet and Projects missing.
    let (on, on_view) = open_with(
        cx,
        demo::checkouts(),
        1100.,
        760.,
        Options {
            write: FakeAccess::new(&["read", "write:work"]),
            ..options()
        },
    );
    fill(cx, &on_view, &demo::DemoSource);
    let on_manage = manage_of(cx, &on_view);
    let button = cx.update(|cx| on_manage.read(cx).button_focus().clone());
    cx.update_window(on, |_, window, cx| button.focus(window, cx)).unwrap();
    key(cx, on, &["enter"]);
    save(cx, on, out, &name("menu-scoped"));
    key(cx, on, &["up", "enter"]);
    cx.update_window(on, |_, window, cx| window.input("pasted-token-is-masked", cx))
        .unwrap();
    settle(cx);
    save(cx, on, out, &name("write-access-scopes"));

    // Frames for the keyboard recording, at 100% only: focus the button, open the menu, walk to Write access…, open
    // it, Tab around the dialog (focus stays inside), Shift+Tab back, then Esc returns focus to the button.
    if !suffix.is_empty() {
        return;
    }
    let (keys, keys_view) = open_with(
        cx,
        demo::checkouts(),
        1100.,
        760.,
        Options {
            write: FakeAccess::new(&["read", "write:work"]),
            ..options()
        },
    );
    fill(cx, &keys_view, &demo::DemoSource);
    let keys_manage = manage_of(cx, &keys_view);
    let button = cx.update(|cx| keys_manage.read(cx).button_focus().clone());
    cx.update_window(keys, |_, window, cx| button.focus(window, cx))
        .unwrap();
    settle(cx);
    let mut frame = 0;
    let mut shot = |cx: &mut HeadlessAppContext| {
        frame += 1;
        save(cx, keys, out, &format!("keys-{frame:02}"));
    };
    shot(cx);
    for step in [
        "enter",
        "down",
        "down",
        "down",
        "enter",
        "tab",
        "tab",
        "tab",
        "tab",
        "tab",
        "shift-tab",
        "escape",
    ] {
        key(cx, keys, &[step]);
        shot(cx);
    }
}

/// The Work items dialog (#35): each state the issue names, and at 100% a frame sequence of a keyboard-only status
/// change for the recording.
fn work_items(cx: &mut HeadlessAppContext, out: &Path, suffix: &str) {
    use crate::work_items_tests::{FakeWork, sample_items};
    use dashboard_core::write::WriteError;
    let name = |state: &str| format!("work-{state}{suffix}");
    let open_dialog = |cx: &mut HeadlessAppContext, scopes: &[&str]| {
        let backend = FakeWork::new(scopes, sample_items());
        let (handle, view) = open_with(
            cx,
            demo::checkouts(),
            1100.,
            760.,
            Options {
                write: backend.clone(),
                ..options()
            },
        );
        fill(cx, &view, &demo::DemoSource);
        let manage = cx.update(|cx| view.read(cx).manage().clone());
        settle(cx);
        cx.update_window(handle, |_, window, cx| {
            manage.update(cx, |m, cx| {
                m.open_menu(window, cx);
                m.activate(
                    crate::manage::Entry::Feature(dashboard_core::write::Feature::WorkItems),
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        settle(cx);
        (handle, manage, backend)
    };
    let click = |cx: &mut HeadlessAppContext, handle: AnyWindowHandle, id: &str| {
        let id = gpui_kit::SharedString::from(id.to_owned());
        cx.update_window(handle, |_, window, cx| window.click(id, cx)).unwrap();
        settle(cx);
    };
    let set = |cx: &mut HeadlessAppContext,
               handle: AnyWindowHandle,
               manage: &Entity<crate::manage::Manage>,
               field: &str,
               text: &str| {
        let input = cx.update(|cx| manage.read(cx).input(field).unwrap());
        let text = text.to_owned();
        cx.update_window(handle, |_, window, cx| {
            input.update(cx, |i, cx| i.set_value(text, window, cx))
        })
        .unwrap();
        settle(cx);
    };

    let (handle, manage, backend) = open_dialog(cx, &["read", "write:work"]);
    click(cx, handle, "work-filter-kind-issue");
    save(cx, handle, out, &name("list-filtered"));
    click(cx, handle, "work-filter-kind-all");
    click(cx, handle, "work-row-0");
    save(cx, handle, out, &name("editor-auto"));
    click(cx, handle, "work-priority-inherit");
    click(cx, handle, "work-back");
    click(cx, handle, "work-row-3");
    save(cx, handle, out, &name("editor-blocked"));
    click(cx, handle, "work-priority-inherit");
    save(cx, handle, out, &name("editor-inherit"));
    set(
        cx,
        handle,
        &manage,
        "statusReason",
        &"needs the code-signing certificate from the vendor; ".repeat(10),
    );
    set(cx, handle, &manage, "rank", "first");
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("validation-error"));
    set(
        cx,
        handle,
        &manage,
        "statusReason",
        "needs the signing certificate from the vendor",
    );
    set(cx, handle, &manage, "rank", "-3");
    backend.fail_next(WriteError::StaleVersion, |items| {
        items[3]["status"] = serde_json::json!("waiting_on_franz");
        items[3]["statusReason"] = serde_json::json!("asked Franz for the certificate");
        items[3]["version"] = serde_json::json!(8);
    });
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("conflict"));
    click(cx, handle, "conflict-discard");
    click(cx, handle, "work-status-ready");
    backend.fail_next(WriteError::ItemClosed, |items| {
        items[3]["status"] = serde_json::json!("done");
        items[3]["ghState"] = serde_json::json!("closed");
        items[3]["version"] = serde_json::json!(9);
    });
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("closed"));

    let (read_only, _, _) = open_dialog(cx, &["read", "write:fleet"]);
    save(cx, read_only, out, &name("read-only"));
    key(cx, read_only, &["enter"]);
    save(cx, read_only, out, &name("read-only-editor"));

    // Frames for the recording, at 100% only: keyboard alone, from the list to a saved Blocked status.
    if !suffix.is_empty() {
        return;
    }
    let (keys, _, _) = open_dialog(cx, &["read", "write:work"]);
    let mut frame = 0;
    let mut shot = |cx: &mut HeadlessAppContext| {
        frame += 1;
        save(cx, keys, out, &format!("work-keys-{frame:02}"));
    };
    shot(cx);
    // Down to the codexbar PR, open it (focus on its automatic status), Shift+Tab to Blocked and pick it, then Tab
    // past the automatic choice into the reason, now enabled.
    for step in ["down", "down", "enter", "shift-tab", "enter", "tab", "tab"] {
        key(cx, keys, &[step]);
        shot(cx);
    }
    for typed in ["waiting", " on", " the", " vendor"] {
        cx.update_window(keys, |_, window, cx| window.input(typed, cx)).unwrap();
        settle(cx);
        shot(cx);
    }
    // Save, then Tab through priority and rank to Back to list, which shows the row as pmd answered.
    key(cx, keys, &["ctrl-enter"]);
    shot(cx);
    for _ in 0..9 {
        key(cx, keys, &["tab"]);
    }
    shot(cx);
    key(cx, keys, &["enter"]);
    shot(cx);
}

/// The Fleet dialog (#36): each state the issue names, and at 100% a frame sequence of a keyboard-only
/// `maxSessions` change for the recording.
fn fleet(cx: &mut HeadlessAppContext, out: &Path, suffix: &str) {
    use crate::fleet_tests::FakeFleet;
    use dashboard_core::write::WriteError;
    let name = |state: &str| format!("fleet-{state}{suffix}");
    let open_dialog = |cx: &mut HeadlessAppContext, scopes: &[&str]| {
        let backend = FakeFleet::new(scopes);
        let (handle, view) = open_with(
            cx,
            demo::checkouts(),
            1100.,
            760.,
            Options {
                write: backend.clone(),
                ..options()
            },
        );
        fill(cx, &view, &demo::DemoSource);
        let manage = cx.update(|cx| view.read(cx).manage().clone());
        settle(cx);
        cx.update_window(handle, |_, window, cx| {
            manage.update(cx, |m, cx| {
                m.open_menu(window, cx);
                m.activate(
                    crate::manage::Entry::Feature(dashboard_core::write::Feature::Fleet),
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        settle(cx);
        (handle, manage, backend)
    };
    let click = |cx: &mut HeadlessAppContext, handle: AnyWindowHandle, id: &str| {
        let id = gpui_kit::SharedString::from(id.to_owned());
        cx.update_window(handle, |_, window, cx| window.click(id, cx)).unwrap();
        settle(cx);
    };
    let set = |cx: &mut HeadlessAppContext,
               handle: AnyWindowHandle,
               manage: &Entity<crate::manage::Manage>,
               field: &str,
               text: &str| {
        let input = cx.update(|cx| manage.read(cx).input(field).unwrap());
        let text = text.to_owned();
        cx.update_window(handle, |_, window, cx| {
            input.update(cx, |i, cx| i.set_value(text, window, cx))
        })
        .unwrap();
        settle(cx);
    };

    let (handle, manage, backend) = open_dialog(cx, &["read", "write:fleet"]);
    save(cx, handle, out, &name("machines"));
    click(cx, handle, "fleet-row-3");
    save(cx, handle, out, &name("config-note"));
    set(cx, handle, &manage, "maxSessions", "2");
    save(cx, handle, out, &name("max-sessions-edit"));
    backend.fail_next(WriteError::StaleVersion, |rows| {
        rows[0][3]["maxSessions"] = serde_json::json!(4);
        rows[0][3]["source"] = serde_json::json!("api");
        rows[0][3]["version"] = serde_json::json!(6);
    });
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("conflict"));
    click(cx, handle, "conflict-discard");
    click(cx, handle, "fleet-back");
    click(cx, handle, "fleet-tab-subscription");
    save(cx, handle, out, &name("subscriptions"));
    click(cx, handle, "fleet-tab-agent");
    click(cx, handle, "fleet-toggle-3");
    save(cx, handle, out, &name("agents-toggle"));
    click(cx, handle, "fleet-add");
    click(cx, handle, "fleet-machine-laptop");
    set(cx, handle, &manage, "provider", "claude");
    set(cx, handle, &manage, "alias", "sonnet-5.5");
    set(cx, handle, &manage, "model", "claude-sonnet-5-5");
    click(cx, handle, "fleet-subscription-claude-max");
    click(cx, handle, "fleet-suggest-roles-review");
    backend.fail_next(
        WriteError::BadRequest {
            code: "unknown_subscription".into(),
            message: "agent \"laptop/claude/sonnet-5.5\" uses subscription \"claude-max\", which pmd does not have"
                .into(),
        },
        |_| {},
    );
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("add-agent-error"));

    let (read_only, _, _) = open_dialog(cx, &["read", "write:work"]);
    click(cx, read_only, "fleet-tab-agent");
    save(cx, read_only, out, &name("read-only"));

    // Frames for the recording, at 100% only: keyboard alone, from the tab strip to a saved maxSessions.
    if !suffix.is_empty() {
        return;
    }
    let (keys, _, _) = open_dialog(cx, &["read", "write:fleet"]);
    let mut frame = 0;
    let mut shot = |cx: &mut HeadlessAppContext| {
        frame += 1;
        save(cx, keys, out, &format!("fleet-keys-{frame:02}"));
    };
    shot(cx);
    // Tab to the list, Down to mini, Enter to edit it, Tab to Max sessions, then type 2 over the 3.
    for step in ["tab", "down", "down", "down", "enter", "tab"] {
        key(cx, keys, &[step]);
        shot(cx);
    }
    for step in ["end", "backspace"] {
        key(cx, keys, &[step]);
    }
    cx.update_window(keys, |_, window, cx| window.input("2", cx)).unwrap();
    settle(cx);
    shot(cx);
    key(cx, keys, &["ctrl-enter"]);
    shot(cx);
    for _ in 0..12 {
        key(cx, keys, &["tab"]);
        let back = cx
            .update_window(keys, |_, window, _| window.find("fleet-back").focused() == Some(true))
            .unwrap();
        if back {
            break;
        }
    }
    shot(cx);
    key(cx, keys, &["enter"]);
    shot(cx);
}

/// The Projects dialog (#34): each state the issue names, and at 100% a frame sequence of keyboard-only add and edit.
fn projects(cx: &mut HeadlessAppContext, out: &Path, suffix: &str) {
    use crate::projects_tests::{FakeProjects, Folder};
    use dashboard_core::write::WriteError;
    let name = |state: &str| format!("projects-{state}{suffix}");
    let folder = Folder::new();
    let open_dialog = |cx: &mut HeadlessAppContext, scopes: &[&str], folder: &Folder| {
        let backend = FakeProjects::new(scopes);
        let (handle, view) = open_with(
            cx,
            demo::checkouts(),
            1100.,
            760.,
            Options {
                write: backend.clone(),
                config_path: Some(folder.config_dir().join("config.toml")),
                ..options()
            },
        );
        fill(cx, &view, &demo::DemoSource);
        let manage = cx.update(|cx| view.read(cx).manage().clone());
        settle(cx);
        cx.update_window(handle, |_, window, cx| {
            manage.update(cx, |m, cx| {
                m.open_menu(window, cx);
                m.activate(
                    crate::manage::Entry::Feature(dashboard_core::write::Feature::Projects),
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        settle(cx);
        (handle, manage, backend)
    };
    let click = |cx: &mut HeadlessAppContext, handle: AnyWindowHandle, id: &str| {
        let id = gpui_kit::SharedString::from(id.to_owned());
        cx.update_window(handle, |_, window, cx| window.click(id, cx)).unwrap();
        settle(cx);
    };
    let set = |cx: &mut HeadlessAppContext,
               handle: AnyWindowHandle,
               manage: &Entity<crate::manage::Manage>,
               field: &str,
               text: &str| {
        let input = cx.update(|cx| manage.read(cx).input(field).unwrap());
        let text = text.to_owned();
        cx.update_window(handle, |_, window, cx| {
            input.update(cx, |i, cx| i.set_value(text, window, cx))
        })
        .unwrap();
        settle(cx);
    };

    let (handle, manage, backend) = open_dialog(cx, &["read", "write:projects"], &folder);
    save(cx, handle, out, &name("list"));
    click(cx, handle, "projects-show-untracked");
    save(cx, handle, out, &name("list-untracked"));
    click(cx, handle, "projects-show-untracked");
    // hs-buddy: no checkout configured, so Link checkout shows.
    click(cx, handle, "project-row-2");
    click(cx, handle, "project-priority-0");
    save(cx, handle, out, &name("edit-priority"));
    let fork = folder.checkout("fork", "https://github.com/someone/hs-buddy.git");
    set(cx, handle, &manage, "folder", &fork.to_string_lossy());
    // The button can sit below the fold at 150%; press it directly.
    cx.update(|cx| manage.update(cx, |m, cx| m.link_checkout(cx)));
    settle(cx);
    save(cx, handle, out, &name("link-mismatch"));
    backend.fail_next(WriteError::StaleVersion, |rows| {
        let row = rows.iter_mut().find(|row| row["slug"] == "hs-buddy").unwrap();
        row["priority"] = serde_json::json!(1);
        row["goal"] = serde_json::json!("Ship the signed installer this week");
        row["version"] = serde_json::json!(5);
    });
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("conflict"));
    click(cx, handle, "conflict-discard");
    click(cx, handle, "project-untrack");
    save(cx, handle, out, &name("untrack-confirm"));
    click(cx, handle, "untrack-keep");
    click(cx, handle, "projects-back");
    click(cx, handle, "projects-add");
    set(cx, handle, &manage, "repo", "hemsoft-dev/Yahtzee Bench");
    click(cx, handle, "project-priority-5");
    key(cx, handle, &["ctrl-enter"]);
    save(cx, handle, out, &name("add-error"));

    let (read_only, _, _) = open_dialog(cx, &["read", "write:work"], &folder);
    save(cx, read_only, out, &name("read-only"));

    // Frames for the recording, at 100% only: keyboard alone, adding a project and setting its priority.
    if !suffix.is_empty() {
        return;
    }
    let (keys, _, _) = open_dialog(cx, &["read", "write:projects"], &folder);
    let mut frame = 0;
    let mut shot = |cx: &mut HeadlessAppContext| {
        frame += 1;
        save(cx, keys, out, &format!("projects-keys-{frame:02}"));
    };
    shot(cx);
    // From the list, Tab to Close and Add project, then Enter opens the empty editor with focus on the repository.
    for _ in 0..8 {
        key(cx, keys, &["tab"]);
        let add = cx
            .update_window(keys, |_, window, _| window.find("projects-add").focused() == Some(true))
            .unwrap();
        if add {
            break;
        }
    }
    shot(cx);
    key(cx, keys, &["enter"]);
    shot(cx);
    for typed in ["hemsoft-dev/", "yahtzee"] {
        cx.update_window(keys, |_, window, cx| window.input(typed, cx)).unwrap();
        settle(cx);
        shot(cx);
    }
    // Tab past the slug (empty uses yahtzee) to P2, Enter picks it, and Ctrl+Enter adds the project.
    for _ in 0..4 {
        key(cx, keys, &["tab"]);
    }
    shot(cx);
    key(cx, keys, &["enter"]);
    shot(cx);
    key(cx, keys, &["ctrl-enter"]);
    shot(cx);
}

/// A headless app with the dashboard's theme, keys and zoom.
fn headless() -> HeadlessAppContext {
    // The Mac platform can only be created on the main thread; a test runs on another, so take its text system only.
    #[cfg(target_os = "macos")]
    let text_system: Arc<dyn gpui_kit::PlatformTextSystem> = Arc::new(gpui_apple::AppleTextSystem::new());
    #[cfg(not(target_os = "macos"))]
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(gpui_kit::assets::Assets), || {
        gpui_kit::platform::current_headless_renderer()
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        crate::board_view::bind_keys(cx);
        crate::zoom::init(cx, None);
    });
    cx
}

#[test]
#[ignore = "writes screenshots; run with CAPTURE_DIR set"]
fn activity_width_captures() {
    let Some(out) = std::env::var_os("CAPTURE_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    for (level, repository) in [(1.0, 18.0), (1.3, 20.88)] {
        cx.update(|cx| {
            crate::zoom::set_level(level, cx);
            crate::zoom::hide_toast(cx);
        });
        for width in [760., 1100., 1680.] {
            let checkouts = vec!["/demo/short-label".into(), "/demo/long-label".into()];
            let mut view = None;
            let window = cx
                .open_window(size(px(width), px(400.)), |window, cx| {
                    let mut options = options();
                    options
                        .columns
                        .set(dashboard_core::columns::Column::Repository, repository);
                    let board = cx.new(|cx| {
                        BoardView::new(
                            &checkouts,
                            StatusCache::default(),
                            Arc::new(crate::ui_tests::ActivityLayoutSource),
                            options,
                            window,
                            cx,
                        )
                    });
                    view = Some(board.clone());
                    cx.new(|cx| Root::new(board, window, cx))
                })
                .unwrap();
            fill(&mut cx, &view.unwrap(), &crate::ui_tests::ActivityLayoutSource);
            save(
                &mut cx,
                window.into(),
                &out,
                &format!("activity-{width}-{}", (level * 100.) as u32),
            );
        }
    }
}

#[test]
#[ignore = "writes screenshots; run with CAPTURE_DIR set"]
fn captures() {
    let Some(out) = std::env::var_os("CAPTURE_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();

    // The Manage menu and its dialogs (#33), at the app's default size and at 150%.
    for (level, suffix) in [(1.0, ""), (1.5, "-150")] {
        cx.update(|cx| {
            crate::zoom::set_level(level, cx);
            crate::zoom::hide_toast(cx);
        });
        manage(&mut cx, &out, suffix);
        work_items(&mut cx, &out, suffix);
        fleet(&mut cx, &out, suffix);
        projects(&mut cx, &out, suffix);
    }
    cx.update(|cx| crate::zoom::set_level(1.0, cx));

    // Regression #23: gh-x loaded six PRs but could not enrich their supplemental connections.
    let (partial_rows, partial_rows_view) = open(&mut cx, vec![PathBuf::from("/demo/project-manager")], 980.);
    fill(&mut cx, &partial_rows_view, &demo::DemoSource);
    save(&mut cx, partial_rows, &out, "enrichment-row");
    cx.update(|cx| {
        partial_rows_view.update(cx, |view, cx| {
            view.toggle_expanded(Path::new("/demo/project-manager"), cx)
        })
    });
    cx.run_until_parked();
    save(&mut cx, partial_rows, &out, "enrichment-expanded");

    // The collapsed table, twelve rows: clean, attention, a partly unavailable repository and a failing one first.
    let (board, view) = open(&mut cx, demo::checkouts(), 560.);
    fill(&mut cx, &view, &demo::DemoSource);
    save(&mut cx, board, &out, "table");

    cx.update(|cx| view.update(cx, |view, cx| view.cycle_priority_sort(cx)));
    cx.run_until_parked();
    save(&mut cx, board, &out, "priority-highest");
    cx.update(|cx| view.update(cx, |view, cx| view.cycle_priority_sort(cx)));
    cx.run_until_parked();
    save(&mut cx, board, &out, "priority-lowest");
    cx.update(|cx| view.update(cx, |view, cx| view.cycle_priority_sort(cx)));
    cx.run_until_parked();
    save(&mut cx, board, &out, "priority-config");

    cx.update_window(board, |_, window, cx| {
        use dashboard_core::columns::Column;
        use gpui_kit::point;
        window.render_frame(cx);
        let from = window.find(("resize", Column::Priority as usize)).bounds().center();
        window.drag(from, point(from.x + px(48.), from.y), cx);
    })
    .unwrap();
    cx.run_until_parked();
    save(&mut cx, board, &out, "priority-resized");
    cx.update_window(board, |_, window, cx| {
        use dashboard_core::columns::Column;
        window.double_click(("resize", Column::Priority as usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    // The same table sorted by update time, newest first, then oldest first.
    cx.update(|cx| view.update(cx, |view, cx| view.cycle_sort(cx)));
    cx.run_until_parked();
    save(&mut cx, board, &out, "table-sorted");
    cx.update(|cx| view.update(cx, |view, cx| view.cycle_sort(cx)));
    cx.run_until_parked();
    save(&mut cx, board, &out, "table-sorted-oldest");

    // The window at its 480 px minimum: the table can scroll sideways to its controls.
    let (narrow, narrow_view) = open_sized(&mut cx, demo::checkouts(), 480., 560.);
    fill(&mut cx, &narrow_view, &demo::DemoSource);
    save(&mut cx, narrow, &out, "table-narrow");
    cx.update_window(narrow, |_, window, cx| {
        use gpui_kit::{ScrollDelta, point};
        window.scroll("table", ScrollDelta::Pixels(point(px(-2000.), px(0.))), cx);
    })
    .unwrap();
    save(&mut cx, narrow, &out, "table-narrow-scrolled");

    // The table with codexbar expanded: its health rows and its PR, issue, merged-PR and workflow-run tables.
    let (expanded, expanded_view) = open(&mut cx, demo::checkouts(), 980.);
    fill(&mut cx, &expanded_view, &demo::DemoSource);
    cx.update(|cx| expanded_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/codexbar"), cx)));
    cx.run_until_parked();
    save(&mut cx, expanded, &out, "expanded-row");

    // The failing row expanded: its error in place of the last activity and again in its details.
    let (failed, failed_view) = open(&mut cx, demo::checkouts(), 560.);
    fill(&mut cx, &failed_view, &demo::DemoSource);
    cx.update(|cx| failed_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/weather-cli"), cx)));
    cx.run_until_parked();
    save(&mut cx, failed, &out, "error-row");

    // Frames for the expand-and-collapse recording: codexbar opens, hs-buddy opens too, then codexbar closes.
    let (sequence, sequence_view) = open(&mut cx, demo::checkouts(), 820.);
    fill(&mut cx, &sequence_view, &demo::DemoSource);
    save(&mut cx, sequence, &out, "expand-1");
    for (frame, checkout) in [(2, "/demo/codexbar"), (3, "/demo/hs-buddy"), (4, "/demo/codexbar")] {
        cx.update(|cx| sequence_view.update(cx, |view, cx| view.toggle_expanded(Path::new(checkout), cx)));
        cx.run_until_parked();
        save(&mut cx, sequence, &out, &format!("expand-{frame}"));
    }

    // A partly unavailable repository, expanded: the sections gh-x couldn't load say so instead of "None".
    let (partial, partial_view) = open(&mut cx, vec![PathBuf::from("/demo/minibot")], 520.);
    fill(&mut cx, &partial_view, &demo::DemoSource);
    cx.update(|cx| partial_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/minibot"), cx)));
    cx.run_until_parked();
    save(&mut cx, partial, &out, "expanded-unavailable");

    // Without a pmd token: one setup message instead of a failure on every card.
    struct NoToken;
    impl StatusSource for NoToken {
        fn fetch(&self, _: &Path, _: bool) -> Result<StatusDocument, FetchError> {
            Err(FetchError::MissingToken)
        }
    }
    let (setup, setup_view) = open(&mut cx, demo::checkouts(), 520.);
    fill(&mut cx, &setup_view, &NoToken);
    save(&mut cx, setup, &out, "setup-missing-token");

    let (empty, _) = open(&mut cx, vec![], 520.);
    cx.run_until_parked();
    save(&mut cx, empty, &out, "empty-board");

    // pmd rows: ages in words under each name, and dots for blocked, waiting on Franz, failing checks, OK, Board
    // unavailable and a failed refresh.
    let names = ["codexbar", "hs-buddy", "summarist", "gh-x", "minibot", "weather-cli"];
    let (dots, dots_view) = open(
        &mut cx,
        names.iter().map(|name| PathBuf::from("/demo").join(name)).collect(),
        520.,
    );
    fill(&mut cx, &dots_view, &demo::HostedSource);
    save(&mut cx, dots, &out, "pmd-dots");

    // Hosted details: Blocked and Waiting on Franz first, then open PRs and issues in plain words, In progress, and
    // where the data came from, in local time.
    let (hosted, hosted_view) = open(&mut cx, vec![PathBuf::from("/demo/codexbar")], 980.);
    fill(&mut cx, &hosted_view, &demo::HostedSource);
    cx.update(|cx| hosted_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/codexbar"), cx)));
    cx.run_until_parked();
    save(&mut cx, hosted, &out, "pmd-hosted-board");
    cx.update(|cx| {
        hosted_view.update(cx, |view, cx| {
            let mut document = demo::HostedSource.fetch(Path::new("/demo/codexbar"), false).unwrap();
            document.hosted.as_mut().unwrap().last_error = Some(dashboard_core::pmd::LastError {
                message: "Upstream refresh failed; showing last good status".into(),
                at: "2026-10-21T06:25:00Z".into(),
            });
            view.apply(
                Outcome {
                    checkout: PathBuf::from("/demo/codexbar"),
                    result: Ok(document),
                },
                cx,
            );
        })
    });
    cx.run_until_parked();
    save(&mut cx, hosted, &out, "pmd-stale-status");
    cx.update(|cx| {
        hosted_view.update(cx, |view, cx| {
            view.apply(
                Outcome {
                    checkout: PathBuf::from("/demo/codexbar"),
                    result: Err(FetchError::Unavailable),
                },
                cx,
            )
        })
    });
    cx.run_until_parked();
    save(&mut cx, hosted, &out, "pmd-outage-cache");
    cx.update(|cx| {
        hosted_view.update(cx, |view, cx| {
            view.apply(
                Outcome {
                    checkout: PathBuf::from("/demo/codexbar"),
                    result: Err(FetchError::MissingToken),
                },
                cx,
            )
        })
    });
    cx.run_until_parked();
    save(&mut cx, hosted, &out, "pmd-token-cache");

    // Last, so no zoom or toast carries into the other captures.
    // Zoom (#9): the same table with codexbar expanded at 80%, 100% and 150%, in a 1100×760 window like the app's.
    let (zoomed, zoomed_view) = open(&mut cx, demo::checkouts(), 760.);
    fill(&mut cx, &zoomed_view, &demo::DemoSource);
    cx.update(|cx| zoomed_view.update(cx, |view, cx| view.toggle_expanded(Path::new("/demo/codexbar"), cx)));
    for (level, name) in [(0.8, "zoom-80"), (1.0, "zoom-100"), (1.5, "zoom-150")] {
        cx.update(|cx| crate::zoom::set_level(level, cx));
        cx.run_until_parked();
        // Each level's toast shows in the capture, as it does right after the change.
        save(&mut cx, zoomed, &out, name);
    }
}

#[test]
#[ignore = "writes current-head recovery screenshots offscreen"]
fn captures_recovery() {
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").expect("CAPTURE_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    let (handle, view) = open(&mut cx, vec![PathBuf::from("/demo/gh-x")], 520.);
    let fail = |cx: &mut HeadlessAppContext| {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(
                    Outcome {
                        checkout: PathBuf::from("/demo/gh-x"),
                        result: Err(FetchError::Timeout),
                    },
                    cx,
                );
            })
        });
        cx.run_until_parked();
    };
    fail(&mut cx);
    save(&mut cx, handle, &out, "recovery-connecting");
    fill(&mut cx, &view, &demo::HostedSource);
    save(&mut cx, handle, &out, "recovery-normal");
    fail(&mut cx);
    save(&mut cx, handle, &out, "recovery-stale");
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let mut doc = demo::HostedSource.fetch(Path::new("/demo/gh-x"), false).unwrap();
            doc.hosted.as_mut().unwrap().last_error = Some(dashboard_core::pmd::LastError {
                message: "Upstream refresh failed".into(),
                at: "2026-10-25T20:00:00Z".into(),
            });
            view.apply(
                Outcome {
                    checkout: PathBuf::from("/demo/gh-x"),
                    result: Ok(doc),
                },
                cx,
            );
        })
    });
    fail(&mut cx);
    save(&mut cx, handle, &out, "recovery-offline-upstream-error");
}

#[test]
#[ignore = "captures live pmd data for backlog validation"]
fn captures_live_backlog() {
    let Some(token_path) = std::env::var_os("PMD_CAPTURE_TOKEN").filter(|p| !p.is_empty()) else {
        eprintln!("Skipping live backlog capture: PMD_CAPTURE_TOKEN is not set.");
        return;
    };
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").expect("CAPTURE_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let pmd = dashboard_core::priority::Pmd {
        url: "http://100.97.164.73:8790".into(),
        token_path: PathBuf::from(token_path),
    };
    let checkouts = ["project-dashboard", "project-manager", "chief", "gh-x", "skills"]
        .map(|n| PathBuf::from(format!("/home/franz/github/hemsoft/{n}")));
    let documents: Vec<_> = checkouts
        .iter()
        .map(|p| {
            (
                p.clone(),
                pmd.fetch(p, false).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
        })
        .collect();
    let mut cx = headless();
    let (handle, view) = open(&mut cx, checkouts.to_vec(), 850.);
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            for (checkout, doc) in documents {
                v.apply(
                    Outcome {
                        checkout,
                        result: Ok(doc),
                    },
                    cx,
                );
            }
        })
    });
    cx.run_until_parked();
    save(&mut cx, handle, &out, "live-project");
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.selected_view = dashboard_core::config::View::Backlog;
            cx.notify();
        })
    });
    cx.run_until_parked();
    save(&mut cx, handle, &out, "live-backlog");
}

#[test]
#[ignore = "writes deterministic Backlog UI evidence offscreen"]
fn captures_fixture_backlog() {
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").expect("CAPTURE_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    let (handle, view) = open(&mut cx, vec![PathBuf::from("/demo/codexbar")], 600.);
    fill(&mut cx, &view, &demo::HostedSource);
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.selected_view = dashboard_core::config::View::Backlog;
            cx.notify();
        })
    });
    cx.run_until_parked();
    save(&mut cx, handle, &out, "fixture-backlog");
}

#[test]
#[ignore = "writes screenshots; run with CAPTURE_DIR set"]
fn captures_merge_counts() {
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    let (window, view) = open_sized(&mut cx, vec![PathBuf::from("/demo/project-manager")], 1500., 700.);
    fill(&mut cx, &view, &demo::HostedSource);
    settle(&mut cx);
    save(&mut cx, window, &out, "project-merge-counts");
}

#[test]
#[ignore = "writes compact layout screenshots offscreen"]
fn captures_compact_columns() {
    use dashboard_core::columns::Column;
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    let mut options = options();
    for column in Column::ALL {
        options.columns.set(column, column.min_width());
    }
    options.columns.set(Column::Repository, 10.);
    let (window, view) = open_with(&mut cx, demo::checkouts(), 1000., 640., options);
    fill(&mut cx, &view, &demo::HostedSource);
    save(&mut cx, window, &out, "compact-100");
    cx.update(|cx| crate::zoom::set_level(1.3, cx));
    cx.run_until_parked();
    save(&mut cx, window, &out, "compact-130");
}

#[test]
#[ignore = "writes issue 55 screenshots and refresh recording frames offscreen"]
fn captures_live_rows() {
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    cx.update(|cx| {
        crate::zoom::set_level(1.3, cx);
        crate::zoom::hide_toast(cx);
    });
    for width in [480., 1680.] {
        let (window, view) = open_sized(&mut cx, demo::checkouts(), width, 600.);
        fill(&mut cx, &view, &demo::HostedSource);
        settle(&mut cx);
        save(&mut cx, window, &out, &format!("links-{}", width as u32));
    }
    let (window, view) = open_sized(&mut cx, demo::checkouts(), 1680., 600.);
    fill(&mut cx, &view, &demo::HostedSource);
    settle(&mut cx);
    for frame in 0..80 {
        if frame == 15 {
            cx.update(|cx| {
                view.update(cx, |v, cx| {
                    let checkout = PathBuf::from("/demo/codexbar");
                    let mut doc = demo::HostedSource.fetch(&checkout, false).unwrap();
                    doc.hosted.as_mut().unwrap().merge_counts.merged_24h = Some(7);
                    v.apply(
                        Outcome {
                            checkout,
                            result: Ok(doc),
                        },
                        cx,
                    );
                })
            });
        }
        save(&mut cx, window, &out, &format!("refresh-{frame:03}"));
        if frame == 25 {
            save(&mut cx, window, &out, "row-mid-glow");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        cx.run_until_parked();
    }
}

#[test]
#[ignore = "writes narrow expanded detail scrolling evidence offscreen"]
fn captures_expanded_details_scroll() {
    use gpui_kit::{ScrollDelta, point};
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    cx.update(|cx| {
        crate::zoom::set_level(1.3, cx);
        crate::zoom::hide_toast(cx);
    });
    let checkout = PathBuf::from("/demo/codexbar");
    let (window, view) = open_sized(&mut cx, vec![checkout.clone()], 480., 800.);
    fill(&mut cx, &view, &demo::DemoSource);
    settle(&mut cx);
    cx.update(|cx| view.update(cx, |v, cx| v.toggle_expanded(&checkout, cx)));
    save(&mut cx, window, &out, "details-narrow-130-before");
    cx.update_window(window, |_, window, cx| {
        window.scroll(
            ("details-scroll", 0usize),
            ScrollDelta::Pixels(point(px(-3000.), px(0.))),
            cx,
        );
    })
    .unwrap();
    save(&mut cx, window, &out, "details-narrow-130-scrolled");
}

#[test]
#[ignore = "writes issue 57 readiness states and hover evidence offscreen"]
fn captures_project_readiness() {
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    cx.update(|cx| {
        crate::zoom::set_level(1.3, cx);
        crate::zoom::hide_toast(cx);
    });
    let (window, view) = open_sized(&mut cx, crate::ui_tests::readiness_checkouts(), 1680., 850.);
    fill(&mut cx, &view, &crate::ui_tests::ReadinessSource);
    settle(&mut cx);
    save(&mut cx, window, &out, "readiness-reasons-130");
    cx.update_window(window, |_, window, cx| window.hover(("waiting-age", 2usize), cx))
        .unwrap();
    for _ in 0..6 {
        cx.advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    save(&mut cx, window, &out, "readiness-waiting-hover-130");
    cx.update_window(window, |_, window, cx| window.hover("source-freshness", cx))
        .unwrap();
    for _ in 0..6 {
        cx.advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    save(&mut cx, window, &out, "readiness-stale-hover-130");
    let (narrow, view) = open_sized(&mut cx, crate::ui_tests::readiness_checkouts(), 480., 850.);
    fill(&mut cx, &view, &crate::ui_tests::ReadinessSource);
    settle(&mut cx);
    save(&mut cx, narrow, &out, "readiness-narrow-130");
}

#[test]
#[ignore = "writes Fleet view evidence offscreen"]
fn captures_fleet_status() {
    let out = PathBuf::from(std::env::var_os("CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let mut cx = headless();
    for (width, level, suffix) in [
        (1100., 1., "default"),
        (2400., 1., "wide"),
        (480., 1., "narrow"),
        (1100., 1.5, "150"),
    ] {
        cx.update(|cx| {
            crate::zoom::set_level(level, cx);
            crate::zoom::hide_toast(cx);
        });
        let (window, view) = open_sized(&mut cx, vec![], width, 760.);
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                v.selected_view = dashboard_core::config::View::Fleet;
                cx.notify();
            })
        });
        save(&mut cx, window, &out, &format!("fleet-loading-{suffix}"));
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                v.fleet_status = crate::ui_tests::fleet_status_fixture();
                cx.notify();
            })
        });
        save(&mut cx, window, &out, &format!("fleet-populated-{suffix}"));
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                v.fleet_error = Some("pmd request timed out.".into());
                cx.notify();
            })
        });
        save(&mut cx, window, &out, &format!("fleet-retained-error-{suffix}"));
    }
}
