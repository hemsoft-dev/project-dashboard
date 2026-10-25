//! Production HTTP adapter against loopback fixtures only; never connects to the real pmd.
use dashboard_core::{
    Board, Overall,
    cache::StatusCache,
    fixtures,
    pmd::checkout_identity,
    priority::Pmd,
    scheduler::{self, Outcome},
    source::{FetchError, StatusSource},
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
#[path = "support/http.rs"]
mod http;
use http::{Reply, Server};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

const TOKEN: &str = "fixture-read-token";
const REPO: &str = "hemsoft-dev/gh-x";
struct Checkout(PathBuf);
impl Checkout {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dashboard-http-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(path.join(".git")).unwrap();
        std::fs::write(path.join("token"), TOKEN).unwrap();
        let checkout = Self(path);
        checkout.origin(REPO);
        checkout
    }
    fn origin(&self, repo: &str) {
        std::fs::write(
            self.0.join(".git/config"),
            format!("[remote \"origin\"]\n url = git@github.com:{repo}.git\n"),
        )
        .unwrap();
    }
    fn source(&self, server: &Server) -> Pmd {
        Pmd {
            url: server.url.clone(),
            token_path: self.0.join("token"),
        }
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl Reply {
    fn json(value: Value) -> Self {
        Self {
            code: 200,
            body: value.to_string(),
        }
    }
}
fn projects() -> Value {
    json!({"projects":[{"slug":"gh-x","repo":REPO,"tracked":true,"priority":1}]})
}
fn envelope() -> Value {
    let mut status: Value = serde_json::from_str(fixtures::CLEAN).unwrap();
    status.as_object_mut().unwrap().remove("health");
    status["repository"].as_object_mut().unwrap().remove("rootPath");
    json!({"project":"gh-x","generatedAt":"2026-10-25T20:00:00Z","lastError":null,"status":status})
}
fn item(id: &str) -> Value {
    json!({"id":id,"project":"gh-x","kind":"issue","number":31,"title":"API data","url":"https://github.com/hemsoft-dev/gh-x/issues/31","status":"ready"})
}
fn response(path: &str) -> Reply {
    match path {
        "/v1/projects" => Reply::json(projects()),
        "/v1/projects/gh-x/status" => Reply::json(envelope()),
        "/v1/board?project=gh-x" => Reply::json(json!({"generatedAt":"2026-10-25T20:00:00Z","columns":[
            {"status":"ready","title":"Ready","items":[item("stable-1")],"truncated":true},
            {"status":"in_progress","title":"In progress","items":[],"truncated":false},
            {"status":"waiting_on_franz","title":"Waiting on Franz","items":[],"truncated":false},
            {"status":"blocked","title":"Blocked","items":[],"truncated":false}]})),
        "/v1/work?project=gh-x&limit=500" => Reply::json(json!({"items":[item("stable-1")],"nextCursor":"page+2/="})),
        "/v1/work?project=gh-x&limit=500&cursor=page%2B2%2F%3D" => {
            Reply::json(json!({"items":[item("stable-2")],"nextCursor":null}))
        }
        _ => panic!("unexpected API request {path}"),
    }
}
fn apply(
    board: &mut Board,
    cache: &mut StatusCache,
    checkout: &Checkout,
    result: Result<dashboard_core::status::StatusDocument, FetchError>,
) {
    board.apply(
        Outcome {
            checkout: checkout.0.clone(),
            result,
        },
        cache,
    );
}
#[test]
fn hosted_status_preserves_rows_and_board_truncation_and_follows_encoded_work_cursor() {
    let checkout = Checkout::new();
    let server = Server::new(response);
    let source = checkout.source(&server);
    for manual in [false, true] {
        let doc = source.fetch(&checkout.0, manual).unwrap();
        assert!(doc.health.is_none());
        assert!(serde_json::to_value(&doc).unwrap().get("health").is_none());
        let hosted = doc.hosted.as_ref().unwrap();
        assert!(hosted.board.as_ref().unwrap().columns[0].truncated);
        assert_eq!(
            hosted.work.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["stable-1", "stable-2"]
        );
        assert_eq!(hosted.work[1].url, "https://github.com/hemsoft-dev/gh-x/issues/31");
        assert_eq!(doc.issues, fixtures::document(fixtures::CLEAN).issues);
        assert_eq!(doc.pull_requests, fixtures::document(fixtures::CLEAN).pull_requests);
        assert_eq!(doc.sections, fixtures::document(fixtures::CLEAN).sections);
        assert!(
            doc.health_rows()
                .iter()
                .all(|r| r.unavailable && !r.clean && r.text.contains("not supplied by pmd") && !r.text.contains("0 "))
        );
        let mut board = Board::new(std::slice::from_ref(&checkout.0), &StatusCache::default());
        let mut cache = StatusCache::default();
        apply(&mut board, &mut cache, &checkout, Ok(doc));
        assert_eq!(board.cards()[0].overall(), Overall::Unavailable);
    }
    assert_eq!(server.requests.lock().unwrap().len(), 10);
}
#[test]
fn last_error_and_outage_keep_last_good_status_with_original_age_and_explicit_error() {
    let checkout = Checkout::new();
    let failing = Arc::new(AtomicBool::new(false));
    let flag = failing.clone();
    let server = Server::new(move |path| {
        if flag.load(Ordering::SeqCst) {
            return Reply {
                code: 503,
                body: TOKEN.into(),
            };
        }
        let mut reply = response(path);
        if path.ends_with("/status") {
            let mut value = envelope();
            value["lastError"] = json!({"message":format!("upstream unavailable {TOKEN}"),"at":"2026-10-25T20:05:00Z"});
            reply = Reply::json(value);
        }
        reply
    });
    let source = checkout.source(&server);
    let mut cache = StatusCache::default();
    let mut board = Board::new(std::slice::from_ref(&checkout.0), &cache);
    apply(&mut board, &mut cache, &checkout, source.fetch(&checkout.0, false));
    assert!(
        board.cards()[0]
            .error
            .as_ref()
            .unwrap()
            .to_string()
            .contains("upstream unavailable [redacted]")
    );
    let good = board.cards()[0].document.clone().unwrap();
    assert_eq!(good.hosted.as_ref().unwrap().generated_at, "2026-10-25T20:00:00Z");
    failing.store(true, Ordering::SeqCst);
    apply(&mut board, &mut cache, &checkout, source.fetch(&checkout.0, true));
    assert_eq!(board.cards()[0].document.as_ref(), Some(&good));
    assert_eq!(board.cards()[0].error, Some(FetchError::Http(503)));
    assert_eq!(cache.get(&checkout.0), Some(&good));
    cache.save(&checkout.0).unwrap();
    assert!(
        !std::fs::read_to_string(checkout.0.join("status-cache.json"))
            .unwrap()
            .contains(TOKEN)
    );
}
#[test]
fn first_load_failures_are_explicit_and_never_create_clean_or_zero_status() {
    let checkout = Checkout::new();
    for (kind, expected) in [
        ("401", FetchError::Unauthorized),
        ("outage", FetchError::Http(503)),
        ("no_status", FetchError::NoStatus),
        ("malformed", FetchError::Malformed),
        ("unsupported", FetchError::UnsupportedSchema),
        ("missing", FetchError::Malformed),
        ("mismatch", FetchError::IdentityMismatch),
        ("slug", FetchError::IdentityMismatch),
        ("untracked", FetchError::Untracked),
        ("disabled", FetchError::Untracked),
        ("ambiguous", FetchError::Ambiguous),
    ] {
        let server = Server::new(move |path| {
            if path == "/v1/projects" {
                return match kind {
                    "401" => Reply {
                        code: 401,
                        body: TOKEN.into(),
                    },
                    "outage" => Reply {
                        code: 503,
                        body: TOKEN.into(),
                    },
                    "untracked" => {
                        let mut p = projects();
                        p["projects"][0]["repo"] = json!("other-owner/gh-x");
                        Reply::json(p)
                    }
                    "disabled" => {
                        let mut p = projects();
                        p["projects"][0]["tracked"] = json!(false);
                        Reply::json(p)
                    }
                    "ambiguous" => {
                        let mut p = projects();
                        let first = p["projects"][0].clone();
                        p["projects"].as_array_mut().unwrap().push(first);
                        Reply::json(p)
                    }
                    _ => response(path),
                };
            }
            if path.ends_with("/status") {
                let mut value = envelope();
                match kind {
                    "no_status" => {
                        return Reply {
                            code: 404,
                            body: json!({"error":{"code":"no_status","message":TOKEN}}).to_string(),
                        };
                    }
                    "malformed" => {
                        return Reply {
                            code: 200,
                            body: "{invalid".into(),
                        };
                    }
                    "unsupported" => value["status"]["schemaVersion"] = json!(2),
                    "missing" => {
                        value["status"].as_object_mut().unwrap().remove("issues");
                    }
                    "mismatch" => value["status"]["repository"]["nameWithOwner"] = json!("other-owner/gh-x"),
                    "slug" => value["project"] = json!("wrong-slug"),
                    _ => unreachable!(),
                }
                return Reply::json(value);
            }
            panic!("failure must not read Board");
        });
        let source = checkout.source(&server);
        let mut cache = StatusCache::default();
        let mut board = Board::new(std::slice::from_ref(&checkout.0), &cache);
        let result = source.fetch(&checkout.0, true);
        assert_eq!(result, Err(expected.clone()), "{kind}");
        apply(&mut board, &mut cache, &checkout, result);
        assert!(board.cards()[0].document.is_none());
        assert!(cache.is_empty());
        assert_eq!(board.cards()[0].overall(), Overall::Failed);
        assert!(!expected.to_string().contains(TOKEN));
    }
}
#[test]
fn missing_token_timeout_and_connection_failure_are_bounded_and_explicit() {
    let checkout = Checkout::new();
    let server = Server::new(response);
    let mut source = checkout.source(&server);
    source.token_path = checkout.0.join("missing");
    assert_eq!(source.fetch(&checkout.0, false), Err(FetchError::MissingToken));
    assert!(server.requests.lock().unwrap().is_empty());
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let delayed = Server::new(move |path| {
        let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(5));
        response(path)
    });
    let started = Instant::now();
    assert_eq!(
        checkout
            .source(&delayed)
            .fetch_with_timeout(&checkout.0, Duration::from_millis(500)),
        Err(FetchError::Timeout)
    );
    let _ = release.send(());
    assert!(started.elapsed() < Duration::from_secs(3));
    // An accepted connection with an invalid HTTP status avoids OS-specific closed-port retries.
    let broken = Server::new(|_| Reply {
        code: 0,
        body: String::new(),
    });
    let started = Instant::now();
    assert_eq!(
        checkout.source(&broken).fetch(&checkout.0, false),
        Err(FetchError::Unavailable)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(broken.requests.lock().unwrap().len(), 1);
}
#[test]
fn changed_identity_or_service_rejects_delayed_completions_and_old_cache() {
    let checkout = Checkout::new();
    let entered = Arc::new(AtomicBool::new(false));
    let flag = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let server = Server::new(move |path| {
        if path.ends_with("/status") && !flag.swap(true, Ordering::SeqCst) {
            let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(5));
        }
        response(path)
    });
    let source = Arc::new(checkout.source(&server));
    let mut cache = StatusCache::default();
    let mut board = Board::new(std::slice::from_ref(&checkout.0), &cache);
    board.sync_pmd_identities(&source.url, &mut cache);
    let generations = board.start_refresh(std::slice::from_ref(&checkout.0));
    let receive = scheduler::refresh(source.clone(), vec![checkout.0.clone()], 1, false);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !entered.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    checkout.origin("other-owner/gh-x");
    assert_eq!(
        board.sync_pmd_identities(&source.url, &mut cache),
        vec![checkout.0.clone()]
    );
    release.send(()).unwrap();
    let outcome = receive.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(outcome.result, Err(FetchError::IdentityMismatch));
    assert!(!board.apply_started(outcome, generations[&checkout.0], &mut cache));
    assert!(cache.is_empty());
    checkout.origin(REPO);
    board.sync_pmd_identities(&source.url, &mut cache);
    let doc = source.fetch(&checkout.0, false).unwrap();
    apply(&mut board, &mut cache, &checkout, Ok(doc.clone()));
    let generations = board.start_refresh(std::slice::from_ref(&checkout.0));
    board.sync_pmd_identities("http://127.0.0.1:9999", &mut cache);
    assert!(!board.apply_started(
        Outcome {
            checkout: checkout.0.clone(),
            result: Ok(doc)
        },
        generations[&checkout.0],
        &mut cache
    ));
    assert!(cache.is_empty());
    assert!(board.cards()[0].document.is_none());
}
#[test]
fn linked_worktree_identity_uses_common_origin_and_never_invents_a_folder_identity() {
    let checkout = Checkout::new();
    let linked = checkout.0.join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    let gitdir = checkout.0.join(".git/worktrees/linked");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(linked.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    assert_eq!(checkout_identity(&linked), Ok(REPO.into()));
    std::fs::write(checkout.0.join(".git/config"),
        "[remote \"origin\"] # common origin\n url = \"https://github.com/hemsoft-dev/gh-x.git#literal;fragment\" ; primary\n").unwrap();
    for pointer in [gitdir.display().to_string(), "../.git/worktrees/linked".to_owned()] {
        std::fs::write(linked.join(".git"), format!("gitdir: {pointer}\n")).unwrap();
        assert_eq!(checkout_identity(&linked), Ok(REPO.into()));
    }
    std::fs::write(checkout.0.join(".git/config"), "[core]\n bare=false\n").unwrap();
    assert_eq!(checkout_identity(&linked), Err(FetchError::Identity));
    assert_eq!(checkout_identity(Path::new("/not/a/repo")), Err(FetchError::Identity));
}
#[test]
fn repeated_work_cursor_and_board_failure_are_explicit_without_losing_hosted_status() {
    for board_failure in [false, true] {
        let checkout = Checkout::new();
        let server = Server::new(move |path| {
            if board_failure && path.starts_with("/v1/board") {
                return Reply {
                    code: 503,
                    body: String::new(),
                };
            }
            if path.starts_with("/v1/work") {
                return Reply::json(json!({"items":[],"nextCursor":"repeated"}));
            }
            response(path)
        });
        let doc = checkout.source(&server).fetch(&checkout.0, false).unwrap();
        let hosted = doc.hosted.unwrap();
        assert!(hosted.work_error.is_some());
        assert!(hosted.board.is_none());
        assert_eq!(doc.issues, fixtures::document(fixtures::CLEAN).issues);
    }
}
#[test]
fn production_adapter_and_failures_launch_zero_gh_x_processes() {
    if std::env::var_os("DASHBOARD_GH_TRAP_CHILD").is_some() {
        return;
    }
    // Structural assertion covers all targets; removal of the process adapter is deliberate.
    for source in [
        include_str!("../src/source.rs"),
        include_str!("../src/pmd.rs"),
        include_str!("../src/priority.rs"),
    ] {
        assert!(!source.contains("Command::"));
        assert!(!source.contains("std::process"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let trap = Checkout::new();
        let executable = trap.0.join("gh");
        let counter = trap.0.join("launches");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'launch\\n' >> \"$DASHBOARD_GH_COUNTER\"\nexit 99\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![trap.0.clone()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--test-threads=1")
            .env("DASHBOARD_GH_TRAP_CHILD", "1")
            .env("DASHBOARD_GH_COUNTER", &counter)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "fixture child failed: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        let launches = std::fs::read_to_string(counter).unwrap_or_default().lines().count();
        assert_eq!(launches, 0, "production success/failure paths must never launch gh-x");
    }
}

#[test]
fn new_service_config_wins_over_a_delayed_first_load_from_the_old_service() {
    let checkout = Checkout::new();
    let entered = Arc::new(AtomicBool::new(false));
    let flag = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let old = Server::new(move |path| {
        if path.ends_with("/status") {
            flag.store(true, Ordering::SeqCst);
            let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(5));
        }
        response(path)
    });
    let new = Server::new(response);
    let limiter = Arc::new(scheduler::Limited::new(Arc::new(checkout.source(&old)), 2));
    let mut cache = StatusCache::default();
    let mut board = Board::new(std::slice::from_ref(&checkout.0), &cache);
    board.sync_pmd_identities(&old.url, &mut cache);
    let old_generation = board.start_refresh(std::slice::from_ref(&checkout.0))[&checkout.0];
    let pending = scheduler::refresh(limiter.clone(), vec![checkout.0.clone()], 1, false);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !entered.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        board.sync_pmd_identities(&new.url, &mut cache),
        vec![checkout.0.clone()]
    );
    limiter.set_source(Arc::new(checkout.source(&new)));
    let generation = board.start_refresh(std::slice::from_ref(&checkout.0))[&checkout.0];
    let current = scheduler::refresh(limiter, vec![checkout.0.clone()], 1, true)
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert!(board.apply_started(current, generation, &mut cache));
    release.send(()).unwrap();
    let old_result = pending.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(old_result.result.is_ok());
    assert!(!board.apply_started(old_result, old_generation, &mut cache));
    assert_eq!(
        cache.get(&checkout.0).unwrap().hosted.as_ref().unwrap().service_url,
        new.url
    );
}

#[test]
fn board_refresh_failure_retains_last_good_board_and_work_with_original_board_timestamp() {
    let checkout = Checkout::new();
    let failing = Arc::new(AtomicBool::new(false));
    let flag = failing.clone();
    let server = Server::new(move |path| {
        if flag.load(Ordering::SeqCst) && path.starts_with("/v1/board") {
            Reply {
                code: 503,
                body: String::new(),
            }
        } else {
            response(path)
        }
    });
    let source = checkout.source(&server);
    let mut cache = StatusCache::default();
    let mut board = Board::new(std::slice::from_ref(&checkout.0), &cache);
    apply(&mut board, &mut cache, &checkout, source.fetch(&checkout.0, false));
    let old = board.cards()[0].document.as_ref().unwrap().hosted.clone().unwrap();
    failing.store(true, Ordering::SeqCst);
    apply(&mut board, &mut cache, &checkout, source.fetch(&checkout.0, true));
    let current = board.cards()[0].document.as_ref().unwrap().hosted.as_ref().unwrap();
    assert_eq!(current.board, old.board);
    assert_eq!(current.work, old.work);
    assert!(current.work_error.as_ref().unwrap().contains("503"));
}

#[test]
fn a_body_that_stalls_after_headers_is_reported_as_timeout() {
    let checkout = Checkout::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let source = Pmd {
        url: format!("http://{}", listener.local_addr().unwrap()),
        token_path: checkout.0.join("token"),
    };
    let (release, wait) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n")
            .unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(5));
    });
    let start = Instant::now();
    assert_eq!(
        source.fetch_with_timeout(&checkout.0, Duration::from_millis(500)),
        Err(FetchError::Timeout)
    );
    let _ = release.send(());
    assert!(start.elapsed() < Duration::from_secs(3));
    server.join().unwrap();
}

#[test]
fn origin_comments_and_quoted_comment_characters_preserve_hosted_identity_and_cache() {
    let checkout = Checkout::new();
    let server = Server::new(response);
    let source = checkout.source(&server);
    let mut cache = StatusCache::default();
    let mut board = Board::new(std::slice::from_ref(&checkout.0), &cache);
    board.sync_pmd_identities(&source.url, &mut cache);
    apply(&mut board, &mut cache, &checkout, source.fetch(&checkout.0, false));
    let original = board.cards()[0].document.clone();
    for config in [
        "[remote \"origin\"] # GitHub remote\n url = https://github.com/hemsoft-dev/gh-x.git\n",
        "[remote \"origin\"] ; GitHub remote\n url = https://github.com/hemsoft-dev/gh-x.git\n",
        "[remote \"origin\"]\n url = https://github.com/hemsoft-dev/gh-x.git # primary\n",
        "[remote \"origin\"]\n url = https://github.com/hemsoft-dev/gh-x.git ; primary\n",
        "# [remote \"elsewhere\"]\n; url = ignored\n[remote \"origin\"] # primary\n url = \"https://github.com/hemsoft-dev/gh-x.git#literal;fragment\" ; trailing\n",
        "[remote \"origin\"]\n url = \"https://github.com/hemsoft-dev/gh-x.git?literal;query#fragment\" # trailing\n",
    ] {
        std::fs::write(checkout.0.join(".git/config"), config).unwrap();
        assert_eq!(checkout_identity(&checkout.0), Ok(REPO.into()), "{config}");
        assert!(board.sync_pmd_identities(&source.url, &mut cache).is_empty());
        assert_eq!(board.cards()[0].document, original);
        assert_eq!(
            source.fetch(&checkout.0, false).unwrap().repository.name_with_owner,
            REPO
        );
    }
}
