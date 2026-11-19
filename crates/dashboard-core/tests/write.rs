//! The write client against a loopback stub only; never connects to the real pmd.
use dashboard_core::config::Config;
use dashboard_core::edit::{Form, Next, Row};
use dashboard_core::write::{
    Access, CreateCheck, Feature, MAX_BODY, Method, PmdWriter, Precondition, WriteBackend, WriteError, WriteRequest,
    entry_state, load_token, store_token, write_state,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "support/stub.rs"]
mod stub;
use stub::{Dir, READ, Reply, Stub, WRITE};

fn patch(version: u64) -> WriteRequest {
    WriteRequest {
        method: Method::Patch,
        path: "v1/work/01JBX".into(),
        precondition: Precondition::IfMatch(version),
        body: json!({"status": "blocked", "statusReason": "needs review"}),
    }
}

fn create() -> WriteRequest {
    WriteRequest {
        method: Method::Put,
        path: "v1/machines/air".into(),
        precondition: Precondition::IfNoneMatch,
        body: json!({"os": "macos", "tailnetIp": null, "maxSessions": 2, "capabilities": ["apple"]}),
    }
}

#[test]
fn patch_sends_if_match_json_and_the_write_token_and_returns_the_row() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply::json(200, json!({"id": "01JBX", "version": 5, "status": "blocked"})));
    let row = dir.writer(&stub).write(&patch(4)).unwrap();
    assert_eq!(row["version"], 5);
    let [request] = stub.requests().try_into().unwrap();
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.path, "/v1/work/01JBX");
    assert_eq!(request.header("if-match"), Some("4"));
    assert_eq!(request.header("if-none-match"), None);
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.token(), WRITE);
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body, json!({"status": "blocked", "statusReason": "needs review"}));
}

#[test]
fn create_sends_if_none_match_star_with_put() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply::json(201, json!({"name": "air", "version": 1})));
    dir.writer(&stub).write(&create()).unwrap();
    let [request] = stub.requests().try_into().unwrap();
    assert_eq!(request.method, "PUT");
    assert_eq!(request.header("if-none-match"), Some("*"));
    assert_eq!(request.header("if-match"), None);
}

#[test]
fn whoami_uses_the_write_token_and_reads_name_and_scopes() {
    let dir = Dir::new();
    let stub = Stub::new(|request| {
        assert_eq!((request.method.as_str(), request.path.as_str()), ("GET", "/v1/whoami"));
        assert_eq!(request.token(), WRITE);
        Reply::json(
            200,
            json!({"name": "project-dashboard@home", "scopes": ["read", "write:work"]}),
        )
    });
    let whoami = dir.writer(&stub).whoami().unwrap();
    assert_eq!(whoami.name, "project-dashboard@home");
    assert!(whoami.allows(Feature::WorkItems));
    assert!(!whoami.allows(Feature::Fleet));
    assert!(!whoami.allows(Feature::Projects));
}

#[test]
fn reread_after_a_conflict_uses_the_read_token() {
    let dir = Dir::new();
    let stub = Stub::new(|request| {
        assert_eq!(request.method, "GET");
        assert_eq!(request.token(), READ);
        Reply::json(200, json!({"id": "01JBX", "version": 6}))
    });
    assert_eq!(dir.writer(&stub).read("v1/work/01JBX").unwrap()["version"], 6);
}

#[test]
fn every_pmd_status_maps_to_its_typed_error() {
    let cases: Vec<(Reply, WriteError)> = vec![
        (
            Reply::error(400, "bad_request", "maxSessions must be 0-64"),
            WriteError::BadRequest {
                code: "bad_request".into(),
                message: "maxSessions must be 0-64".into(),
            },
        ),
        (
            Reply::error(400, "unknown_machine", "machine \"x\" does not exist"),
            WriteError::BadRequest {
                code: "unknown_machine".into(),
                message: "machine \"x\" does not exist".into(),
            },
        ),
        (Reply::error(401, "unauthorized", "bad token"), WriteError::Unauthorized),
        (
            Reply::error(403, "insufficient_scope", "this token lacks the write:fleet scope"),
            WriteError::InsufficientScope {
                message: "this token lacks the write:fleet scope".into(),
            },
        ),
        (Reply::error(404, "not_found", "no such item"), WriteError::NotFound),
        (
            Reply::error(409, "stale_version", "If-Match is 4 but 5"),
            WriteError::StaleVersion,
        ),
        (Reply::error(409, "already_exists", "exists"), WriteError::AlreadyExists),
        (Reply::error(409, "item_closed", "closed"), WriteError::ItemClosed),
        (
            Reply::error(409, "not_tracked", "gh-x is not tracked"),
            WriteError::Conflict {
                code: "not_tracked".into(),
                message: "gh-x is not tracked".into(),
            },
        ),
        (Reply::error(413, "too_large", "body over 64 KB"), WriteError::TooLarge),
        (
            Reply::error(428, "precondition_required", "If-Match required"),
            WriteError::PreconditionRequired,
        ),
        (
            Reply {
                headers: vec![("Retry-After", "42".into())],
                ..Reply::error(429, "rate_limited", "one sync per minute")
            },
            WriteError::RateLimited { retry_after: Some(42) },
        ),
        (
            Reply::error(429, "rate_limited", "one sync per minute"),
            WriteError::RateLimited { retry_after: None },
        ),
        (
            Reply::error(503, "github_unavailable", "GitHub is down"),
            WriteError::Server {
                status: 503,
                code: "github_unavailable".into(),
                message: "GitHub is down".into(),
            },
        ),
        (
            Reply {
                code: 502,
                body: "<html>bad gateway</html>".into(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            WriteError::Http(502),
        ),
        (
            Reply {
                code: 200,
                body: "not json".into(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            WriteError::Malformed,
        ),
    ];
    let dir = Dir::new();
    for (reply, expected) in cases {
        let reply = Arc::new(Mutex::new(Some(reply)));
        let stub = Stub::new(move |_| reply.lock().unwrap().take().unwrap());
        assert_eq!(dir.writer(&stub).write(&patch(4)), Err(expected));
        // Never retried automatically.
        assert_eq!(stub.requests().len(), 1);
    }
}

#[test]
fn network_failure_and_a_closed_port_are_unavailable() {
    let dir = Dir::new();
    // Port 0: nothing can listen there, so the connection fails at once (a freed port could go to a parallel stub).
    let port = 0;
    let writer = PmdWriter {
        url: format!("http://127.0.0.1:{port}"),
        ..dir.writer(&Stub::new(|_| unreachable!()))
    };
    assert_eq!(writer.write(&patch(1)), Err(WriteError::Network));
}

#[test]
fn a_timed_out_edit_is_never_resent() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply {
        delay: Duration::from_millis(600),
        ..Reply::json(200, json!({"version": 2}))
    });
    let writer = PmdWriter {
        timeout: Duration::from_millis(200),
        ..dir.writer(&stub)
    };
    assert_eq!(writer.write(&patch(1)), Err(WriteError::Timeout { create: None }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn a_timed_out_create_reads_the_row_before_offering_a_retry() {
    for (status, expected) in [
        (200, CreateCheck::Exists),
        (404, CreateCheck::Absent),
        (500, CreateCheck::Unknown),
    ] {
        let dir = Dir::new();
        let stub = Stub::new(move |request| match request.method.as_str() {
            "PUT" => Reply {
                delay: Duration::from_millis(600),
                ..Reply::json(201, json!({"version": 1}))
            },
            _ => {
                assert_eq!(request.token(), READ);
                assert_eq!(request.path, "/v1/machines/air");
                Reply::error(status, "x", "y")
            }
        });
        let writer = PmdWriter {
            timeout: Duration::from_millis(200),
            ..dir.writer(&stub)
        };
        assert_eq!(
            writer.write(&create()),
            Err(WriteError::Timeout { create: Some(expected) })
        );
        std::thread::sleep(Duration::from_millis(500));
        let methods: Vec<String> = stub.requests().into_iter().map(|r| r.method).collect();
        assert_eq!(methods, ["PUT", "GET"], "one create, one check, no retry");
    }
}

#[test]
fn redirects_are_not_followed() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply {
        headers: vec![("Location", "http://127.0.0.1:1/v1/work/01JBX".into())],
        ..Reply::error(307, "", "")
    });
    assert_eq!(dir.writer(&stub).write(&patch(1)), Err(WriteError::Http(307)));
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn a_body_over_64_kb_is_refused_without_sending() {
    let dir = Dir::new();
    let stub = Stub::new(|_| unreachable!("never sent"));
    let mut request = patch(1);
    request.body = json!({"statusReason": "x".repeat(MAX_BODY)});
    assert_eq!(dir.writer(&stub).write(&request), Err(WriteError::TooLarge));
    assert!(stub.requests().is_empty());
}

#[test]
fn a_response_over_the_limit_is_malformed() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply::json(200, json!({"version": 1, "pad": "x".repeat(1024 * 1024 + 1)})));
    assert_eq!(dir.writer(&stub).write(&patch(1)), Err(WriteError::Malformed));
}

#[test]
fn paths_outside_the_api_are_refused() {
    let dir = Dir::new();
    let stub = Stub::new(|_| unreachable!("never sent"));
    for path in [
        "/v1/work/x",
        "v1/../admin",
        "v1/agents/mini/./x",
        "v1/agents//x",
        "v1/agents/..\\..\\v1\\projects\\x",
        "v1/agents/%2e%2e/x",
        "v1/work/x?all=1",
        "healthz",
        "http://evil/v1/x",
    ] {
        let request = WriteRequest {
            path: path.into(),
            ..patch(1)
        };
        assert_eq!(dir.writer(&stub).write(&request), Err(WriteError::Malformed), "{path}");
    }
}

#[test]
fn writes_are_off_without_a_write_token_path_and_missing_tokens_say_so() {
    let dir = Dir::new();
    let stub = Stub::new(|_| unreachable!("never sent"));
    let off = PmdWriter {
        write_token_path: None,
        ..dir.writer(&stub)
    };
    assert_eq!(off.token_path(), None);
    assert_eq!(off.write(&patch(1)), Err(WriteError::WritesOff));
    assert_eq!(off.whoami(), Err(WriteError::WritesOff));
    assert_eq!(off.store_token("abc"), Err(WriteError::WritesOff));
    let missing = PmdWriter {
        write_token_path: Some(dir.0.join("absent")),
        ..dir.writer(&stub)
    };
    assert_eq!(missing.whoami(), Err(WriteError::MissingToken));
    std::fs::write(dir.0.join("blank"), "  \n").unwrap();
    let blank = PmdWriter {
        write_token_path: Some(dir.0.join("blank")),
        ..dir.writer(&stub)
    };
    assert_eq!(blank.write(&patch(1)), Err(WriteError::MissingToken));
    assert!(stub.requests().is_empty());
}

/// Every error path, with pmd echoing both tokens in codes, messages and headers: neither token survives in any
/// error's Display or Debug, or in anything a form shows.
#[test]
fn tokens_never_appear_in_any_error() {
    let dir = Dir::new();
    let echo = format!("{WRITE} and {READ}");
    let statuses = [400, 401, 403, 404, 409, 413, 428, 429, 500, 502, 503, 504, 307, 418];
    let codes = [
        "bad_request",
        "stale_version",
        "already_exists",
        "item_closed",
        "not_tracked",
        "rate_limited",
        "github_unavailable",
    ];
    let mut errors = Vec::new();
    for status in statuses {
        for code in codes {
            let (echo, code) = (echo.clone(), code.to_owned());
            let stub = Stub::new(move |_| Reply {
                headers: vec![("Retry-After", echo.clone()), ("X-Echo", echo.clone())],
                ..Reply::error(
                    status,
                    &format!("{code}{echo}"),
                    &format!("maxSessions {echo}\nline two {echo}"),
                )
            });
            let writer = dir.writer(&stub);
            errors.push(writer.write(&patch(1)).unwrap_err());
            errors.push(writer.write(&create()).unwrap_err());
            errors.push(writer.whoami().unwrap_err());
            errors.push(writer.read("v1/work/x").unwrap_err());
        }
    }
    // Timeouts, a closed port and a malformed success too.
    let slow = Stub::new(|_| Reply {
        delay: Duration::from_millis(400),
        ..Reply::json(200, json!({"version": 1}))
    });
    let writer = PmdWriter {
        timeout: Duration::from_millis(100),
        ..dir.writer(&slow)
    };
    errors.push(writer.write(&patch(1)).unwrap_err());
    errors.push(writer.write(&create()).unwrap_err());
    // Port 0: nothing can listen there, so the connection fails at once (a freed port could go to a parallel stub).
    let port = 0;
    errors.push(
        PmdWriter {
            url: format!("http://{WRITE}@127.0.0.1:{port}"),
            ..dir.writer(&slow)
        }
        .write(&patch(1))
        .unwrap_err(),
    );
    errors.push(
        PmdWriter {
            url: format!("http://127.0.0.1:{port}/{WRITE}"),
            ..dir.writer(&slow)
        }
        .write(&patch(1))
        .unwrap_err(),
    );
    let garbage = Stub::new(move |_| Reply {
        code: 200,
        body: format!("{{\"version\": \"{WRITE}\""),
        headers: Vec::new(),
        delay: Duration::ZERO,
    });
    errors.push(dir.writer(&garbage).write(&patch(1)).unwrap_err());
    errors.push(store_token(&dir.0.join("t"), &format!("{WRITE}\n{READ}")).unwrap_err());

    let mut form = Form::edit(
        Row {
            version: 1,
            values: BTreeMap::from([("maxSessions".into(), "2".into())]),
        },
        &["maxSessions"],
    );
    assert!(errors.len() > statuses.len() * codes.len() * 4);
    for error in &errors {
        form.begin_save();
        form.failed(error);
        let shown = format!(
            "{error} {error:?} {} {} {:?}",
            form.message().unwrap_or_default(),
            form.field_error("maxSessions").unwrap_or_default(),
            error.message()
        );
        assert!(!shown.contains(WRITE) && !shown.contains(READ), "{shown}");
        assert!(!shown.contains('\n'), "one line: {shown}");
    }
}

#[test]
fn a_400_naming_a_field_belongs_to_that_field() {
    let error = |message: &str| WriteError::BadRequest {
        code: "bad_request".into(),
        message: message.into(),
    };
    let fields = ["maxSessions", "tailnetIp", "roles", "priority"];
    assert_eq!(error("maxSessions must be 0-64").field(&fields), Some("maxSessions"));
    assert_eq!(
        error("tailnetIp \"1.2.3.4\" is not a Tailscale address").field(&fields),
        Some("tailnetIp")
    );
    assert_eq!(error("roles: \"X\" must be lower-case").field(&fields), Some("roles"));
    assert_eq!(error("unknown field \"bogus\"").field(&fields), None);
    assert_eq!(error("PUT takes no query parameters").field(&fields), None);
    assert_eq!(WriteError::NotFound.field(&fields), None);
}

#[test]
fn stored_tokens_are_owner_only_trimmed_and_replace_the_old_one() {
    let dir = Dir::new();
    let path = dir.0.join("nested").join("pmd-write-token");
    store_token(&path, "  first-token \n").unwrap();
    assert_eq!(load_token(&path).unwrap(), "first-token");
    store_token(&path, "second-token").unwrap();
    assert_eq!(load_token(&path).unwrap(), "second-token");
    // No temporary file is left beside it.
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["pmd-write-token"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    #[cfg(windows)]
    assert_eq!(
        dashboard_core::write::token_file_sddl(&path).unwrap(),
        dashboard_core::write::OWNER_ONLY_SDDL
    );
}

#[test]
fn a_token_that_is_not_one_line_of_printable_text_is_not_stored() {
    let dir = Dir::new();
    let path = dir.0.join("new-write-token");
    for bad in ["", "   ", "two\nlines", "has space", "tab\there", &"x".repeat(8193)] {
        assert!(
            matches!(store_token(&path, bad), Err(WriteError::Storage(_))),
            "{bad:?}"
        );
    }
    assert!(!path.exists());
}

#[test]
fn config_reads_an_optional_write_token_path_beside_the_config() {
    let dir = std::path::Path::new("/config");
    let off = Config::parse("", &dir.join("config.toml")).unwrap();
    assert_eq!(off.pmd_write_token_path, None);
    assert!(!off.to_toml().contains("pmd_write_token_path"));
    assert_eq!(PmdWriter::from_config(&off, dir).write_token_path, None);
    let on = Config::parse("pmd_write_token_path = 'pmd-write-token'", &dir.join("config.toml")).unwrap();
    let writer = PmdWriter::from_config(&on, dir);
    assert_eq!(writer.write_token_path, Some(dir.join("pmd-write-token")));
    assert_eq!(writer.read_token_path, dir.join("pmd-token"));
}

#[test]
fn unbuilt_entries_are_disabled_with_why_and_built_ones_open_read_only_with_why() {
    let whoami = |scopes: &[&str]| {
        Access::Known(dashboard_core::write::Whoami {
            name: "project-dashboard@home".into(),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        })
    };
    // A built dialog always opens: it reads with the read token.
    for access in [Access::Off, whoami(&["read"]), Access::Failed(WriteError::MissingToken)] {
        assert_eq!(entry_state(Feature::WorkItems, true, &access), Ok(()), "{access:?}");
    }
    assert_eq!(
        entry_state(Feature::Projects, false, &whoami(&["read", "write:projects"])),
        Err("Not built yet (#34)".into())
    );
    assert_eq!(
        entry_state(Feature::Fleet, false, &Access::Off),
        Err("Not built yet (#36) · writes are off: set pmd_write_token_path".into())
    );
    // Whether it can write, or why it is read-only.
    assert_eq!(
        write_state(Feature::WorkItems, &Access::Off),
        Err("Writes are off: set pmd_write_token_path".into())
    );
    assert_eq!(
        write_state(Feature::WorkItems, &whoami(&["read", "write:work"])),
        Ok(())
    );
    assert_eq!(
        write_state(Feature::WorkItems, &whoami(&["read"])),
        Err("The token lacks write:work".into())
    );
    assert_eq!(write_state(Feature::Fleet, &whoami(&["admin"])), Ok(()));
    assert_eq!(
        write_state(Feature::Fleet, &Access::Failed(WriteError::MissingToken)),
        Err("No write token yet: add one in Write access…".into())
    );
    assert_eq!(
        write_state(Feature::WorkItems, &Access::Checking),
        Err("Checking write access…".into())
    );
}

#[test]
fn reads_need_only_the_read_token_and_encode_their_query() {
    let dir = Dir::new();
    let stub = Stub::new(|request| {
        assert_eq!(request.method, "GET");
        assert_eq!(request.token(), READ);
        Reply::json(200, json!({"items": [], "nextCursor": ""}))
    });
    let off = PmdWriter {
        write_token_path: None,
        ..dir.writer(&stub)
    };
    let query = [
        ("project".to_owned(), "gh-x,chief".to_owned()),
        ("cursor".to_owned(), "a b&c=d".to_owned()),
    ];
    off.list("v1/work", &query).unwrap();
    off.read("v1/work/01JBX").unwrap();
    let paths: Vec<String> = stub.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        ["/v1/work?project=gh-x%2Cchief&cursor=a+b%26c%3Dd", "/v1/work/01JBX"]
    );
    // No read token: nothing is sent.
    std::fs::remove_file(dir.0.join("pmd-token")).unwrap();
    assert_eq!(off.list("v1/work", &[]), Err(WriteError::MissingReadToken));
    assert_eq!(stub.requests().len(), 2);
}

fn row(version: u64, title: &str, priority: &str) -> Row {
    Row {
        version,
        values: BTreeMap::from([
            ("title".to_owned(), title.to_owned()),
            ("priority".to_owned(), priority.to_owned()),
        ]),
    }
}

#[test]
fn a_form_tracks_unsaved_edits_and_ignores_a_second_save_in_flight() {
    let mut form = Form::edit(row(4, "Old", "2"), &["title", "priority"]);
    assert!(!form.is_dirty());
    form.set("title", "New");
    assert!(form.is_dirty());
    form.set("title", "Old");
    assert!(!form.is_dirty());
    form.set("title", "New");
    assert_eq!(form.begin_save(), Some(Precondition::IfMatch(4)));
    assert!(form.is_saving());
    assert_eq!(form.begin_save(), None, "a double click sends nothing");
    form.set("title", "Changed while saving");
    assert_eq!(form.value("title"), "New");
    form.saved(row(5, "New", "2"));
    assert!(!form.is_dirty() && !form.is_saving());
    assert_eq!(form.version(), Some(5));
}

#[test]
fn a_create_form_uses_if_none_match_and_is_clean_until_edited() {
    let defaults = BTreeMap::from([("priority".to_owned(), "3".to_owned())]);
    let mut form = Form::create(&["title", "priority"], &defaults);
    assert!(!form.is_dirty());
    assert_eq!(form.value("priority"), "3");
    form.set("title", "New row");
    assert!(form.is_dirty());
    assert_eq!(form.begin_save(), Some(Precondition::IfNoneMatch));
}

#[test]
fn errors_go_next_to_the_named_field_or_on_the_dialog() {
    let mut form = Form::edit(row(4, "Old", "2"), &["title", "priority"]);
    form.set("priority", "9");
    form.begin_save();
    let next = form.failed(&WriteError::BadRequest {
        code: "bad_request".into(),
        message: "priority must be 0-5".into(),
    });
    assert_eq!(next, Next::Show);
    assert_eq!(form.field_error("priority"), Some("priority must be 0-5"));
    assert_eq!(form.message(), None);
    assert!(!form.is_saving());
    assert_eq!(form.begin_save(), None, "a known field problem blocks the save");
    form.set("priority", "3");
    assert_eq!(form.field_error("priority"), None);
    assert!(form.begin_save().is_some());
    form.failed(&WriteError::Network);
    assert_eq!(form.message(), Some("pmd is unavailable."));
    assert_eq!(form.value("priority"), "3", "the user's values stay");
}

#[test]
fn changed_elsewhere_keeps_edits_beside_the_new_values_then_reapplies_or_discards() {
    let mut form = Form::edit(row(4, "Old", "2"), &["title", "priority"]);
    form.set("title", "Mine");
    form.begin_save();
    assert_eq!(form.failed(&WriteError::StaleVersion), Next::Reread);
    assert!(form.is_saving(), "busy until the re-read lands");
    assert_eq!(form.begin_save(), None);
    form.conflict(row(7, "Theirs", "1"));
    assert!(!form.is_saving());
    assert!(form.message().unwrap().starts_with("Changed elsewhere"));
    let conflict = form.conflict_state().unwrap();
    assert_eq!(
        conflict.differences(),
        [("priority", "2", "1"), ("title", "Mine", "Theirs")]
    );
    assert_eq!(form.begin_save(), None, "the user chooses first");
    let mut discarded = form.clone();

    form.reapply();
    assert_eq!(form.value("title"), "Mine");
    assert_eq!(form.value("priority"), "2");
    assert!(form.is_dirty());
    assert_eq!(form.begin_save(), Some(Precondition::IfMatch(7)));

    discarded.discard();
    assert_eq!(discarded.value("title"), "Theirs");
    assert!(!discarded.is_dirty());
    assert_eq!(discarded.version(), Some(7));
    assert_eq!(discarded.message(), None);
}

#[test]
fn a_failed_reread_says_so_and_allows_another_try() {
    let mut form = Form::edit(row(4, "Old", "2"), &["title", "priority"]);
    form.set("title", "Mine");
    form.begin_save();
    form.failed(&WriteError::StaleVersion);
    form.reread_failed(&WriteError::Network);
    assert_eq!(
        form.message(),
        Some("Changed elsewhere, and reloading it failed: pmd is unavailable.")
    );
    assert_eq!(form.value("title"), "Mine");
    assert!(form.begin_save().is_some());
}

#[test]
fn rows_read_version_and_field_text_from_pmd_json() {
    let value = json!({"version": 3, "maxSessions": 2, "tailnetIp": null, "os": "macos", "capabilities": ["apple"]});
    let row = Row::from_json(&value, &["maxSessions", "tailnetIp", "os", "capabilities"]).unwrap();
    assert_eq!(row.version, 3);
    assert_eq!(row.values["maxSessions"], "2");
    assert_eq!(row.values["tailnetIp"], "");
    assert_eq!(row.values["os"], "macos");
    assert_eq!(row.values["capabilities"], "[\"apple\"]");
    assert_eq!(Row::from_json(&json!({"os": "x"}), &["os"]), Err(WriteError::Malformed));
}

#[test]
fn dots_inside_a_segment_are_a_name_not_a_traversal() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply::json(200, json!({"id": "mini/codex/model..preview", "version": 2})));
    let request = WriteRequest {
        path: "v1/agents/mini/codex/model..preview".into(),
        ..patch(1)
    };
    dir.writer(&stub).write(&request).unwrap();
    assert_eq!(stub.requests()[0].path, "/v1/agents/mini/codex/model..preview");
}

#[test]
fn a_form_checking_a_failed_save_stays_busy() {
    let mut form = Form::create(&["name"], &BTreeMap::new());
    form.set("name", "air");
    form.begin_save().unwrap();
    form.checking("Loading it…");
    assert!(form.is_saving());
    assert_eq!(form.message(), Some("Loading it…"));
    form.set("name", "typed meanwhile");
    assert_eq!(form.value("name"), "air", "no edits while checking");
    assert_eq!(form.begin_save(), None, "and no second save");
}

// Boundaries and small accessors that #101's mutation baseline showed no test pinned.

#[test]
fn a_body_of_exactly_64_kb_is_sent_and_one_byte_more_is_not() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply::json(200, json!({"version": 2})));
    // pmd's documented limit, written out rather than read from MAX_BODY, so a changed constant fails here.
    const PMD_LIMIT: usize = 64 * 1024;
    assert_eq!(MAX_BODY, PMD_LIMIT);
    let mut request = patch(1);
    // `{"statusReason":""}` is 19 bytes, so this body serializes to exactly 64 KB.
    request.body = json!({"statusReason": "x".repeat(PMD_LIMIT - 19)});
    assert_eq!(serde_json::to_vec(&request.body).unwrap().len(), PMD_LIMIT);
    assert_eq!(dir.writer(&stub).write(&request), Ok(json!({"version": 2})));
    assert_eq!(stub.requests()[0].body.len(), PMD_LIMIT);

    request.body = json!({"statusReason": "x".repeat(PMD_LIMIT - 18)});
    assert_eq!(dir.writer(&stub).write(&request), Err(WriteError::TooLarge));
    assert_eq!(stub.requests().len(), 1, "the oversized body is never sent");
}

#[test]
fn a_response_of_exactly_1_mb_is_read_and_one_byte_more_is_malformed() {
    const MAX_RESPONSE: usize = 1024 * 1024;
    // `{"version":1,"pad":""}` is 22 bytes.
    let exact = json!({"version": 1, "pad": "x".repeat(MAX_RESPONSE - 22)}).to_string();
    assert_eq!(exact.len(), MAX_RESPONSE);
    let dir = Dir::new();
    let body = exact.clone();
    let stub = Stub::new(move |_| Reply {
        body: body.clone(),
        ..Reply::json(200, Value::Null)
    });
    assert_eq!(dir.writer(&stub).write(&patch(1)).unwrap()["version"], 1);

    // One more byte, a space, so the first 1 MB alone would still be valid JSON: only the size check refuses it.
    let over = format!("{exact} ");
    let stub = Stub::new(move |_| Reply {
        body: over.clone(),
        ..Reply::json(200, Value::Null)
    });
    assert_eq!(dir.writer(&stub).write(&patch(1)), Err(WriteError::Malformed));
}

#[test]
fn a_token_of_exactly_8_kb_loads_whole_and_one_byte_more_is_refused() {
    let dir = Dir::new();
    let path = dir.0.join("long-token");
    let exact = "t".repeat(8192);
    std::fs::write(&path, &exact).unwrap();
    assert_eq!(load_token(&path), Ok(exact.clone()));
    std::fs::write(&path, format!("{exact}t")).unwrap();
    assert_eq!(load_token(&path), Err(WriteError::MissingToken));
}

#[test]
fn a_response_that_stalls_mid_body_is_a_timeout() {
    // Headers and part of the body arrive, then nothing: the read of the body times out, not the request.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut seen = Vec::new();
        let mut byte = [0];
        while !seen.ends_with(b"\r\n\r\n") && stream.read_exact(&mut byte).is_ok() {
            seen.push(byte[0]);
        }
        let _ =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n{\"ver");
        let _ = stream.flush();
        std::thread::sleep(Duration::from_millis(800));
    });
    let dir = Dir::new();
    let writer = PmdWriter {
        url,
        timeout: Duration::from_millis(300),
        ..dir.writer(&Stub::new(|_| unreachable!()))
    };
    assert_eq!(writer.write(&patch(1)), Err(WriteError::Timeout { create: None }));
    server.join().unwrap();
}

#[test]
fn each_feature_has_its_menu_label() {
    let labels: Vec<&str> = Feature::ALL.iter().map(|f| f.label()).collect();
    assert_eq!(labels, ["Projects", "Work items", "Fleet"]);
}

#[test]
fn only_errors_that_carry_pmds_message_return_it() {
    let text = || "pmd's message".to_owned();
    for error in [
        WriteError::BadRequest {
            code: "bad_request".into(),
            message: text(),
        },
        WriteError::InsufficientScope { message: text() },
        WriteError::Conflict {
            code: "not_tracked".into(),
            message: text(),
        },
        WriteError::Server {
            status: 503,
            code: "github_unavailable".into(),
            message: text(),
        },
    ] {
        assert_eq!(error.message(), Some("pmd's message"), "{error:?}");
    }
    for error in [
        WriteError::WritesOff,
        WriteError::Network,
        WriteError::StaleVersion,
        WriteError::TooLarge,
    ] {
        assert_eq!(error.message(), None, "{error:?}");
    }
}

#[test]
fn a_listing_without_a_query_is_a_plain_read_and_with_one_needs_its_own_backend() {
    struct ReadsOnly;
    impl WriteBackend for ReadsOnly {
        fn token_path(&self) -> Option<std::path::PathBuf> {
            None
        }
        fn whoami(&self) -> Result<dashboard_core::write::Whoami, WriteError> {
            Err(WriteError::WritesOff)
        }
        fn store_token(&self, _: &str) -> Result<(), WriteError> {
            Err(WriteError::WritesOff)
        }
        fn read(&self, path: &str) -> Result<Value, WriteError> {
            Ok(json!({"read": path}))
        }
        fn write(&self, _: &WriteRequest) -> Result<Value, WriteError> {
            Err(WriteError::WritesOff)
        }
    }
    assert_eq!(ReadsOnly.list("v1/work", &[]), Ok(json!({"read": "v1/work"})));
    let query = [("status".to_owned(), "open".to_owned())];
    assert_eq!(ReadsOnly.list("v1/work", &query), Err(WriteError::Malformed));
}

#[test]
fn the_off_backend_has_no_token_and_refuses_everything() {
    use dashboard_core::write::Off;
    assert_eq!(Off.token_path(), None);
    assert_eq!(Off.whoami().unwrap_err(), WriteError::WritesOff);
    assert_eq!(Off.store_token(WRITE), Err(WriteError::WritesOff));
    assert_eq!(Off.read("v1/work/01JBX"), Err(WriteError::WritesOff));
    assert_eq!(Off.list("v1/work", &[]), Err(WriteError::WritesOff));
    assert_eq!(Off.write(&patch(1)), Err(WriteError::WritesOff));
}

#[test]
fn the_live_backend_reports_its_configured_token_path() {
    let dir = Dir::new();
    let writer = dir.writer(&Stub::new(|_| unreachable!()));
    assert_eq!(writer.token_path(), Some(dir.0.join("pmd-write-token")));
    let off = PmdWriter {
        write_token_path: None,
        ..writer
    };
    assert_eq!(off.token_path(), None);
}

#[test]
fn a_form_lists_its_fields_in_the_order_given() {
    let form = Form::edit(row(1, "T", "2"), &["priority", "title"]);
    assert_eq!(form.fields().collect::<Vec<_>>(), ["priority", "title"]);
    let create = Form::create(&["title"], &BTreeMap::new());
    assert_eq!(create.fields().collect::<Vec<_>>(), ["title"]);
}

#[test]
fn a_form_ignores_text_for_a_field_it_does_not_have_and_while_a_conflict_waits() {
    let mut form = Form::edit(row(4, "Old", "2"), &["title", "priority"]);
    form.set("notes", "not a field of this form");
    assert_eq!(form.values().len(), 2);
    assert!(!form.values().contains_key("notes"));
    assert!(!form.is_dirty());

    form.begin_save();
    form.failed(&WriteError::StaleVersion);
    form.conflict(row(7, "Theirs", "1"));
    form.set("title", "Typed during the conflict");
    assert_ne!(
        form.value("title"),
        "Typed during the conflict",
        "the user chooses first"
    );
}

#[test]
fn a_rejected_field_blocks_saving_until_its_error_is_cleared() {
    let mut form = Form::edit(row(4, "Old", "2"), &["title", "priority"]);
    form.set("priority", "high");
    form.reject("priority", "a number from 0 to 4");
    assert_eq!(form.field_error("priority"), Some("a number from 0 to 4"));
    assert_eq!(form.begin_save(), None, "a known problem sends nothing");
    form.clear_error("priority");
    assert_eq!(form.field_error("priority"), None);
    assert_eq!(form.begin_save(), Some(Precondition::IfMatch(4)));
}
