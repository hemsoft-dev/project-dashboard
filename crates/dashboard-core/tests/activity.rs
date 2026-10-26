//! Last activity: the newest PR, issue or workflow run in a status document, and the table's sort order.

use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, Utc};
use dashboard_core::activity::{ActivityKind, ago, last_activity, time_in};
use dashboard_core::cache::StatusCache;
use dashboard_core::fixtures::{self, ATTENTION, CLEAN, UNAVAILABLE};
use dashboard_core::scheduler::Outcome;
use dashboard_core::status::{Issue, PullRequest, PullRequests, StatusDocument, WorkflowRun};
use dashboard_core::{Board, SortOrder};

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
}

fn pr(number: u64, updated_at: Option<&str>) -> PullRequest {
    PullRequest {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        updated_at: updated_at.map(str::to_owned),
        ..PullRequest::default()
    }
}

fn issue(number: u64, updated_at: Option<&str>) -> Issue {
    Issue {
        number,
        url: format!("https://github.com/o/r/issues/{number}"),
        updated_at: updated_at.map(str::to_owned),
        ..Issue::default()
    }
}

fn run(id: &str, started_at: Option<&str>, created_at: Option<&str>) -> WorkflowRun {
    WorkflowRun {
        id: id.into(),
        workflow: "CI".into(),
        url: format!("https://github.com/o/r/actions/runs/{id}"),
        started_at: started_at.map(str::to_owned),
        created_at: created_at.map(str::to_owned),
        ..WorkflowRun::default()
    }
}

#[test]
fn the_fixtures_newest_items_win() {
    let clean = last_activity(&fixtures::document(CLEAN)).unwrap();
    assert_eq!(
        (clean.kind, clean.label.as_str()),
        (ActivityKind::WorkflowRun, "CI run")
    );
    assert_eq!(clean.at, at("2026-10-19T04:12:35Z"));
    assert_eq!(
        clean.url,
        "https://github.com/hemsoft-dev/gh-x/actions/runs/18311000001"
    );

    let attention = last_activity(&fixtures::document(ATTENTION)).unwrap();
    assert_eq!(attention.at, at("2026-10-21T06:26:02Z"));
    assert_eq!(attention.title, "chore(deps): bump the cargo group with 3 updates");

    // Two sections unavailable and nothing else with a time.
    assert_eq!(last_activity(&fixtures::document(UNAVAILABLE)), None);
}

#[test]
fn real_gh_x_output_picks_the_newest_issue() {
    let doc = StatusDocument::parse(include_str!("fixtures/real-gh-x-v0.22.0-codexbar.json")).unwrap();
    let newest = last_activity(&doc).unwrap();
    assert_eq!(
        (newest.kind, newest.label.as_str()),
        (ActivityKind::Issue, "Issue #233")
    );
    assert_eq!(newest.at, at("2026-10-21T08:06:49Z"));
    assert_eq!(newest.url, "https://github.com/hemsoft-dev/codexbar/issues/233");
}

#[test]
fn prs_issues_and_runs_all_compete_including_merged_prs() {
    let mut doc = StatusDocument::default();
    doc.pull_requests.open = vec![pr(1, Some("2026-10-21T06:00:00Z"))];
    doc.issues = vec![issue(2, Some("2026-10-21T07:00:00Z"))];
    doc.workflow_runs = vec![run("3", Some("2026-10-21T08:00:00Z"), None)];
    assert_eq!(last_activity(&doc).unwrap().kind, ActivityKind::WorkflowRun);
    doc.pull_requests.merged = vec![pr(4, Some("2026-10-21T09:00:00Z"))];
    let newest = last_activity(&doc).unwrap();
    assert_eq!(
        (newest.kind, newest.label.as_str()),
        (ActivityKind::PullRequest, "PR #4")
    );
}

#[test]
fn ties_go_to_open_prs_then_merged_prs_then_issues_then_runs() {
    let same = Some("2026-10-21T06:00:00Z");
    let mut doc = StatusDocument {
        workflow_runs: vec![run("9", same, None)],
        issues: vec![issue(7, same), issue(8, same)],
        ..StatusDocument::default()
    };
    assert_eq!(last_activity(&doc).unwrap().label, "Issue #7");
    doc.pull_requests.merged = vec![pr(5, same)];
    assert_eq!(last_activity(&doc).unwrap().label, "PR #5");
    doc.pull_requests.open = vec![pr(3, same)];
    assert_eq!(last_activity(&doc).unwrap().label, "PR #3");
}

#[test]
fn missing_or_unreadable_timestamps_are_skipped_and_runs_fall_back_to_created_at() {
    let mut doc = StatusDocument::default();
    doc.pull_requests.open = vec![pr(1, None), pr(2, Some("not a time"))];
    doc.issues = vec![issue(3, None)];
    assert_eq!(last_activity(&doc), None);
    doc.workflow_runs = vec![run("4", None, Some("2026-10-21T05:00:00Z")), run("5", None, None)];
    let newest = last_activity(&doc).unwrap();
    assert_eq!(newest.url, "https://github.com/o/r/actions/runs/4");
    assert_eq!(newest.at, at("2026-10-21T05:00:00Z"));
    // A started run reads its start, not its creation.
    doc.workflow_runs = vec![run("6", Some("2026-10-21T04:00:00Z"), Some("2026-10-21T09:00:00Z"))];
    assert_eq!(last_activity(&doc).unwrap().at, at("2026-10-21T04:00:00Z"));
}

#[test]
fn fractional_seconds_and_offsets_compare_as_instants() {
    let doc = StatusDocument {
        issues: vec![issue(1, Some("2026-10-21T08:00:00.900Z"))],
        pull_requests: PullRequests {
            open: vec![pr(2, Some("2026-10-21T04:00:00.5-04:00"))],
            ..PullRequests::default()
        },
        ..StatusDocument::default()
    };
    // 04:00:00.5-04:00 is 08:00:00.5Z, before the issue's 08:00:00.9Z.
    assert_eq!(last_activity(&doc).unwrap().label, "Issue #1");
}

#[test]
fn the_activity_reads_its_label_age_and_time() {
    let mut doc = StatusDocument::default();
    doc.pull_requests.open = vec![pr(12, Some("2026-10-21T06:16:00Z"))];
    let newest = last_activity(&doc).unwrap();
    assert_eq!(newest.label, "PR #12");
    assert_eq!(newest.age(at("2026-10-21T06:30:00Z")), "14m ago");
    // US Eastern in October is four hours behind UTC.
    let eastern = FixedOffset::west_opt(4 * 3600).unwrap();
    assert_eq!(time_in(newest.at, &eastern), "2026-10-21 02:16");
    assert_eq!(time_in(newest.at, &Utc), "2026-10-21 06:16");
}

#[test]
fn ages_read_in_the_largest_whole_unit() {
    let now = at("2026-10-21T12:00:00Z");
    assert_eq!(ago(at("2026-10-21T11:59:30Z"), now), "just now");
    assert_eq!(ago(at("2026-10-21T13:00:00Z"), now), "just now");
    assert_eq!(ago(at("2026-10-21T11:46:00Z"), now), "14m ago");
    assert_eq!(ago(at("2026-10-21T09:00:00Z"), now), "3h ago");
    assert_eq!(ago(at("2026-10-19T12:00:00Z"), now), "2d ago");
}

fn board_with(times: &[Option<&str>]) -> Board {
    let checkouts: Vec<PathBuf> = (0..times.len())
        .map(|ix| PathBuf::from(format!("/src/r{ix}")))
        .collect();
    let mut cache = StatusCache::default();
    let mut board = Board::new(&checkouts, &cache);
    for (checkout, time) in checkouts.into_iter().zip(times) {
        let mut doc = StatusDocument {
            schema_version: 1,
            ..StatusDocument::default()
        };
        doc.issues = vec![issue(1, *time)];
        board.apply(
            Outcome {
                checkout,
                result: Ok(doc),
            },
            &mut cache,
        );
    }
    board
}

#[test]
fn config_order_is_the_default_and_update_time_sorts_both_ways() {
    let board = board_with(&[
        Some("2026-10-21T05:00:00Z"),
        None,
        Some("2026-10-21T07:00:00Z"),
        Some("2026-10-21T05:00:00Z"),
        Some("2026-10-21T06:00:00Z"),
        None,
    ]);
    assert_eq!(SortOrder::default(), SortOrder::Config);
    assert_eq!(board.order(SortOrder::Config), [0, 1, 2, 3, 4, 5]);
    // Ties keep config order; repositories without an update time go last, in config order, either way.
    assert_eq!(board.order(SortOrder::NewestFirst), [2, 4, 0, 3, 1, 5]);
    assert_eq!(board.order(SortOrder::OldestFirst), [0, 3, 4, 2, 1, 5]);
}

#[test]
fn the_sort_cycles_newest_oldest_then_config() {
    assert_eq!(SortOrder::Config.next(), SortOrder::NewestFirst);
    assert_eq!(SortOrder::NewestFirst.next(), SortOrder::OldestFirst);
    assert_eq!(SortOrder::OldestFirst.next(), SortOrder::Config);
}

#[test]
fn loading_and_failed_cards_without_a_status_sort_last() {
    let mut cache = StatusCache::default();
    let checkouts = vec![
        PathBuf::from("/src/loading"),
        PathBuf::from("/src/failed"),
        PathBuf::from("/src/ok"),
    ];
    let mut board = Board::new(&checkouts, &cache);
    board.apply(
        Outcome {
            checkout: "/src/failed".into(),
            result: Err(dashboard_core::source::FetchError::MissingToken),
        },
        &mut cache,
    );
    board.apply(
        Outcome {
            checkout: "/src/ok".into(),
            result: Ok(fixtures::document(CLEAN)),
        },
        &mut cache,
    );
    assert_eq!(board.order(SortOrder::NewestFirst), [2, 0, 1]);
    assert_eq!(board.order(SortOrder::OldestFirst), [2, 0, 1]);
}
