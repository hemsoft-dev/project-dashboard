//! The Fleet dialog's data (#36): pmd's rows, the client-side checks, the full-row `PUT`, and every answer pmd gives,
//! against a loopback stub.
use std::collections::BTreeMap;

use dashboard_core::edit::{Form, Next};
use dashboard_core::fleet::{self, FleetRow, Kind, Known};
use dashboard_core::write::{Method, Precondition, WriteBackend, WriteError, WriteRequest};
use serde_json::{Value, json};

#[path = "support/stub.rs"]
mod stub;
use stub::{Dir, Reply, Stub, WRITE};

fn machine() -> Value {
    json!({"name": "mini", "os": "linux", "tailnetIp": "100.97.164.73", "maxSessions": 3,
           "capabilities": ["browser", "linux"], "source": "config", "version": 2,
           "createdAt": "2026-10-25T00:00:00Z", "updatedAt": "2026-10-25T00:00:00Z"})
}

fn agent() -> Value {
    json!({"id": "mini/codex/gpt-6.1-sol", "machine": "mini", "provider": "codex", "model": "gpt-6.1-sol",
           "subscription": "chatgpt-pro", "roles": ["implement", "review"], "enabled": true, "source": "api",
           "version": 4})
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

fn known() -> Known {
    Known {
        machines: vec!["mini".into(), "air".into()],
        subscriptions: vec!["chatgpt-pro".into()],
    }
}

#[test]
fn rows_read_their_key_source_version_and_fields_as_text() {
    let mini = FleetRow::from_json(Kind::Machine, &machine()).unwrap();
    assert_eq!(
        (mini.key.as_str(), mini.source.as_str(), mini.version),
        ("mini", "config", 2)
    );
    assert!(mini.from_config());
    assert_eq!(mini.value("capabilities"), "browser, linux");
    assert_eq!(mini.value("maxSessions"), "3");
    assert_eq!(mini.path(), "v1/machines/mini");
    let mut no_ip = machine();
    no_ip["tailnetIp"] = Value::Null;
    assert_eq!(
        FleetRow::from_json(Kind::Machine, &no_ip).unwrap().value("tailnetIp"),
        ""
    );
    let sol = FleetRow::from_json(Kind::Agent, &agent()).unwrap();
    assert_eq!(sol.value("alias"), "gpt-6.1-sol");
    assert_eq!(sol.value("enabled"), "true");
    assert_eq!(sol.path(), "v1/agents/mini/codex/gpt-6.1-sol");
    let list = FleetRow::list_from_json(Kind::Agent, &json!({"agents": [agent()]})).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        FleetRow::list_from_json(Kind::Machine, &json!({"agents": []})),
        Err(WriteError::Malformed)
    );
    assert_eq!(
        FleetRow::from_json(Kind::Machine, &json!({"name": "x"})),
        Err(WriteError::Malformed)
    );
}

#[test]
fn a_machine_body_carries_every_field_in_pmds_form() {
    let row = values(&[
        ("name", "air"),
        ("os", "macos"),
        ("tailnetIp", " 100.101.1.2 "),
        ("maxSessions", "2"),
        ("capabilities", "browser, apple apple"),
    ]);
    let (key, body) = fleet::body(Kind::Machine, &row, &known()).unwrap();
    assert_eq!(key, "air");
    assert_eq!(
        body,
        json!({"os": "macos", "tailnetIp": "100.101.1.2", "maxSessions": 2, "capabilities": ["apple", "browser"]})
    );
    let none = values(&[
        ("name", "air"),
        ("os", "linux"),
        ("tailnetIp", ""),
        ("maxSessions", "0"),
        ("capabilities", ""),
    ]);
    let (_, body) = fleet::body(Kind::Machine, &none, &known()).unwrap();
    assert_eq!(
        body,
        json!({"os": "linux", "tailnetIp": null, "maxSessions": 0, "capabilities": []})
    );
}

#[test]
fn machine_checks_match_pmds() {
    let bad = values(&[
        ("name", "Air"),
        ("os", "beos"),
        ("tailnetIp", "192.168.1.2"),
        ("maxSessions", "65"),
        ("capabilities", "Apple"),
    ]);
    let problems = fleet::body(Kind::Machine, &bad, &known()).unwrap_err();
    let fields: Vec<&str> = problems.iter().map(|(field, _)| *field).collect();
    assert_eq!(fields, ["capabilities", "name", "os", "tailnetIp", "maxSessions"]);
    for ip in ["100.64.0.1", "100.127.255.255", "fd7a:115c:a1e0::1"] {
        let row = values(&[
            ("name", "air"),
            ("os", "linux"),
            ("tailnetIp", ip),
            ("maxSessions", "64"),
        ]);
        assert!(fleet::body(Kind::Machine, &row, &known()).is_ok(), "{ip}");
    }
    let row = values(&[
        ("name", "air"),
        ("os", "linux"),
        ("tailnetIp", "100.128.0.1"),
        ("maxSessions", "-1"),
    ]);
    assert_eq!(fleet::body(Kind::Machine, &row, &known()).unwrap_err().len(), 2);
}

#[test]
fn a_subscription_body_leaves_its_usage_source_to_pmd() {
    let row = values(&[
        ("id", "claude-max"),
        ("provider", "anthropic"),
        ("plan", " Max 20x "),
        ("metered", "false"),
    ]);
    let (key, body) = fleet::body(Kind::Subscription, &row, &known()).unwrap();
    assert_eq!(key, "claude-max");
    assert_eq!(
        body,
        json!({"provider": "anthropic", "plan": "Max 20x", "metered": false})
    );
    let bad = values(&[
        ("id", "x_y"),
        ("provider", "9ai"),
        ("plan", &"p".repeat(101)),
        ("metered", "true"),
    ]);
    let fields: Vec<&str> = fleet::body(Kind::Subscription, &bad, &known())
        .unwrap_err()
        .iter()
        .map(|(f, _)| *f)
        .collect();
    assert_eq!(fields, ["id", "provider", "plan"]);
}

#[test]
fn an_agent_body_is_keyed_by_machine_provider_and_alias_and_checks_both_references() {
    let row = values(&[
        ("machine", "air"),
        ("provider", "claude"),
        ("alias", "opus-5.5"),
        ("model", "claude-opus-5-5"),
        ("subscription", "chatgpt-pro"),
        ("roles", "review"),
        ("enabled", "true"),
    ]);
    let (key, body) = fleet::body(Kind::Agent, &row, &known()).unwrap();
    assert_eq!(key, "air/claude/opus-5.5");
    assert_eq!(
        body,
        json!({"model": "claude-opus-5-5", "subscription": "chatgpt-pro", "roles": ["review"], "enabled": true})
    );
    let mut unknown = row.clone();
    unknown.insert("machine".into(), "laptop".into());
    unknown.insert("subscription".into(), "gemini-ultra".into());
    let problems = fleet::body(Kind::Agent, &unknown, &known()).unwrap_err();
    assert_eq!(problems[0], ("machine", "No machine \"laptop\" in pmd.".to_owned()));
    assert_eq!(
        problems[1],
        ("subscription", "No subscription \"gemini-ultra\" in pmd.".to_owned())
    );
}

#[test]
fn suggested_words_join_the_list_once() {
    assert_eq!(fleet::words(" linux,browser  linux "), ["browser", "linux"]);
    assert_eq!(fleet::add_word("linux", "browser"), "linux, browser");
    assert_eq!(fleet::add_word("browser, linux", "linux"), "browser, linux");
    assert_eq!(fleet::add_word("", "review"), "review");
}

fn put(kind: Kind, key: &str, precondition: Precondition, body: Value) -> WriteRequest {
    WriteRequest {
        method: Method::Put,
        path: kind.path(key),
        precondition,
        body,
    }
}

#[test]
fn create_and_replace_send_the_full_row_with_their_precondition() {
    let dir = Dir::new();
    let stub = Stub::new(|request| {
        if request.method == "PUT" && request.header("if-none-match") == Some("*") {
            Reply::json(201, agent())
        } else {
            Reply::json(200, machine())
        }
    });
    let writer = dir.writer(&stub);
    let row = values(&[
        ("machine", "mini"),
        ("provider", "codex"),
        ("alias", "gpt-6.1-sol"),
        ("model", "gpt-6.1-sol"),
        ("subscription", "chatgpt-pro"),
        ("roles", "implement, review"),
        ("enabled", "true"),
    ]);
    let (key, body) = fleet::body(Kind::Agent, &row, &known()).unwrap();
    let created = writer
        .write(&put(Kind::Agent, &key, Precondition::IfNoneMatch, body))
        .unwrap();
    assert_eq!(FleetRow::from_json(Kind::Agent, &created).unwrap().version, 4);
    let mini = FleetRow::from_json(Kind::Machine, &machine()).unwrap();
    let mut edit = Form::edit(mini.row(), Kind::Machine.fields());
    edit.set("maxSessions", "2");
    let (key, body) = fleet::body(Kind::Machine, edit.values(), &known()).unwrap();
    let precondition = edit.begin_save().unwrap();
    writer.write(&put(Kind::Machine, &key, precondition, body)).unwrap();
    let requests = stub.requests();
    assert_eq!(requests[0].path, "/v1/agents/mini/codex/gpt-6.1-sol");
    assert_eq!(requests[0].header("if-none-match"), Some("*"));
    assert_eq!(requests[1].path, "/v1/machines/mini");
    assert_eq!(requests[1].header("if-match"), Some("2"));
    assert_eq!(requests[1].token(), WRITE);
    let sent: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(
        sent,
        json!({"os": "linux", "tailnetIp": "100.97.164.73", "maxSessions": 2, "capabilities": ["browser", "linux"]})
    );
}

#[test]
fn every_pmd_answer_to_a_fleet_put_lands_where_the_dialog_shows_it() {
    let cases: Vec<(Reply, Option<&str>, Next)> = vec![
        (
            Reply::error(
                400,
                "bad_request",
                "roles: \"X\" must be lower-case letters, digits and _",
            ),
            Some("roles"),
            Next::Show,
        ),
        (
            Reply::error(
                400,
                "unknown_machine",
                "agent \"laptop/codex/x\" is on machine \"laptop\", which pmd does not have",
            ),
            Some("machine"),
            Next::Show,
        ),
        (
            Reply::error(400, "unknown_subscription", "subscription \"x\" does not exist"),
            Some("subscription"),
            Next::Show,
        ),
        (
            Reply::error(409, "already_exists", "agent exists (version 3)"),
            Some("alias"),
            Next::Show,
        ),
        (
            Reply::error(409, "stale_version", "If-Match is 2 but it is 3"),
            None,
            Next::Reread,
        ),
        (Reply::error(404, "not_found", "no machine"), None, Next::Show),
        (Reply::error(413, "too_large", "over"), None, Next::Show),
        (
            Reply::error(428, "precondition_required", "send If-Match"),
            None,
            Next::Show,
        ),
    ];
    for (reply, field, expected) in cases {
        let dir = Dir::new();
        let reply = std::sync::Mutex::new(Some(reply));
        let stub = Stub::new(move |_| reply.lock().unwrap().take().unwrap());
        let mut edit = Form::create(Kind::Agent.fields(), &Kind::Agent.defaults());
        for (k, v) in [
            ("machine", "mini"),
            ("provider", "codex"),
            ("alias", "x"),
            ("model", "m"),
            ("subscription", "chatgpt-pro"),
        ] {
            edit.set(k, v);
        }
        let (key, body) = fleet::body(Kind::Agent, edit.values(), &known()).unwrap();
        let precondition = edit.begin_save().unwrap();
        let error = dir
            .writer(&stub)
            .write(&put(Kind::Agent, &key, precondition, body))
            .unwrap_err();
        let next = match fleet::error_field(Kind::Agent, &error) {
            Some(field) => {
                edit.failed_on(field, &error);
                assert_eq!(edit.message(), None, "on the field only, not twice");
                Next::Show
            }
            None => edit.failed(&error),
        };
        assert_eq!(next, expected, "{error:?}");
        match field {
            Some(field) => assert!(edit.field_error(field).is_some(), "{error:?} on {field}"),
            None => assert!(edit.message().is_some(), "{error:?}"),
        }
        assert_eq!(stub.requests().len(), 1, "never retried");
    }
}

#[test]
fn a_put_that_changes_nothing_returns_the_row_as_it_was() {
    let dir = Dir::new();
    let stub = Stub::new(|_| Reply::json(200, machine()));
    let mini = FleetRow::from_json(Kind::Machine, &machine()).unwrap();
    let (key, body) = fleet::body(Kind::Machine, &mini.values, &known()).unwrap();
    let answer = dir
        .writer(&stub)
        .write(&put(Kind::Machine, &key, Precondition::IfMatch(2), body))
        .unwrap();
    let same = FleetRow::from_json(Kind::Machine, &answer).unwrap();
    assert_eq!((same.version, same.source.as_str()), (2, "config"));
}
