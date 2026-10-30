use dashboard_core::{
    fixtures,
    pmd::Hosted,
    readiness::{DispatchReason, Freshness, Source, oldest_waiting, waiting_tooltip},
};
use serde_json::json;
fn hosted() -> Hosted {
    serde_json::from_value(json!({"serviceUrl":"fixture","generatedAt":"2026-10-29T12:00:00Z","lastError":null,"board":null,"work":[],"workError":null})).unwrap()
}
#[test]
fn dispatch_reasons_and_owner_hold_capacity_details_match_pmd() {
    for (reason, label) in [
        ("empty_backlog", "Empty backlog"),
        ("all_owned", "All owned"),
        ("held", "Held"),
        ("waiting_on_franz", "Waiting on Franz"),
        ("capacity_gated", "Capacity gated"),
        ("dispatchable", "Dispatchable"),
        ("future_reason", "—"),
    ] {
        let value: DispatchReason=serde_json::from_value(json!({"project":"example","reason":reason,"count":2,"owners":[{"owner":"codex","machine":"mini","state":"running","ownership":"held"}],"holdSources":["manual overlay"],"subscriptions":["codex-pro"],"stale":true})).unwrap();
        assert_eq!(value.label(), label);
        let text = value.tooltip();
        for detail in ["codex", "mini", "manual overlay", "codex-pro", "stale"] {
            assert!(text.contains(detail), "{detail}: {text}");
        }
    }
}
#[test]
fn oldest_waiting_uses_waiting_since_not_work_order_and_ignores_invalid_or_closed_items() {
    let mut h = hosted();
    let item = |id, status, since, state| {
        serde_json::from_value(json!({"id":id,"project":"example","kind":"issue","number":1,"title":"A held task","url":"https://example.test/1","status":status,"waitingSince":since,"waitingReason":"manual:signing","ghState":state})).unwrap()
    };
    h.work = vec![
        item("new", "waiting_on_franz", "2026-10-29T00:00:00Z", "open"),
        item("old", "blocked", "2026-10-23T00:00:00Z", "open"),
        item("closed", "blocked", "2026-10-01T00:00:00Z", "closed"),
        item("invalid", "blocked", "invalid", "open"),
        item("ready", "ready", "2026-10-01T00:00:00Z", "open"),
    ];
    let (old, since) = oldest_waiting(&h).unwrap();
    assert_eq!(old.id, "old");
    let text = waiting_tooltip(old, since);
    assert!(text.contains("manual:signing"));
    assert!(text.contains("Issue #1"));
    assert!(!text.contains("T00:00"));
    h.work.clear();
    assert!(oldest_waiting(&h).is_none());
}
#[test]
fn freshness_is_scoped_to_full_repository_and_tracks_state_and_collection_time() {
    let meta = json!({"lastSuccessAt":"2026-10-29T12:00:00Z","lastAttemptAt":"2026-10-29T12:01:00Z","state":"fresh","reason":"","staleAfterSeconds":60});
    let f:Freshness=serde_json::from_value(json!({"github":meta,"capacity":meta,"ghX":{"OTHER/name":meta,"HEMSOFT-DEV/example":meta},"t3":{"mini/env":meta}})).unwrap();
    let sources = f.for_repository("hemsoft-dev/example");
    assert!(sources.contains_key("gh-x/hemsoft-dev/example"));
    assert!(!sources.contains_key("gh-x/OTHER/name"));
    let now = "2026-10-29T12:00:30Z".parse().unwrap();
    let later = "2026-10-29T12:02:00Z".parse().unwrap();
    for (name, source) in sources {
        assert_eq!(source.state_at(now), "fresh");
        assert_eq!(source.state_at(later), "stale");
        assert!(source.tooltip(&name, now).contains("Last success:"));
    }
    for state in ["stale", "error", "partial", "unknown"] {
        let source = Source {
            state: state.into(),
            ..Default::default()
        };
        assert_eq!(source.state_at(now), state);
    }
    assert_eq!(Source::default().state_at(now), "unknown");
    let mut doc = fixtures::document(fixtures::CLEAN);
    doc.hosted = Some(hosted());
    assert!(doc.hosted.unwrap().readiness.dispatch.is_none());
}

#[test]
fn source_freshness_turnover_rebaselines_but_healthy_dispatch_changes_glow() {
    use dashboard_core::{changes::changed, readiness::Readiness};
    let mut doc = fixtures::document(fixtures::CLEAN);
    let mut h = hosted();
    h.readiness = Readiness {
        dispatch: Some(DispatchReason {
            project: "example".into(),
            reason: "held".into(),
            ..Default::default()
        }),
        sources: std::collections::BTreeMap::from([(
            "GitHub".into(),
            Source {
                state: "fresh".into(),
                last_success_at: Some("2026-10-29T12:00:00Z".into()),
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    doc.hosted = Some(h);
    let mut next = doc.clone();
    let source = next
        .hosted
        .as_mut()
        .unwrap()
        .readiness
        .sources
        .get_mut("GitHub")
        .unwrap();
    source.last_attempt_at = Some("2026-10-29T12:01:00Z".into());
    source.last_success_at = Some("2026-10-29T12:01:00Z".into());
    assert!(!changed(Some(&doc), &next));
    next.hosted
        .as_mut()
        .unwrap()
        .readiness
        .sources
        .get_mut("GitHub")
        .unwrap()
        .state = "error".into();
    next.hosted
        .as_mut()
        .unwrap()
        .readiness
        .dispatch
        .as_mut()
        .unwrap()
        .reason = "capacity_gated".into();
    assert!(
        !changed(Some(&doc), &next),
        "source outage rebaselines even when the explanation changed"
    );
    let recovery = doc.clone();
    assert!(!changed(Some(&next), &recovery));
    let mut real = recovery.clone();
    real.hosted
        .as_mut()
        .unwrap()
        .readiness
        .dispatch
        .as_mut()
        .unwrap()
        .reason = "dispatchable".into();
    assert!(changed(Some(&recovery), &real));
}
