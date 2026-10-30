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

#[test]
fn relative_ages_and_section_cache_freshness_do_not_glow_project_rows() {
    let baseline = fixtures::document(fixtures::ATTENTION);
    assert!(!baseline.issues.is_empty());
    assert!(!baseline.pull_requests.open.is_empty());
    assert!(!baseline.pull_requests.merged.is_empty());
    assert!(!baseline.workflow_runs.is_empty());
    assert!(!baseline.sections.is_empty());
    let edits: Vec<fn(&mut StatusDocument)> = vec![
        |d| d.issues.iter_mut().for_each(|i| i.updated.push_str(" later")),
        |d| {
            d.pull_requests
                .open
                .iter_mut()
                .for_each(|p| p.updated.push_str(" later"))
        },
        |d| {
            d.pull_requests
                .merged
                .iter_mut()
                .for_each(|p| p.updated.push_str(" later"))
        },
        |d| d.workflow_runs.iter_mut().for_each(|r| r.age.push_str(" later")),
        |d| d.workflow_runs.iter_mut().for_each(|r| r.elapsed.push_str(" later")),
        |d| {
            d.sections
                .iter_mut()
                .for_each(|s| s.fetched_at = Some("2026-10-30T01:00:00Z".into()))
        },
        |d| d.sections.iter_mut().for_each(|s| s.cache_age_seconds += 300.),
        |d| d.sections.iter_mut().for_each(|s| s.cached = !s.cached),
    ];
    for (index, edit) in edits.iter().enumerate() {
        let mut next = baseline.clone();
        edit(&mut next);
        assert!(
            !changed(Some(&baseline), &next),
            "clock-only field {index} must not glow"
        );
        assert!(
            !changed(Some(&next), &baseline),
            "cache reset/refresh {index} must not glow"
        );
    }
    let mut next = baseline.clone();
    for edit in edits {
        edit(&mut next);
    }
    assert!(!changed(Some(&baseline), &next), "snapshot turnover must not glow");
    next.pull_requests.open[0].checks.push_str(" changed");
    assert!(
        changed(Some(&baseline), &next),
        "checks still glow alongside clock-only changes"
    );
}

#[test]
fn absolute_activity_times_and_section_verdicts_still_trigger_glows() {
    let baseline = fixtures::document(fixtures::ATTENTION);
    let edits: Vec<fn(&mut StatusDocument)> = vec![
        |d| d.issues[0].updated_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.pull_requests.open[0].updated_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.pull_requests.merged[0].updated_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.pull_requests.merged[0].merged_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.workflow_runs[0].created_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.workflow_runs[0].started_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.workflow_runs[0].updated_at = Some("2026-10-30T01:00:00Z".into()),
        |d| d.workflow_runs[0].status.push_str(" changed"),
        |d| d.sections[0].status.push_str(" changed"),
        |d| d.sections[0].reason.push_str(" changed"),
    ];
    for (index, edit) in edits.iter().enumerate() {
        let mut next = baseline.clone();
        edit(&mut next);
        assert!(
            changed(Some(&baseline), &next),
            "activity/verdict field {index} must glow"
        );
    }
}
