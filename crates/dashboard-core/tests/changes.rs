use dashboard_core::{changes::changed, fixtures, status::StatusDocument};

#[test]
fn first_load_and_unchanged_rows_do_not_glow() {
    let doc = fixtures::document(fixtures::ATTENTION);
    assert!(!changed(None, &doc));
    assert!(!changed(Some(&doc), &doc));
    let mut next = doc.clone();
    next.generated_at = "later poll".into();
    assert!(!changed(Some(&doc), &next));
}

#[test]
fn row_fields_trigger_independently_of_polling_timestamps() {
    let mut doc = fixtures::document(fixtures::ATTENTION);
    // Build a hosted baseline without depending on a live service.
    doc.hosted = Some(
        serde_json::from_value(serde_json::json!({
            "serviceUrl":"http://fixture", "generatedAt":"before", "lastError":null,
            "board":null,"work":[],"workError":null
        }))
        .unwrap(),
    );
    let mut next = doc.clone();
    next.hosted.as_mut().unwrap().checked_at = Some("later".into());
    next.hosted.as_mut().unwrap().generated_at = "later".into();
    assert!(!changed(Some(&doc), &next));
    let edits: Vec<fn(&mut StatusDocument)> = vec![
        |d| d.hosted.as_mut().unwrap().open_counts.open_prs = Some(100),
        |d| d.hosted.as_mut().unwrap().open_counts.open_issues = Some(100),
        |d| d.hosted.as_mut().unwrap().merge_counts.merged_24h = Some(3),
        |d| d.hosted.as_mut().unwrap().merge_counts.merged_7d = Some(9),
        |d| d.pull_requests.open[0].checks.push_str(" changed"),
        |d| d.pull_requests.open[0].title = "Updated title".into(),
        |d| d.verdict.get_or_insert_with(Default::default).reason = "changed".into(),
        |d| d.hosted.as_mut().unwrap().work_error = Some("unavailable".into()),
    ];
    for edit in edits {
        let mut next = doc.clone();
        edit(&mut next);
        assert!(changed(Some(&doc), &next));
    }
}

#[test]
fn work_status_owner_and_session_changes_trigger_the_row() {
    let doc: StatusDocument = serde_json::from_value(serde_json::json!({
        "hosted": {"serviceUrl":"http://fixture", "generatedAt":"before", "lastError":null,
        "board":null,"workError":null,"work":[{
            "id":"w1","project":"fixture","kind":"issue","number":1,"title":"Ship","url":"https://example.test/1",
            "status":"in_progress", "assignment":{"id":"a1","machine":"mini","threadId":"t1","model":"fixture","state":"working","stale":false,"ownership":"owned"}
        }]}
    })).unwrap();
    for field in ["status", "machine", "threadId", "state", "ownership"] {
        let mut next = serde_json::to_value(&doc).unwrap();
        if field == "status" {
            next["hosted"]["work"][0][field] = "blocked".into();
        } else {
            next["hosted"]["work"][0]["assignment"][field] = "changed".into();
        }
        assert!(changed(Some(&doc), &serde_json::from_value(next).unwrap()), "{field}");
    }
}

#[test]
fn backlog_only_changes_do_not_glow_project_rows() {
    use dashboard_core::backlog::{Assignment, Context, Decision, Plan, Ref};
    use dashboard_core::source::TransportError;

    let baseline: StatusDocument = serde_json::from_value(serde_json::json!({
        "hosted": {"serviceUrl":"http://fixture", "generatedAt":"before", "lastError":null,
        "board":null,"work":[],"workError":null}
    }))
    .unwrap();
    let decision = Decision {
        work_item: Ref {
            id: "another-project:issue:1".into(),
        },
        reason: "Dispatched elsewhere".into(),
    };
    let contexts = [
        Context {
            assignments: vec![Assignment {
                thread_id: "another-thread".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        Context {
            plan: Plan {
                recommendations: vec![decision.clone()],
                ..Default::default()
            },
            ..Default::default()
        },
        Context {
            plan: Plan {
                skipped: vec![decision],
                ..Default::default()
            },
            ..Default::default()
        },
        Context {
            plan: Plan {
                blocked: Some("Global dispatch paused".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        Context {
            plan: Plan {
                items_truncated: true,
                skipped_truncated: true,
                ..Default::default()
            },
            ..Default::default()
        },
        Context {
            error: Some("Backlog enrichment unavailable".into()),
            transport_error: Some(TransportError::Connect),
            ..Default::default()
        },
    ];
    for context in contexts {
        let mut next = baseline.clone();
        next.hosted.as_mut().unwrap().backlog = context;
        assert!(!changed(Some(&baseline), &next), "Backlog-only update must not glow");
        assert!(!changed(Some(&next), &baseline), "Backlog recovery must not glow");
        next.hosted.as_mut().unwrap().merge_counts.merged_24h = Some(1);
        assert!(
            changed(Some(&baseline), &next),
            "Project data must still glow alongside Backlog changes"
        );
    }
}
