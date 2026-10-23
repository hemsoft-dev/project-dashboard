use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashboard_core::cache::StatusCache;
use dashboard_core::config::Config;
use dashboard_core::priority::{DEFAULT_URL, Pmd, Priorities, PrioritySource};
use dashboard_core::{Board, Card, SortOrder, fixtures, scheduler::Outcome};

const PROJECTS: &str = r#"{"projects":[
 {"slug":"alpha","priority":3,"tracked":true},
 {"slug":"beta","priority":0,"tracked":true},
 {"slug":"gamma","priority":3,"tracked":true},
 {"slug":"untracked","priority":1,"tracked":false},
 {"slug":"untiered","priority":null,"tracked":true}] }"#;

struct Token(PathBuf);
impl Token {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dashboard-pmd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, " test-read-token\n").unwrap();
        Self(path)
    }
}
impl Drop for Token {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn server(status: &str, body: &str, delay: Duration) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/secret\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/projects http/1.1\r\n"));
        assert!(request.contains("authorization: bearer test-read-token\r\n"));
        std::thread::sleep(delay);
        let _ = stream.write_all(response.as_bytes());
    });
    (url, thread)
}

#[test]
fn authenticated_projects_match_repo_names_and_omit_untracked_or_untiered_projects() {
    let token = Token::new();
    let (url, server) = server("200 OK", PROJECTS, Duration::ZERO);
    let priorities = Pmd {
        url: format!("{url}/"),
        token_path: token.0.clone(),
    }
    .fetch();
    server.join().unwrap();
    for (name, expected) in [
        ("alpha", Some(3)),
        ("BETA", Some(0)),
        ("untracked", None),
        ("untiered", None),
        ("missing", None),
    ] {
        assert_eq!(priorities.for_card(&Card::new(PathBuf::from(name))), expected);
    }
    // gh-x's canonical identity wins over an arbitrarily named checkout directory.
    let mut card = Card::new("renamed-folder".into());
    let mut document = fixtures::document(fixtures::CLEAN);
    document.repository.name_with_owner = "hemsoft-dev/beta".to_owned();
    card.document = Some(document);
    assert_eq!(priorities.for_card(&card), Some(0));
}

#[test]
fn failed_auth_redirect_malformed_and_oversized_responses_clear_priorities() {
    let token = Token::new();
    for (status, body) in [
        ("401 Unauthorized", PROJECTS.to_owned()),
        ("302 Found", PROJECTS.to_owned()),
        ("200 OK", "invalid".to_owned()),
        ("200 OK", " ".repeat(1024 * 1024 + 1)),
    ] {
        let (url, server) = server(status, &body, Duration::ZERO);
        assert_eq!(
            Pmd {
                url,
                token_path: token.0.clone()
            }
            .fetch(),
            Priorities::default()
        );
        server.join().unwrap();
    }
}

#[test]
fn missing_empty_tokens_and_unreachable_services_leave_priority_unknown() {
    let token = Token::new();
    let pmd = Pmd {
        url: "http://127.0.0.1:1".into(),
        token_path: token.0.clone(),
    };
    assert_eq!(pmd.fetch(), Priorities::default());
    std::fs::write(&token.0, "\n ").unwrap();
    assert_eq!(pmd.fetch(), Priorities::default());
    std::fs::remove_file(&token.0).unwrap();
    assert_eq!(pmd.fetch(), Priorities::default());
}

#[test]
fn a_slow_service_has_a_bounded_request_time() {
    let token = Token::new();
    let (url, server) = server("200 OK", PROJECTS, Duration::from_millis(2400));
    let start = Instant::now();
    assert_eq!(
        Pmd {
            url,
            token_path: token.0.clone()
        }
        .fetch(),
        Priorities::default()
    );
    assert!(start.elapsed() < Duration::from_millis(2350));
    server.join().unwrap();
}

#[test]
fn priority_sort_keeps_ties_and_unknowns_in_config_order_in_both_directions() {
    let paths: Vec<PathBuf> = ["missing", "alpha", "untracked", "beta", "gamma", "untiered"]
        .map(PathBuf::from)
        .into();
    let mut board = Board::new(&paths, &StatusCache::default());
    board.set_priorities(Priorities::parse(PROJECTS.as_bytes()).unwrap());
    assert_eq!(board.order(SortOrder::PriorityFirst), [3, 1, 4, 0, 2, 5]);
    assert_eq!(board.order(SortOrder::PriorityLast), [1, 4, 3, 0, 2, 5]);
    assert_eq!(board.order(SortOrder::Config), [0, 1, 2, 3, 4, 5]);
    // Status refreshes don't overwrite priority; priority failures don't change gh-x health.
    let mut cache = StatusCache::default();
    let mut document = fixtures::document(fixtures::CLEAN);
    document.repository.name_with_owner = "hemsoft-dev/beta".into();
    board.apply(
        Outcome {
            checkout: "beta".into(),
            result: Ok(document),
        },
        &mut cache,
    );
    let card = board.card(Path::new("beta")).unwrap().clone();
    assert_eq!(board.priority(&card), Some(0));
    board.set_priorities(Priorities::default());
    assert_eq!(board.card(Path::new("beta")), Some(&card));
    assert_eq!(board.priority(&card), None);
}

#[test]
fn old_configs_default_pmd_and_custom_token_paths_resolve_beside_config() {
    let config = Config::parse("", Path::new("config.toml")).unwrap();
    assert_eq!(config.pmd_url, DEFAULT_URL);
    assert_eq!(
        Pmd::from_config(&config, Path::new("settings")).token_path,
        Path::new("settings/pmd-token")
    );
    let config = Config::parse(
        "pmd_url = 'http://localhost:8790'\npmd_token_path = 'secrets/read-token'",
        Path::new("config.toml"),
    )
    .unwrap();
    let pmd = Pmd::from_config(&config, Path::new("settings"));
    assert_eq!(pmd.url, "http://localhost:8790");
    assert_eq!(pmd.token_path, Path::new("settings/secrets/read-token"));
    assert_eq!(
        Config::parse(&config.to_toml(), Path::new("config.toml")).unwrap(),
        config
    );
    let absolute = std::env::temp_dir().join("pmd-read-token");
    let config = Config {
        pmd_token_path: absolute.clone(),
        ..config
    };
    assert_eq!(Pmd::from_config(&config, Path::new("settings")).token_path, absolute);
}
