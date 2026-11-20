//! The Backlog's column sort (#118): within each stage group, by one column in either direction, with rows that
//! show no value last and ties in pmd's dispatch order.

use dashboard_core::backlog::{self, Row};
use dashboard_core::pmd::{Hosted, MergeCounts, OpenCounts, WorkItem};
use dashboard_core::status::StatusDocument;
use dashboard_core::{BacklogColumn, BacklogSort, Direction};
use serde_json::json;

/// A work item. `holder` is a live assignment's machine and model.
fn item(
    id: &str,
    project: &str,
    (kind, number): (&str, u64),
    status: &str,
    priority: Option<u8>,
    rank: i64,
    holder: Option<(&str, &str)>,
) -> WorkItem {
    let mut value = json!({
        "id": id, "project": project, "kind": kind, "number": number, "title": id,
        "url": format!("https://github.com/hemsoft-dev/{project}/issues/{number}"),
        "status": status, "ghState": "open", "priority": priority, "rank": rank,
    });
    if let Some((machine, model)) = holder {
        value["assignment"] =
            json!({ "id": format!("{id}-a"), "machine": machine, "model": model, "state": "running" });
    }
    serde_json::from_value(value).unwrap()
}

fn document(work: Vec<WorkItem>) -> StatusDocument {
    let hosted = Hosted {
        readiness: Default::default(),
        open_counts: OpenCounts::default(),
        merge_counts: MergeCounts::default(),
        service_url: "https://pmd.test".into(),
        generated_at: "2026-11-20T00:00:00Z".into(),
        checked_at: None,
        last_error: None,
        board: None,
        work,
        backlog: Default::default(),
        work_error: None,
        work_transport_error: None,
    };
    StatusDocument {
        schema_version: 1,
        hosted: Some(hosted),
        ..StatusDocument::default()
    }
}

/// Three projects, two capitalized, with rows in every stage group. Byte order would put "Alpha" and "Gamma" before
/// "beta"; ignoring case puts them alpha, beta, gamma.
/// In progress: g2. Up next: b1, b2, a1. Waiting / Blocked: b3 (blocked), a2 (no reason), g1 (waiting on Franz, held).
/// P4 and later: a4 (P4), b4 (P5), a3 (no priority).
fn documents() -> Vec<StatusDocument> {
    vec![
        document(vec![
            item("b1", "beta", ("issue", 12), "ready", Some(1), 1, None),
            item("b2", "beta", ("pr", 4), "ready", Some(1), 2, None),
            item("b3", "beta", ("issue", 30), "blocked", Some(0), 0, None),
            item("b4", "beta", ("issue", 31), "ready", Some(5), 0, None),
        ]),
        document(vec![
            item("a1", "Alpha", ("issue", 7), "ready", Some(2), 0, None),
            item("a2", "Alpha", ("issue", 8), "triage", Some(1), 0, None),
            item("a3", "Alpha", ("issue", 40), "ready", None, 0, None),
            item("a4", "Alpha", ("issue", 41), "ready", Some(4), 0, None),
        ]),
        document(vec![
            item(
                "g1",
                "Gamma",
                ("issue", 2),
                "waiting_on_franz",
                Some(1),
                0,
                Some(("mini", "opus")),
            ),
            item(
                "g2",
                "Gamma",
                ("pr", 1),
                "in_progress",
                Some(3),
                0,
                Some(("box", "sol")),
            ),
        ]),
    ]
}

fn sorted(sort: BacklogSort) -> Vec<String> {
    let documents = documents();
    let mut rows: Vec<Row> = backlog::rows(&documents);
    sort.apply(&mut rows);
    rows.iter().map(|r| r.items[0].id.clone()).collect()
}

const DISPATCH: [&str; 10] = ["g2", "b1", "b2", "a1", "b3", "a2", "g1", "a4", "b4", "a3"];

#[test]
fn the_fixture_has_the_stages_reasons_and_holders_the_sorts_read() {
    let documents = documents();
    let rows = backlog::rows(&documents);
    let by_id = |id: &str| rows.iter().find(|r| r.items[0].id == id).unwrap();
    assert_eq!(by_id("b3").reason, "blocked");
    assert_eq!(by_id("a2").reason, "");
    assert_eq!(by_id("g1").reason, "waiting on Franz");
    assert_eq!(by_id("g1").workers, ["mini · opus"]);
    assert_eq!(by_id("a3").priority(), None);
    assert_eq!(by_id("a4").priority(), Some(4));
    assert_eq!(sorted(BacklogSort::Dispatch), DISPATCH);
}

#[test]
fn each_column_sorts_within_its_stage_group_both_ways_with_missing_values_last() {
    use BacklogColumn::*;
    use Direction::*;
    let cases: [(BacklogColumn, Direction, [&str; 10]); 10] = [
        // Case is ignored (alpha, beta, Gamma, not Alpha, Gamma, beta); equal projects keep dispatch order.
        (
            Project,
            Ascending,
            ["g2", "a1", "b1", "b2", "a2", "b3", "g1", "a4", "a3", "b4"],
        ),
        (
            Project,
            Descending,
            ["g2", "b1", "b2", "a1", "g1", "b3", "a2", "b4", "a4", "a3"],
        ),
        // a3 has no priority and stays last both ways.
        (
            Priority,
            Ascending,
            ["g2", "b1", "b2", "a1", "b3", "a2", "g1", "a4", "b4", "a3"],
        ),
        (
            Priority,
            Descending,
            ["g2", "a1", "b1", "b2", "a2", "g1", "b3", "b4", "a4", "a3"],
        ),
        // Issues before pull requests, then by number: #7, #12, PR #4.
        (
            Item,
            Ascending,
            ["g2", "a1", "b1", "b2", "g1", "a2", "b3", "b4", "a3", "a4"],
        ),
        (
            Item,
            Descending,
            ["g2", "b2", "b1", "a1", "b3", "a2", "g1", "a4", "a3", "b4"],
        ),
        // a2 has no reason; rows without one keep dispatch order among themselves.
        (
            Reason,
            Ascending,
            ["g2", "b1", "b2", "a1", "b3", "g1", "a2", "a4", "b4", "a3"],
        ),
        (
            Reason,
            Descending,
            ["g2", "b1", "b2", "a1", "g1", "b3", "a2", "a4", "b4", "a3"],
        ),
        // Only g1 and g2 have a holder.
        (
            Holder,
            Ascending,
            ["g2", "b1", "b2", "a1", "g1", "b3", "a2", "a4", "b4", "a3"],
        ),
        (
            Holder,
            Descending,
            ["g2", "b1", "b2", "a1", "g1", "b3", "a2", "a4", "b4", "a3"],
        ),
    ];
    for (column, direction, expected) in cases {
        assert_eq!(
            sorted(BacklogSort::By(column, direction)),
            expected,
            "{column:?} {direction:?}"
        );
    }
}

#[test]
fn holders_compare_by_their_text_ignoring_case() {
    let documents = vec![document(vec![
        item("z", "p", ("issue", 1), "in_progress", Some(1), 0, Some(("Zeta", "m"))),
        item("y", "p", ("issue", 2), "in_progress", Some(1), 0, Some(("alpha", "m"))),
        item("x", "p", ("issue", 3), "in_progress", Some(1), 0, Some(("Mid", "m"))),
    ])];
    let mut rows = backlog::rows(&documents);
    let ids = |rows: &[Row]| rows.iter().map(|r| r.items[0].id.clone()).collect::<Vec<_>>();
    assert_eq!(
        ids(&rows),
        ["x", "y", "z"],
        "dispatch: equal priority and rank, then id"
    );
    BacklogSort::By(BacklogColumn::Holder, Direction::Ascending).apply(&mut rows);
    assert_eq!(ids(&rows), ["y", "x", "z"]);
    BacklogSort::By(BacklogColumn::Holder, Direction::Descending).apply(&mut rows);
    assert_eq!(ids(&rows), ["z", "x", "y"]);
}

#[test]
fn reasons_compare_ignoring_case() {
    // Reasons are pmd's status reason plus the waiting status, sorted and joined: "Zulu · blocked", "apple · blocked",
    // "blocked". Byte order would put "Zulu" first.
    let mut documents = vec![document(vec![
        item("r1", "p", ("issue", 1), "blocked", Some(1), 0, None),
        item("r2", "p", ("issue", 2), "blocked", Some(1), 0, None),
        item("r3", "p", ("issue", 3), "blocked", Some(1), 0, None),
    ])];
    let work = &mut documents[0].hosted.as_mut().unwrap().work;
    work[0].detail.status_reason = Some("Zulu".into());
    work[1].detail.status_reason = Some("apple".into());
    let mut rows = backlog::rows(&documents);
    let reasons = |rows: &[Row]| rows.iter().map(|r| r.reason.clone()).collect::<Vec<_>>();
    assert_eq!(reasons(&rows), ["Zulu · blocked", "apple · blocked", "blocked"]);
    BacklogSort::By(BacklogColumn::Reason, Direction::Ascending).apply(&mut rows);
    assert_eq!(reasons(&rows), ["apple · blocked", "blocked", "Zulu · blocked"]);
    BacklogSort::By(BacklogColumn::Reason, Direction::Descending).apply(&mut rows);
    assert_eq!(reasons(&rows), ["Zulu · blocked", "blocked", "apple · blocked"]);
}

#[test]
fn each_header_cycles_ascending_descending_then_dispatch_order() {
    for column in BacklogColumn::ALL {
        let first = BacklogSort::Dispatch.next_for(column);
        assert_eq!(first, BacklogSort::By(column, Direction::Ascending));
        let second = first.next_for(column);
        assert_eq!(second, BacklogSort::By(column, Direction::Descending));
        assert_eq!(second.next_for(column), BacklogSort::Dispatch);
        // Another header starts its own column ascending, from either direction.
        for other in BacklogColumn::ALL.into_iter().filter(|&c| c != column) {
            assert_eq!(first.next_for(other), BacklogSort::By(other, Direction::Ascending));
            assert_eq!(second.next_for(other), BacklogSort::By(other, Direction::Ascending));
        }
    }
}

#[test]
fn headers_say_how_they_sort() {
    let label = |column, direction| BacklogSort::By(column, direction).label(column);
    use BacklogColumn::*;
    use Direction::*;
    assert_eq!(label(Project, Ascending), "Project, A to Z");
    assert_eq!(label(Project, Descending), "Project, Z to A");
    assert_eq!(label(Priority, Ascending), "Priority, highest first");
    assert_eq!(label(Priority, Descending), "Priority, lowest first");
    assert_eq!(label(Item, Ascending), "Item, issues first, lowest number first");
    assert_eq!(
        label(Item, Descending),
        "Item, pull requests first, highest number first"
    );
    assert_eq!(label(Reason, Ascending), "Reason, A to Z");
    assert_eq!(label(Holder, Descending), "Holder, Z to A");
    assert_eq!(BacklogSort::Dispatch.label(Holder), "Holder, not sorted");
    assert_eq!(
        BacklogSort::By(Project, Ascending).label(Priority),
        "Priority, not sorted"
    );
    assert_eq!(BacklogSort::By(Item, Ascending).direction_of(Item), Some(Ascending));
    assert_eq!(BacklogSort::By(Item, Ascending).direction_of(Reason), None);
    assert_eq!(BacklogSort::default(), BacklogSort::Dispatch);
}
