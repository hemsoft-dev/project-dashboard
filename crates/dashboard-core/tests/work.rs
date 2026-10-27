//! The Work items dialog's data (#35): parsing pmd's items and pages, the list's query, the changed-field `PATCH` and
//! its checks, and the write against a loopback stub for every answer pmd gives.
use dashboard_core::edit::{Form, Next};
use dashboard_core::work::{self, FIELDS, Kind, WorkFilter, WorkItem, WorkPage};
use dashboard_core::write::{Method, Precondition, WriteBackend, WriteError, WriteRequest};
use serde_json::{Value, json};

#[path = "support/stub.rs"]
mod stub;
use stub::{Dir, Reply, Stub, WRITE};

fn item(overrides: Value) -> Value {
    let mut item = json!({
        "id": "01JBXWORK", "project": "gh-x", "kind": "issue", "number": 233, "title": "Show the tray icon",
        "url": "https://github.com/hemsoft-dev/gh-x/issues/233", "ghState": "open", "labels": [], "author": "franz",
        "status": "ready", "statusSource": "auto", "statusReason": null, "priority": 2, "priorityOverride": null,
        "rank": 0, "version": 4, "links": [], "assignment": null, "ghUpdatedAt": "2026-10-27T05:00:00Z"
    });
    for (key, value) in overrides.as_object().unwrap() {
        item[key] = value.clone();
    }
    item
}

fn form(overrides: Value) -> Form {
    let item = WorkItem::from_json(&item(overrides)).unwrap();
    Form::edit(item.row(), &FIELDS)
}

#[test]
fn an_item_reads_its_status_source_override_and_rank_into_the_editor() {
    let auto = WorkItem::from_json(&item(json!({}))).unwrap();
    assert!(!auto.is_manual() && !auto.is_closed());
    assert_eq!(auto.path(), "v1/work/01JBXWORK");
    let row = auto.row();
    assert_eq!(row.version, 4);
    assert_eq!(row.values["status"], "auto", "an automatic status edits as auto");
    assert_eq!(row.values["priority"], "", "no override inherits");
    assert_eq!(row.values["rank"], "0");
    let manual = WorkItem::from_json(&item(json!({
        "status": "blocked", "statusSource": "manual", "statusReason": "needs Apple review",
        "priority": 0, "priorityOverride": 0, "rank": -5
    })))
    .unwrap();
    let row = manual.row();
    assert_eq!(row.values["status"], "blocked");
    assert_eq!(row.values["statusReason"], "needs Apple review");
    assert_eq!(row.values["priority"], "0");
    assert_eq!(row.values["rank"], "-5");
    let closed = WorkItem::from_json(&item(json!({"status": "done", "ghState": "closed"}))).unwrap();
    assert!(closed.is_closed());
    assert_eq!(WorkItem::from_json(&json!({"id": 1})), Err(WriteError::Malformed));
}

#[test]
fn a_page_reads_its_items_and_an_empty_cursor_means_the_last_page() {
    let page = WorkPage::from_json(&json!({"items": [item(json!({}))], "nextCursor": "abc"})).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_cursor.as_deref(), Some("abc"));
    let last = WorkPage::from_json(&json!({"items": [], "nextCursor": ""})).unwrap();
    assert_eq!(last.next_cursor, None);
    assert_eq!(
        WorkPage::from_json(&json!({"nextCursor": ""})),
        Err(WriteError::Malformed)
    );
}

#[test]
fn filters_become_pmds_query_with_paging() {
    assert_eq!(
        WorkFilter::default().query(None),
        [("limit".to_owned(), "50".to_owned())]
    );
    let filter = WorkFilter {
        project: " gh-x, chief  ".into(),
        status: Some("blocked"),
        kind: Some(Kind::Pr),
        max_priority: Some(1),
    };
    let expected = [
        ("limit", "50"),
        ("project", "gh-x,chief"),
        ("status", "blocked"),
        ("kind", "pr"),
        ("maxPriority", "1"),
        ("cursor", "next"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()));
    assert_eq!(filter.query(Some("next")), expected);
}

#[test]
fn an_unchanged_editor_sends_nothing() {
    assert_eq!(work::patch(&form(json!({}))), Ok(None));
}

#[test]
fn a_new_manual_status_carries_its_reason_and_auto_never_does() {
    let mut edit = form(json!({}));
    edit.set("status", "blocked");
    edit.set("statusReason", "  needs Apple review ");
    assert_eq!(
        work::patch(&edit),
        Ok(Some(json!({"status": "blocked", "statusReason": "needs Apple review"})))
    );
    edit.set("statusReason", "");
    assert_eq!(work::patch(&edit), Ok(Some(json!({"status": "blocked"}))));
    let mut back = form(json!({"status": "blocked", "statusSource": "manual", "statusReason": "waiting"}));
    back.set("status", "auto");
    assert_eq!(work::patch(&back), Ok(Some(json!({"status": "auto"}))));
}

#[test]
fn a_reason_alone_is_sent_alone_and_emptying_it_sends_null() {
    let manual = json!({"status": "blocked", "statusSource": "manual", "statusReason": "waiting"});
    let mut edit = form(manual.clone());
    edit.set("statusReason", "waiting on review");
    assert_eq!(
        work::patch(&edit),
        Ok(Some(json!({"statusReason": "waiting on review"})))
    );
    edit.set("statusReason", "   ");
    assert_eq!(work::patch(&edit), Ok(Some(json!({"statusReason": null}))));
}

#[test]
fn priority_and_rank_changes_are_sent_and_reorder_the_list() {
    let mut edit = form(json!({}));
    edit.set("priority", "0");
    edit.set("rank", "-12");
    let body = work::patch(&edit).unwrap().unwrap();
    assert_eq!(body, json!({"priority": 0, "rank": -12}));
    assert!(work::reorders(&body));
    let mut inherit = form(json!({"priority": 0, "priorityOverride": 0}));
    inherit.set("priority", "");
    assert_eq!(work::patch(&inherit), Ok(Some(json!({"priority": null}))));
    // The same number written another way is no change.
    let mut same = form(json!({"rank": 7}));
    same.set("rank", "007");
    assert_eq!(work::patch(&same), Ok(None));
    assert!(!work::reorders(&json!({"status": "ready"})));
}

#[test]
fn the_same_checks_as_pmd_stop_a_save_on_the_field() {
    let mut edit = form(json!({}));
    edit.set("status", "blocked");
    edit.set("statusReason", "x".repeat(501));
    edit.set("rank", "1000000001");
    let problems = work::patch(&edit).unwrap_err();
    let fields: Vec<&str> = problems.iter().map(|(field, _)| *field).collect();
    assert_eq!(fields, ["statusReason", "rank"]);
    edit.set("statusReason", "é".repeat(500));
    edit.set("rank", "twelve");
    assert_eq!(work::patch(&edit).unwrap_err()[0].0, "rank");
    edit.set("rank", "-1000000000");
    assert!(
        work::patch(&edit).is_ok(),
        "500 characters and the rank limit are allowed"
    );
    edit.set("priority", "6");
    assert_eq!(work::patch(&edit).unwrap_err()[0].0, "priority");
}

#[test]
fn the_editor_says_before_save_when_a_reason_will_be_cleared() {
    let manual = json!({"status": "blocked", "statusSource": "manual", "statusReason": "waiting"});
    let mut edit = form(manual.clone());
    assert_eq!(work::reason_note(&edit), None);
    edit.set("status", "auto");
    assert_eq!(
        work::reason_note(&edit).as_deref(),
        Some("Back to automatic clears the current reason.")
    );
    let mut other = form(manual.clone());
    other.set("status", "ready");
    assert_eq!(
        work::reason_note(&other),
        None,
        "the reason goes along with the new status"
    );
    other.set("statusReason", "");
    assert_eq!(
        work::reason_note(&other).as_deref(),
        Some("A new status without a reason clears the current reason.")
    );
    let mut emptied = form(manual);
    emptied.set("statusReason", "");
    assert_eq!(
        work::reason_note(&emptied).as_deref(),
        Some("Saving clears the reason.")
    );
    assert_eq!(work::reason_note(&form(json!({}))), None, "no reason, nothing to clear");
}

fn patch_of(edit: &Form, body: Value) -> WriteRequest {
    WriteRequest {
        method: Method::Patch,
        path: work::item_path("01JBXWORK"),
        precondition: Precondition::IfMatch(edit.version().unwrap()),
        body,
    }
}

#[test]
fn a_save_patches_only_the_changed_fields_with_if_match_and_reads_back_the_item() {
    let dir = Dir::new();
    let stub = Stub::new(|_| {
        Reply::json(
            200,
            item(json!({"status": "blocked", "statusSource": "manual", "statusReason": "review", "version": 5})),
        )
    });
    let mut edit = form(json!({}));
    edit.set("status", "blocked");
    edit.set("statusReason", "review");
    let body = work::patch(&edit).unwrap().unwrap();
    let precondition = edit.begin_save().unwrap();
    assert_eq!(precondition, Precondition::IfMatch(4));
    let answer = dir.writer(&stub).write(&patch_of(&edit, body)).unwrap();
    let saved = WorkItem::from_json(&answer).unwrap();
    edit.saved(saved.row());
    assert!(!edit.is_dirty());
    assert_eq!(edit.version(), Some(5));
    let [request] = stub.requests().try_into().unwrap();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("PATCH", "/v1/work/01JBXWORK")
    );
    assert_eq!(request.header("if-match"), Some("4"));
    assert_eq!(request.token(), WRITE);
    let sent: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(sent, json!({"status": "blocked", "statusReason": "review"}));
}

/// What the editor shows after one answer.
type Check = Box<dyn Fn(&Form, Next)>;

#[test]
fn every_pmd_answer_to_a_work_patch_reaches_the_editor() {
    let cases: Vec<(Reply, Check)> = vec![
        (
            Reply::error(
                400,
                "bad_request",
                "statusReason must be valid text of at most 500 characters",
            ),
            Box::new(|form, next| {
                assert_eq!(next, Next::Show);
                assert!(form.field_error("statusReason").unwrap().contains("at most 500"));
            }),
        ),
        (
            Reply::error(400, "bad_request", "rank must be between -1000000000 and 1000000000"),
            Box::new(|form, _| assert!(form.field_error("rank").is_some())),
        ),
        (
            Reply::error(400, "bad_request", "an automatic status has no statusReason"),
            Box::new(|form, _| assert!(form.message().unwrap().contains("automatic status"))),
        ),
        (
            Reply::error(404, "not_found", "no work item"),
            Box::new(|form, _| assert!(form.message().unwrap().starts_with("Not found"))),
        ),
        (
            Reply::error(409, "stale_version", "If-Match is 4 but the item is at 6"),
            Box::new(|form, next| {
                assert_eq!(next, Next::Reread);
                assert!(form.is_saving(), "busy until the re-read lands");
            }),
        ),
        (
            Reply::error(409, "item_closed", "the item is closed; its status is done"),
            Box::new(|form, next| {
                assert_eq!(next, Next::Show);
                assert!(form.message().unwrap().contains("closed"));
            }),
        ),
        (
            Reply::error(413, "too_large", "the body is over 65536 bytes"),
            Box::new(|form, _| assert!(form.message().unwrap().starts_with("Too large"))),
        ),
        (
            Reply::error(428, "precondition_required", "send If-Match"),
            Box::new(|form, _| assert!(form.message().unwrap().contains("428"))),
        ),
    ];
    for (reply, check) in cases {
        let dir = Dir::new();
        let reply = std::sync::Mutex::new(Some(reply));
        let stub = Stub::new(move |_| reply.lock().unwrap().take().unwrap());
        let mut edit = form(json!({}));
        edit.set("status", "blocked");
        let body = work::patch(&edit).unwrap().unwrap();
        edit.begin_save().unwrap();
        let error = dir.writer(&stub).write(&patch_of(&edit, body)).unwrap_err();
        let next = edit.failed(&error);
        check(&edit, next);
        assert_eq!(edit.value("status"), "blocked", "the user's values stay");
        assert_eq!(stub.requests().len(), 1, "never retried");
    }
}
