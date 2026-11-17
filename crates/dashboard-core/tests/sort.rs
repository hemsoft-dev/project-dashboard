//! The Project table's per-column sort and name filter (#98).

use std::path::PathBuf;

use dashboard_core::cache::StatusCache;
use dashboard_core::fixtures::{self, UNAVAILABLE};
use dashboard_core::pmd::{Hosted, MergeCounts, OpenCounts};
use dashboard_core::priority::Priorities;
use dashboard_core::scheduler::Outcome;
use dashboard_core::source::FetchError;
use dashboard_core::status::{Issue, StatusDocument};
use dashboard_core::{Board, Card, Count, Direction, SortColumn, SortOrder};

/// One row's data. `None` counts are values pmd didn't supply.
struct Row {
    name: &'static str,
    prs: Option<u64>,
    issues: Option<u64>,
    merged_24h: Option<u64>,
    merged_7d: Option<u64>,
    /// An issue as the newest activity: its number, title and update time.
    activity: Option<(u64, &'static str, &'static str)>,
}

fn hosted(row: &Row) -> Hosted {
    Hosted {
        readiness: Default::default(),
        open_counts: OpenCounts {
            open_prs: row.prs,
            open_issues: row.issues,
        },
        merge_counts: MergeCounts {
            merged_24h: row.merged_24h,
            merged_7d: row.merged_7d,
        },
        service_url: "https://pmd.test".into(),
        generated_at: "2026-11-16T00:00:00Z".into(),
        checked_at: None,
        last_error: None,
        board: None,
        work: Vec::new(),
        backlog: Default::default(),
        work_error: None,
        work_transport_error: None,
    }
}

fn document(row: &Row) -> StatusDocument {
    let mut document = StatusDocument {
        schema_version: 1,
        hosted: Some(hosted(row)),
        ..StatusDocument::default()
    };
    document.repository.name_with_owner = format!("hemsoft-dev/{}", row.name);
    if let Some((number, title, at)) = row.activity {
        document.issues = vec![Issue {
            number,
            title: title.into(),
            updated_at: Some(at.into()),
            url: format!("https://github.com/hemsoft-dev/{}/issues/{number}", row.name),
            ..Issue::default()
        }];
    }
    document
}

/// A board with these rows in config order, then a loading row ("pending") that has no status at all.
fn board(rows: &[Row]) -> Board {
    let mut checkouts: Vec<PathBuf> = rows
        .iter()
        .map(|row| PathBuf::from(format!("/src/{}", row.name)))
        .collect();
    checkouts.push(PathBuf::from("/src/pending"));
    let mut cache = StatusCache::default();
    let mut board = Board::new(&checkouts, &cache);
    for (checkout, row) in checkouts.iter().zip(rows) {
        board.apply(
            Outcome {
                checkout: checkout.clone(),
                result: Ok(document(row)),
            },
            &mut cache,
        );
    }
    board
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "Zeta",
            prs: Some(10),
            issues: Some(0),
            merged_24h: Some(2),
            merged_7d: Some(10),
            activity: Some((3, "alpha fix", "2026-11-16T05:00:00Z")),
        },
        Row {
            name: "alpha",
            prs: Some(2),
            issues: Some(10),
            merged_24h: None,
            merged_7d: Some(2),
            activity: None,
        },
        Row {
            name: "delta",
            prs: Some(2),
            issues: Some(2),
            merged_24h: Some(0),
            merged_7d: None,
            activity: Some((9, "Beta thing", "2026-11-16T07:00:00Z")),
        },
        Row {
            name: "Beta",
            prs: Some(0),
            issues: None,
            merged_24h: Some(10),
            merged_7d: Some(2),
            activity: Some((12, "zebra", "2026-11-16T05:00:00Z")),
        },
    ]
}

const PRIORITIES: &str = r#"{"projects":[
 {"slug":"zeta","repo":"hemsoft-dev/Zeta","priority":3,"tracked":true},
 {"slug":"alpha","repo":"hemsoft-dev/alpha","priority":0,"tracked":true},
 {"slug":"beta","repo":"hemsoft-dev/Beta","priority":3,"tracked":true}] }"#;

/// The displayed names of `indices`.
fn names(board: &Board, indices: Vec<usize>) -> Vec<String> {
    indices
        .into_iter()
        .map(|ix| board.cards()[ix].name().trim_start_matches("hemsoft-dev/").to_owned())
        .collect()
}

fn sorted(board: &Board, column: SortColumn, direction: Direction) -> Vec<String> {
    names(board, board.rows(SortOrder::By(column, direction), "", session))
}

/// A T3 thread title for some rows; the rest have none.
fn session(card: &Card) -> Option<String> {
    match card.name().as_str() {
        "hemsoft-dev/Zeta" => Some("b: review".into()),
        "hemsoft-dev/delta" => Some("A: build".into()),
        "hemsoft-dev/Beta" => Some("a: build".into()),
        _ => None,
    }
}

use Direction::{Ascending, Descending};

#[test]
fn every_column_sorts_both_ways_with_missing_values_last_and_ties_in_config_order() {
    let mut board = board(&rows());
    board.set_priorities(Priorities::parse(PRIORITIES.as_bytes()).unwrap());
    let expect = |column, direction, expected: &[&str]| {
        assert_eq!(sorted(&board, column, direction), expected, "{column:?} {direction:?}");
    };
    // Displayed names ignore case. The loading row shows its folder name, "pending", after "hemsoft-dev/…".
    expect(
        SortColumn::Repository,
        Ascending,
        &["alpha", "Beta", "delta", "Zeta", "pending"],
    );
    expect(
        SortColumn::Repository,
        Descending,
        &["pending", "Zeta", "delta", "Beta", "alpha"],
    );
    // P0 first; equal P3s keep config order; no priority last in both directions.
    expect(
        SortColumn::Priority,
        Ascending,
        &["alpha", "Zeta", "Beta", "delta", "pending"],
    );
    expect(
        SortColumn::Priority,
        Descending,
        &["Zeta", "Beta", "alpha", "delta", "pending"],
    );
    // Numbers, not text: 10 is more than 2. Zero is a value; the loading row has none.
    expect(
        SortColumn::PullRequests,
        Ascending,
        &["Beta", "alpha", "delta", "Zeta", "pending"],
    );
    expect(
        SortColumn::PullRequests,
        Descending,
        &["Zeta", "alpha", "delta", "Beta", "pending"],
    );
    // Beta has no pmd issue count, so it shows (and sorts by) its status's one listed issue.
    expect(
        SortColumn::Issues,
        Ascending,
        &["Zeta", "Beta", "delta", "alpha", "pending"],
    );
    expect(
        SortColumn::Issues,
        Descending,
        &["alpha", "delta", "Beta", "Zeta", "pending"],
    );
    expect(
        SortColumn::Merged24h,
        Ascending,
        &["delta", "Zeta", "Beta", "alpha", "pending"],
    );
    expect(
        SortColumn::Merged24h,
        Descending,
        &["Beta", "Zeta", "delta", "alpha", "pending"],
    );
    expect(
        SortColumn::Merged7d,
        Ascending,
        &["alpha", "Beta", "Zeta", "delta", "pending"],
    );
    expect(
        SortColumn::Merged7d,
        Descending,
        &["Zeta", "alpha", "Beta", "delta", "pending"],
    );
    // Updated sorts by time: Zeta and Beta share 05:00 and keep config order.
    expect(
        SortColumn::Updated,
        Ascending,
        &["Zeta", "Beta", "delta", "alpha", "pending"],
    );
    expect(
        SortColumn::Updated,
        Descending,
        &["delta", "Zeta", "Beta", "alpha", "pending"],
    );
    // Last activity sorts by its displayed text ("Issue #12 zebra"), not its time.
    expect(
        SortColumn::Activity,
        Ascending,
        &["Beta", "Zeta", "delta", "alpha", "pending"],
    );
    expect(
        SortColumn::Activity,
        Descending,
        &["delta", "Zeta", "Beta", "alpha", "pending"],
    );
    // Thread titles ignore case, so "A: build" and "a: build" tie and keep config order.
    expect(
        SortColumn::Session,
        Ascending,
        &["delta", "Beta", "Zeta", "alpha", "pending"],
    );
    expect(
        SortColumn::Session,
        Descending,
        &["Zeta", "delta", "Beta", "alpha", "pending"],
    );
    assert_eq!(
        names(&board, board.rows(SortOrder::Config, "", session)),
        ["Zeta", "alpha", "delta", "Beta", "pending"]
    );
}

#[test]
fn ten_sorts_after_two_and_rows_without_a_count_follow_zero() {
    let rows = [2, 10, 1, 0]
        .into_iter()
        .zip(["two", "ten", "one", "zero"])
        .map(|(n, name)| Row {
            name,
            prs: Some(n),
            issues: Some(n),
            merged_24h: Some(n),
            merged_7d: Some(n),
            activity: None,
        })
        .collect::<Vec<_>>();
    let board = board(&rows);
    for column in [
        SortColumn::PullRequests,
        SortColumn::Issues,
        SortColumn::Merged24h,
        SortColumn::Merged7d,
    ] {
        assert_eq!(
            sorted(&board, column, Ascending),
            ["zero", "one", "two", "ten", "pending"]
        );
        assert_eq!(
            sorted(&board, column, Descending),
            ["ten", "two", "one", "zero", "pending"]
        );
    }
}

#[test]
fn a_failed_row_has_no_activity_text_and_sorts_last_by_activity() {
    let mut rows = rows();
    rows.truncate(1);
    let mut cache = StatusCache::default();
    let mut board = board(&rows);
    board.apply(
        Outcome {
            checkout: "/src/Zeta".into(),
            result: Err(FetchError::Failed("HTTP 500".into())),
        },
        &mut cache,
    );
    // The row keeps its last good activity time, but its activity cell shows the error.
    assert!(board.cards()[0].last_activity().is_some());
    assert_eq!(board.cards()[0].activity_text(), None);
    assert_eq!(sorted(&board, SortColumn::Activity, Ascending), ["Zeta", "pending"]);
}

#[test]
fn counts_display_and_sort_as_the_table_shows_them() {
    let unavailable = fixtures::document(UNAVAILABLE);
    assert_eq!(Count::of_section(&unavailable, "issues", 0), Count::Unavailable);
    assert_eq!(Count::of_section(&unavailable, "issues", 3), Count::Known(3));
    assert_eq!(Count::Unavailable.to_string(), "?");
    assert_eq!(Count::Off.to_string(), "–");
    assert_eq!(Count::Known(10).to_string(), "10");
    assert_eq!(Count::Unavailable.value(), None);
    assert_eq!(Count::Off.value(), None);
    assert_eq!(Count::Known(0).value(), Some(0));
    // pmd's count wins over the status's list, which fills in when pmd has none; no status shows "–".
    let board = board(&rows());
    assert_eq!(board.cards()[0].open_pull_requests(), Count::Known(10));
    assert_eq!(board.cards()[0].open_issues(), Count::Known(0));
    assert_eq!(board.cards()[3].open_issues(), Count::Known(1));
    assert_eq!(board.cards()[4].open_pull_requests(), Count::Off);
}

#[test]
fn each_header_cycles_its_first_direction_the_reverse_then_config() {
    for column in SortColumn::ALL {
        let first = column.first_direction();
        let once = SortOrder::Config.next_for(column);
        assert_eq!(once, SortOrder::By(column, first));
        assert_eq!(once.next_for(column), SortOrder::By(column, first.reverse()));
        assert_eq!(once.next_for(column).next_for(column), SortOrder::Config);
        // Another column's header starts that column over, whatever the current direction.
        for other in SortColumn::ALL.into_iter().filter(|&other| other != column) {
            assert_eq!(
                SortOrder::By(other, first.reverse()).next_for(column),
                SortOrder::By(column, first)
            );
        }
    }
    // Text and priority start ascending; counts and update times start with the largest.
    assert_eq!(SortColumn::Repository.first_direction(), Ascending);
    assert_eq!(SortColumn::Priority.first_direction(), Ascending);
    assert_eq!(SortColumn::Activity.first_direction(), Ascending);
    assert_eq!(SortColumn::Session.first_direction(), Ascending);
    assert_eq!(SortColumn::PullRequests.first_direction(), Descending);
    assert_eq!(SortColumn::Updated.first_direction(), Descending);
    // The cold-start order and the original Updated and Priority cycles are unchanged.
    assert_eq!(SortOrder::NEWEST_FIRST.next(), SortOrder::OLDEST_FIRST);
    assert_eq!(SortOrder::Config.next_priority(), SortOrder::PRIORITY_FIRST);
    assert_eq!(SortOrder::PRIORITY_FIRST.next_priority(), SortOrder::PRIORITY_LAST);
    assert_eq!(SortOrder::PRIORITY_LAST.next_priority(), SortOrder::Config);
}

#[test]
fn headers_say_how_they_sort() {
    let by = |column, direction| SortOrder::By(column, direction);
    assert_eq!(SortOrder::Config.label(SortColumn::Issues), "Issues, not sorted");
    assert_eq!(SortOrder::NEWEST_FIRST.label(SortColumn::Issues), "Issues, not sorted");
    assert_eq!(
        by(SortColumn::Issues, Descending).label(SortColumn::Issues),
        "Issues, most first"
    );
    assert_eq!(
        by(SortColumn::PullRequests, Ascending).label(SortColumn::PullRequests),
        "PRs, fewest first"
    );
    assert_eq!(
        by(SortColumn::Repository, Ascending).label(SortColumn::Repository),
        "Repository, A to Z"
    );
    assert_eq!(
        by(SortColumn::Session, Descending).label(SortColumn::Session),
        "T3 session, Z to A"
    );
    assert_eq!(
        by(SortColumn::Activity, Ascending).label(SortColumn::Activity),
        "Last activity, A to Z"
    );
    assert_eq!(
        by(SortColumn::Merged24h, Descending).label(SortColumn::Merged24h),
        "Merged 24h, most first"
    );
    assert_eq!(
        SortOrder::NEWEST_FIRST.label(SortColumn::Updated),
        "Updated, newest first"
    );
    assert_eq!(
        SortOrder::OLDEST_FIRST.label(SortColumn::Updated),
        "Updated, oldest first"
    );
    assert_eq!(
        SortOrder::PRIORITY_FIRST.label(SortColumn::Priority),
        "Priority, highest first"
    );
    assert_eq!(
        SortOrder::PRIORITY_LAST.label(SortColumn::Priority),
        "Priority, lowest first"
    );
    assert_eq!(Ascending.arrow(), "↑");
    assert_eq!(Descending.arrow(), "↓");
    assert_eq!(
        SortOrder::NEWEST_FIRST.direction_of(SortColumn::Updated),
        Some(Descending)
    );
    assert_eq!(SortOrder::NEWEST_FIRST.direction_of(SortColumn::Priority), None);
}

#[test]
fn the_name_filter_ignores_case_and_composes_with_the_sort() {
    let board = board(&rows());
    let filtered = |filter: &str, sort| names(&board, board.rows(sort, filter, session));
    assert_eq!(
        filtered("", SortOrder::Config),
        ["Zeta", "alpha", "delta", "Beta", "pending"]
    );
    // Case-insensitive substring of the displayed name, owner included; surrounding spaces don't count.
    assert_eq!(filtered("ETA", SortOrder::Config), ["Zeta", "Beta"]);
    assert_eq!(filtered("  eta ", SortOrder::Config), ["Zeta", "Beta"]);
    assert_eq!(filtered("hemsoft-dev/a", SortOrder::Config), ["alpha"]);
    assert_eq!(filtered("pend", SortOrder::Config), ["pending"]);
    assert_eq!(filtered("   ", SortOrder::Config).len(), 5);
    // No match leaves no rows.
    assert!(filtered("nothing-like-this", SortOrder::Config).is_empty());
    // The filter keeps the sort's order among the rows it leaves.
    assert_eq!(
        filtered("eta", SortOrder::By(SortColumn::PullRequests, Ascending)),
        ["Beta", "Zeta"]
    );
    assert_eq!(
        filtered("eta", SortOrder::By(SortColumn::Repository, Descending)),
        ["Zeta", "Beta"]
    );
    // Clearing it restores every row in the current order.
    assert_eq!(
        filtered("", SortOrder::By(SortColumn::PullRequests, Descending)),
        ["Zeta", "alpha", "delta", "Beta", "pending"]
    );
    // `order` is the unfiltered table.
    assert_eq!(board.order(SortOrder::Config), [0, 1, 2, 3, 4]);
}
