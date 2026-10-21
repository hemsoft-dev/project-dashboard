//! Finding a checkout's T3 Code thread in fixture state databases shaped like T3 server 0.1.1's `statev2.sqlite`.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use dashboard_core::t3::{self, NoThread, STATE_DB, Server, ThreadLink};
use rusqlite::Connection;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("project-dashboard-t3-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SCHEMA: &str = "
    CREATE TABLE projection_projects (
        project_id TEXT PRIMARY KEY, title TEXT, workspace_root TEXT NOT NULL, scripts_json TEXT,
        created_at TEXT, updated_at TEXT, deleted_at TEXT);
    CREATE TABLE orchestration_v2_projection_threads (
        thread_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL, default_provider TEXT,
        runtime_mode TEXT, interaction_mode TEXT, active_provider_thread_id TEXT, created_at TEXT, updated_at TEXT,
        archived_at TEXT, deleted_at TEXT, payload_json TEXT NOT NULL, provider_instance_id TEXT);
    CREATE TABLE orchestration_v2_projection_runs (
        run_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, ordinal INTEGER, provider TEXT, provider_thread_id TEXT,
        status TEXT NOT NULL, requested_at TEXT, completed_at TEXT, payload_json TEXT, provider_instance_id TEXT);
";

/// A thread row: (id, project, title, updated_at, archived, settled, parent thread, run status).
type Row<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    bool,
    bool,
    Option<&'a str>,
    Option<&'a str>,
);

fn database(dir: &Path, projects: &[(&str, &str, bool)], threads: &[Row]) -> PathBuf {
    let path = dir.join(STATE_DB);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(SCHEMA).unwrap();
    for (id, root, deleted) in projects {
        db.execute(
            "INSERT INTO projection_projects (project_id, title, workspace_root, deleted_at) VALUES (?1, ?1, ?2, ?3)",
            (id, root, deleted.then_some("2026-10-01T00:00:00Z")),
        )
        .unwrap();
    }
    for (id, project, title, updated, archived, settled, parent, run) in threads {
        let payload = serde_json::json!({
            "id": id,
            "settledAt": settled.then_some("2026-10-20T00:00:00Z"),
            "lineage": { "parentThreadId": parent },
        });
        db.execute(
            "INSERT INTO orchestration_v2_projection_threads
                 (thread_id, project_id, title, updated_at, archived_at, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id,
                project,
                title,
                updated,
                archived.then_some("2026-10-20T00:00:00Z"),
                payload.to_string(),
            ),
        )
        .unwrap();
        if let Some(status) = run {
            db.execute(
                "INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, status) VALUES (?1, ?2, ?3)",
                (format!("run:{id}"), id, status),
            )
            .unwrap();
        }
    }
    path
}

fn server(dir: &Path) -> Server {
    Server {
        origin: "http://127.0.0.1:3773".to_owned(),
        environment_id: "59edc6cd-b190-4b4f-ab9d-07324e87e11b".to_owned(),
        state_db: dir.join(STATE_DB),
    }
}

const DASHBOARD: &str = "C:/Users/User/github/HemSoft/project-dashboard";

fn lookup(dir: &Path, checkout: &str) -> Result<ThreadLink, NoThread> {
    t3::read(&dir.join(STATE_DB))
        .unwrap()
        .lookup(&server(dir), Path::new(checkout))
}

#[test]
fn the_newest_open_thread_of_the_checkouts_project_is_linked() {
    let scratch = Scratch::new("newest");
    database(
        &scratch.0,
        &[
            ("p1", DASHBOARD, false),
            ("p2", "C:/Users/User/github/HemSoft/gh-x", false),
        ],
        &[
            (
                "t-old",
                "p1",
                "Older",
                "2026-10-21T10:00:00Z",
                false,
                false,
                None,
                Some("completed"),
            ),
            (
                "mcp:t-new",
                "p1",
                "Newest",
                "2026-10-21T12:00:00Z",
                false,
                false,
                None,
                Some("completed"),
            ),
            (
                "t-other",
                "p2",
                "Other repo",
                "2026-10-21T13:00:00Z",
                false,
                false,
                None,
                None,
            ),
        ],
    );
    let link = lookup(&scratch.0, DASHBOARD).unwrap();
    assert_eq!(link.title, "Newest");
    assert!(!link.running);
    // The thread ID is encoded as T3's router expects.
    assert_eq!(
        link.url,
        "http://127.0.0.1:3773/59edc6cd-b190-4b4f-ab9d-07324e87e11b/mcp%3At-new"
    );
}

#[test]
fn a_running_thread_wins_over_a_newer_idle_one() {
    let scratch = Scratch::new("running");
    database(
        &scratch.0,
        &[("p1", DASHBOARD, false)],
        &[
            (
                "t-running",
                "p1",
                "Working",
                "2026-10-21T09:00:00Z",
                false,
                false,
                None,
                Some("running"),
            ),
            (
                "t-newer",
                "p1",
                "Newer",
                "2026-10-21T12:00:00Z",
                false,
                false,
                None,
                Some("completed"),
            ),
        ],
    );
    let link = lookup(&scratch.0, DASHBOARD).unwrap();
    assert_eq!(link.title, "Working");
    assert!(link.running);
}

#[test]
fn a_folder_registered_twice_links_the_best_thread_across_both_projects() {
    let scratch = Scratch::new("duplicate");
    database(
        &scratch.0,
        &[
            ("p1", DASHBOARD, false),
            ("p2", DASHBOARD.replace('/', "\\").as_str(), false),
        ],
        &[
            ("t-old", "p1", "Older", "2026-10-21T08:00:00Z", false, false, None, None),
            (
                "t-new",
                "p2",
                "Newer, other project",
                "2026-10-21T12:00:00Z",
                false,
                false,
                None,
                None,
            ),
        ],
    );
    assert_eq!(lookup(&scratch.0, DASHBOARD).unwrap().title, "Newer, other project");
    // A running thread in either project still comes first, even when it is older.
    let scratch = Scratch::new("duplicate-running");
    database(
        &scratch.0,
        &[("p1", DASHBOARD, false), ("p2", DASHBOARD, false)],
        &[
            (
                "t-new",
                "p1",
                "Newest",
                "2026-10-21T12:00:00Z",
                false,
                false,
                None,
                None,
            ),
            (
                "t-run-old",
                "p2",
                "Running, older",
                "2026-10-21T07:00:00Z",
                false,
                false,
                None,
                Some("running"),
            ),
            (
                "t-run-new",
                "p2",
                "Running, newer",
                "2026-10-21T09:00:00Z",
                false,
                false,
                None,
                Some("running"),
            ),
        ],
    );
    assert_eq!(lookup(&scratch.0, DASHBOARD).unwrap().title, "Running, newer");
}

#[test]
fn settled_archived_and_subagent_threads_are_skipped() {
    let scratch = Scratch::new("skipped");
    database(
        &scratch.0,
        &[("p1", DASHBOARD, false)],
        &[
            (
                "t-settled",
                "p1",
                "Settled",
                "2026-10-21T15:00:00Z",
                false,
                true,
                None,
                None,
            ),
            (
                "t-archived",
                "p1",
                "Archived",
                "2026-10-21T14:00:00Z",
                true,
                false,
                None,
                None,
            ),
            (
                "t-child",
                "p1",
                "Subagent",
                "2026-10-21T13:00:00Z",
                false,
                false,
                Some("t-open"),
                Some("running"),
            ),
            ("t-open", "p1", "Open", "2026-10-21T08:00:00Z", false, false, None, None),
        ],
    );
    assert_eq!(lookup(&scratch.0, DASHBOARD).unwrap().title, "Open");
}

#[test]
fn a_checkout_without_a_project_or_an_open_thread_says_so() {
    let scratch = Scratch::new("none");
    database(
        &scratch.0,
        &[("p1", DASHBOARD, false), ("p-deleted", "C:/src/deleted", true)],
        &[(
            "t-settled",
            "p1",
            "Settled",
            "2026-10-21T15:00:00Z",
            false,
            true,
            None,
            None,
        )],
    );
    assert_eq!(lookup(&scratch.0, DASHBOARD), Err(NoThread::NoOpenThread));
    assert_eq!(lookup(&scratch.0, "C:/src/elsewhere"), Err(NoThread::NoProject));
    // A deleted project doesn't count.
    assert_eq!(lookup(&scratch.0, "C:/src/deleted"), Err(NoThread::NoProject));
}

#[test]
fn paths_match_across_separators_and_a_trailing_slash() {
    assert!(t3::same_path(
        Path::new(r"C:\Users\User\github\HemSoft\project-dashboard"),
        Path::new("C:/Users/User/github/HemSoft/project-dashboard/")
    ));
    assert!(!t3::same_path(Path::new("/src/a"), Path::new("/src/ab")));
    // Windows paths are case-insensitive; elsewhere case matters.
    assert_eq!(
        t3::same_path(Path::new("C:/Src/Repo"), Path::new("c:/src/repo")),
        cfg!(windows)
    );
}

#[test]
fn an_unexpected_schema_is_a_reason_not_a_panic() {
    let scratch = Scratch::new("schema");
    let db = Connection::open(scratch.0.join(STATE_DB)).unwrap();
    db.execute_batch("CREATE TABLE projection_projects (project_id TEXT, workspace_root TEXT, deleted_at TEXT);")
        .unwrap();
    drop(db);
    match t3::read(&scratch.0.join(STATE_DB)) {
        Err(NoThread::Unreadable(reason)) => {
            assert!(reason.contains("orchestration_v2_projection_threads"), "{reason}")
        }
        other => panic!("expected unreadable, got {other:?}"),
    }
    // A file that isn't a database at all.
    std::fs::write(scratch.0.join(STATE_DB), "not sqlite").unwrap();
    assert!(matches!(
        t3::read(&scratch.0.join(STATE_DB)),
        Err(NoThread::Unreadable(_))
    ));
}

#[test]
fn reading_never_writes_to_t3s_database() {
    let scratch = Scratch::new("readonly");
    let path = database(
        &scratch.0,
        &[("p1", DASHBOARD, false)],
        &[("t1", "p1", "Open", "2026-10-21T08:00:00Z", false, false, None, None)],
    );
    let before = std::fs::read(&path).unwrap();
    t3::read(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // No journal or WAL file appeared next to it either.
    let names: Vec<_> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [STATE_DB]);
}

fn userdata(dir: &Path, origin: &str) {
    std::fs::write(
        dir.join("server-runtime.json"),
        format!(r#"{{"version":1,"origin":"{origin}"}}"#),
    )
    .unwrap();
    std::fs::write(dir.join("environment-id"), "env-1\n").unwrap();
}

#[test]
fn find_reports_why_for_every_checkout_when_t3_is_missing_or_stopped() {
    let checkouts = [PathBuf::from(DASHBOARD), PathBuf::from("C:/src/other")];
    let scratch = Scratch::new("missing");
    assert_eq!(
        t3::find(&scratch.0, &checkouts),
        [Err(NoThread::NotInstalled), Err(NoThread::NotInstalled)]
    );
    database(&scratch.0, &[("p1", DASHBOARD, false)], &[]);
    // No runtime file: the server isn't running.
    assert_eq!(t3::find(&scratch.0, &checkouts)[0], Err(NoThread::NotRunning));
    // A runtime file for a server that has gone away.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    userdata(&scratch.0, &format!("http://127.0.0.1:{port}"));
    assert_eq!(t3::find(&scratch.0, &checkouts)[0], Err(NoThread::NotRunning));
}

#[test]
fn find_links_each_checkout_while_the_server_is_listening() {
    let scratch = Scratch::new("listening");
    database(
        &scratch.0,
        &[("p1", DASHBOARD, false)],
        &[(
            "t1",
            "p1",
            "Open",
            "2026-10-21T08:00:00Z",
            false,
            false,
            None,
            Some("running"),
        )],
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    userdata(&scratch.0, &format!("http://{}/", listener.local_addr().unwrap()));
    let found = t3::find(&scratch.0, &[PathBuf::from(DASHBOARD), PathBuf::from("C:/src/other")]);
    let link = found[0].as_ref().unwrap();
    assert_eq!(link.url, format!("http://{}/env-1/t1", listener.local_addr().unwrap()));
    assert_eq!(found[1], Err(NoThread::NoProject));
}

#[test]
fn homes_are_searched_from_the_setting_to_t3s_defaults() {
    let home = Path::new("/home/franz");
    assert_eq!(
        t3::candidate_homes(Some(Path::new("/cfg/t3")), Some(Path::new("/env/t3")), home),
        [
            PathBuf::from("/cfg/t3"),
            PathBuf::from("/env/t3"),
            home.join(".t3"),
            home.join(".t3-server"),
        ]
    );
    assert_eq!(
        t3::candidate_homes(None, None, home),
        [home.join(".t3"), home.join(".t3-server")]
    );
}

#[test]
fn the_home_with_a_running_server_wins() {
    let scratch = Scratch::new("homes");
    let default = scratch.0.join(".t3");
    let server = scratch.0.join(".t3-server");
    // A stock ~/.t3 with only logs (as on this laptop), and a --base-dir home with the server's files.
    std::fs::create_dir_all(default.join("userdata").join("logs")).unwrap();
    std::fs::create_dir_all(server.join("userdata")).unwrap();
    database(&server.join("userdata"), &[], &[]);
    let homes = [default.clone(), server.clone()];
    assert_eq!(t3::resolve_userdata(&homes), server.join("userdata"));
    // When both have a database, the one with a running server's runtime file wins, wherever it is in the order.
    database(&default.join("userdata"), &[], &[]);
    assert_eq!(t3::resolve_userdata(&homes), default.join("userdata"));
    userdata(&server.join("userdata"), "http://127.0.0.1:3773");
    assert_eq!(t3::resolve_userdata(&homes), server.join("userdata"));
    // Nothing anywhere: the first home, which find() then reports as not installed.
    let empty = [scratch.0.join("nothing")];
    assert_eq!(t3::resolve_userdata(&empty), scratch.0.join("nothing").join("userdata"));
}

#[test]
fn a_stale_home_is_passed_over_for_the_running_server() {
    let scratch = Scratch::new("stale");
    let stale = scratch.0.join(".t3").join("userdata");
    let live = scratch.0.join(".t3-server").join("userdata");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::create_dir_all(&live).unwrap();
    // The first home still has a database and a runtime file, but its server has gone.
    database(
        &stale,
        &[("p-old", DASHBOARD, false)],
        &[(
            "t-old",
            "p-old",
            "Stale",
            "2026-10-21T08:00:00Z",
            false,
            false,
            None,
            None,
        )],
    );
    let gone = TcpListener::bind("127.0.0.1:0").unwrap();
    let gone_port = gone.local_addr().unwrap().port();
    drop(gone);
    userdata(&stale, &format!("http://127.0.0.1:{gone_port}"));
    database(
        &live,
        &[("p1", DASHBOARD, false)],
        &[("t-live", "p1", "Live", "2026-10-21T09:00:00Z", false, false, None, None)],
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    userdata(&live, &format!("http://{}", listener.local_addr().unwrap()));
    let homes = [scratch.0.join(".t3"), scratch.0.join(".t3-server")];
    let found = t3::find_in_homes(&homes, &[PathBuf::from(DASHBOARD)]);
    assert_eq!(found[0].as_ref().unwrap().title, "Live");
    // With no server listening anywhere, the reason is that T3 isn't running.
    drop(listener);
    assert_eq!(
        t3::find_in_homes(&homes, &[PathBuf::from(DASHBOARD)]),
        [Err(NoThread::NotRunning)]
    );
}

#[test]
fn reasons_read_as_tooltips() {
    assert_eq!(NoThread::NotRunning.to_string(), "T3 Code isn't running");
    assert_eq!(
        NoThread::NoOpenThread.to_string(),
        "No open T3 Code thread for this checkout"
    );
}

#[test]
fn the_button_tooltip_names_the_thread_or_the_reason() {
    let link = |running| {
        Ok(ThreadLink {
            title: "gh-x: release".to_owned(),
            url: String::new(),
            running,
        })
    };
    assert_eq!(t3::tooltip(&link(false)), "Open the T3 Code thread: gh-x: release");
    assert_eq!(
        t3::tooltip(&link(true)),
        "Open the running T3 Code thread: gh-x: release"
    );
    assert_eq!(
        t3::tooltip(&Err(NoThread::NotInstalled)),
        "T3 Code isn't installed on this computer"
    );
}
