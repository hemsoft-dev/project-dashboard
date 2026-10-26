//! Parsing the `gh x status --json` fixtures (gh-x#230, schema version 1) and reading gh-x's verdicts from them.

use dashboard_core::fixtures::{self, ATTENTION, CLEAN, ERROR, UNAVAILABLE};
use dashboard_core::status::{ParseError, StatusDocument};

#[test]
fn clean_fixture_parses_every_section() {
    let doc = fixtures::document(CLEAN);
    assert_eq!(doc.schema_version, 1);
    assert_eq!(doc.display_name(), "hemsoft-dev/gh-x");
    assert_eq!(doc.repository.default_branch, "main");
    assert!(doc.issues.is_empty());
    assert!(doc.pull_requests.open.is_empty());
    assert_eq!(doc.pull_requests.merged.len(), 2);
    assert_eq!(doc.pull_requests.merged_limit, 5);
    assert_eq!(doc.workflow_runs.len(), 1);
    assert_eq!(doc.sections.len(), 4);
    assert!(doc.sections.iter().all(|section| section.cached));
    assert_eq!(
        doc.pull_requests.merged[0].url,
        "https://github.com/hemsoft-dev/gh-x/pull/229"
    );
}

#[test]
fn clean_fixture_shows_the_terminal_health_rows_all_clean() {
    let doc = fixtures::document(CLEAN);
    let rows = doc.health_rows();
    let shown: Vec<(&str, &str, bool)> = rows
        .iter()
        .map(|row| (row.label.as_str(), row.text.as_str(), row.clean))
        .collect();
    assert_eq!(
        shown,
        [
            ("Main", "synced with origin/main · Clean working tree", true),
            ("Branches", "1 local (0 dangling) · 1 remote", true),
            ("Worktrees", "1 total · 0 cleanup candidates", true),
            ("Stashes", "0 stashes", true),
        ]
    );
    assert!(doc.is_clean());
}

#[test]
fn attention_fixture_keeps_gh_x_verdicts_and_adds_the_current_branch_row() {
    let doc = fixtures::document(ATTENTION);
    let rows = doc.health_rows();
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows[0].text,
        "behind origin/main by 3 commits · 2 modified files, 1 untracked file"
    );
    assert!(!rows[0].clean);
    assert_eq!(rows[2].text, "2 total · 1 cleanup candidate");
    assert_eq!(rows[3].text, "2 stashes");
    assert_eq!(rows[4].label, "Current");
    assert_eq!(
        rows[4].text,
        "feature/issue-231-tray · ahead of origin/feature/issue-231-tray by 2 commits · 2 modified files, 1 untracked file"
    );
    assert!(!doc.is_clean());
    assert_eq!(doc.pull_requests.open.len(), 2);
    assert_eq!(doc.pull_requests.open[0].ai_clean, Some(false));
    assert_eq!(doc.issues.len(), 2);
    assert!(doc.sections.iter().all(|section| !section.cached));
}

#[test]
fn unavailable_sections_and_rows_carry_gh_x_reasons() {
    let doc = fixtures::document(UNAVAILABLE);
    let unavailable: Vec<&str> = doc
        .unavailable_sections()
        .map(|section| section.name.as_str())
        .collect();
    assert_eq!(unavailable, ["issues", "pullRequests.open"]);
    assert!(doc.sections[0].reason.contains("HTTP 502"));
    let stashes = &doc.health_rows()[3];
    assert!(stashes.unavailable);
    assert_eq!(stashes.text, "Unavailable: git stash list: exit status 128");
    assert!(!doc.is_clean());
}

#[test]
fn error_object_is_reported_with_gh_x_message() {
    let err = StatusDocument::parse(ERROR).unwrap_err();
    assert_eq!(
        err,
        ParseError::Reported {
            code: "status_failed".into(),
            message: "not a git repository (or any of the parent directories): .git".into()
        }
    );
}

#[test]
fn other_schema_versions_are_refused() {
    let err = StatusDocument::parse(r#"{"schemaVersion": 2}"#).unwrap_err();
    assert_eq!(err, ParseError::UnsupportedSchema(2));
    assert!(err.to_string().contains("Update the app"));
}

#[test]
fn malformed_output_is_an_error_not_a_panic() {
    assert!(matches!(
        StatusDocument::parse("Loading…"),
        Err(ParseError::Malformed(_))
    ));
    assert!(matches!(StatusDocument::parse("[]"), Err(ParseError::Malformed(_))));
}

#[test]
fn unknown_and_missing_fields_parse_within_schema_one() {
    let doc = StatusDocument::parse(
        r#"{"schemaVersion": 1, "futureField": {"x": 1}, "repository": {"rootPath": "/src/demo/"}}"#,
    )
    .unwrap();
    assert_eq!(doc.display_name(), "demo");
    assert!(doc.issues.is_empty());
}

#[test]
fn a_detached_head_gets_its_current_row() {
    let mut doc = fixtures::document(CLEAN);
    doc.health.as_mut().unwrap().current_branch.name.clear();
    doc.health.as_mut().unwrap().current_branch.upstream.clear();
    doc.health.as_mut().unwrap().current_branch.verdict.clean = false;
    let rows = doc.health_rows();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[4].label, "Current");
    assert!(
        rows[4].text.starts_with("detached HEAD · no upstream configured"),
        "{}",
        rows[4].text
    );
    assert!(!rows[4].clean);
    assert!(!doc.is_clean());
}

#[test]
fn a_document_without_a_current_branch_object_has_no_current_row() {
    let doc = StatusDocument::parse(r#"{"schemaVersion": 1, "health": {"defaultBranch": {"status": "available", "clean": true, "name": "main", "checkedOut": true, "upstream": "origin/main", "changes": {"clean": true}}}}"#).unwrap();
    assert!(doc.health_rows().iter().all(|row| row.label != "Current"));
}

#[test]
fn an_overall_verdict_from_gh_x_wins_over_the_rows() {
    let mut doc = fixtures::document(CLEAN);
    assert!(doc.is_clean());
    doc.verdict = Some(dashboard_core::status::Verdict {
        status: "available".into(),
        reason: String::new(),
        clean: false,
    });
    assert!(!doc.is_clean());
    let attention =
        StatusDocument::parse(&ATTENTION.replacen("{", r#"{"verdict": {"status": "available", "clean": true},"#, 1))
            .unwrap();
    assert!(attention.is_clean());
}

/// Real `gh x status --json` output from gh-x built at hemsoft-dev/gh-x#231's head 690d829 (the gh-x#230 work), run
/// in the hemsoft-dev/gh-x checkout on 2026-10-21. The terminal view of the same checkout printed:
/// `✓ Main synced with origin/main · Clean working tree`, `Branches 3 local (1 dangling) · 3 remote`,
/// `Worktrees 2 total · 0 cleanup candidates`, `✓ Stashes 0 stashes`, open issues (1), open pull requests (1),
/// recently merged (5) and workflow runs (5).
const REAL_GH_X_PR231: &str = include_str!("fixtures/real-gh-x-pr231-690d829.json");

#[test]
fn real_gh_x_output_parses_and_matches_its_terminal_view() {
    let doc = StatusDocument::parse(REAL_GH_X_PR231).unwrap();
    assert_eq!(doc.display_name(), "hemsoft-dev/gh-x");
    let rows: Vec<(String, String, bool)> = doc
        .health_rows()
        .into_iter()
        .map(|row| (row.label, row.text, row.clean))
        .collect();
    let expected = [
        ("Main", "synced with origin/main · Clean working tree", true),
        ("Branches", "3 local (1 dangling) · 3 remote", false),
        ("Worktrees", "2 total · 0 cleanup candidates", false),
        ("Stashes", "0 stashes", true),
    ];
    assert_eq!(
        rows,
        expected.map(|(label, text, clean)| (label.to_owned(), text.to_owned(), clean))
    );
    assert_eq!(doc.issues.len(), 1);
    assert_eq!(doc.pull_requests.open.len(), 1);
    assert_eq!(doc.pull_requests.merged.len(), 5);
    assert_eq!(doc.workflow_runs.len(), 5);
    assert!(doc.unavailable_sections().next().is_none());
    assert!(
        doc.sections
            .iter()
            .all(|section| section.cached && section.fetched_at.is_some())
    );
    assert!(!doc.is_clean());
    assert_eq!(
        doc.pull_requests.open[0].url,
        "https://github.com/hemsoft-dev/gh-x/pull/231"
    );
}

/// Real output of the released gh-x v0.22.0 (the merged gh-x#230 producer) in the codexbar checkout on
/// 2026-10-21. Its terminal view at the same time: `Main behind origin/main by 95 commits · Clean working tree`,
/// `Branches 1 local (0 dangling) · 28 remote`, `Worktrees 15 total · 0 cleanup candidates`, `✓ Stashes 0 stashes`,
/// open issues (1), open pull requests (0), recently merged (5), workflow runs ✓ ✓ ! ✓ ✓.
const REAL_V022_CODEXBAR: &str = include_str!("fixtures/real-gh-x-v0.22.0-codexbar.json");

/// Real gh-x v0.22.0 output of `gh x status --json --merged 0` in the project-dashboard checkout on 2026-10-21. Its
/// terminal view: `✓ Main synced with origin/main · Clean working tree`, `Branches 2 local (0 dangling) · 1 remote`,
/// `Worktrees 2 total · 1 cleanup candidate`, `✓ Stashes 0 stashes`, open issues (2), open pull requests (0), no
/// merged list, workflow runs * * ✓ ✓ ✓.
const REAL_V022_MERGED0: &str = include_str!("fixtures/real-gh-x-v0.22.0-merged0.json");

fn rows_of(doc: &StatusDocument) -> Vec<(String, String, bool)> {
    doc.health_rows()
        .into_iter()
        .map(|row| (row.label, row.text, row.clean))
        .collect()
}

fn owned(rows: &[(&str, &str, bool)]) -> Vec<(String, String, bool)> {
    rows.iter()
        .map(|(label, text, clean)| ((*label).to_owned(), (*text).to_owned(), *clean))
        .collect()
}

#[test]
fn released_gh_x_output_matches_its_terminal_view() {
    use dashboard_core::status::RunOutcome::{Attention, Success};
    let doc = StatusDocument::parse(REAL_V022_CODEXBAR).unwrap();
    assert_eq!(doc.display_name(), "HemSoft/codexbar");
    assert_eq!(
        rows_of(&doc),
        owned(&[
            ("Main", "behind origin/main by 95 commits · Clean working tree", false),
            ("Branches", "1 local (0 dangling) · 28 remote", false),
            ("Worktrees", "15 total · 0 cleanup candidates", false),
            ("Stashes", "0 stashes", true),
        ])
    );
    assert_eq!(doc.issues.len(), 1);
    assert_eq!(doc.pull_requests.open.len(), 0);
    assert_eq!(doc.pull_requests.merged.len(), 5);
    let outcomes: Vec<_> = doc.workflow_runs.iter().map(|run| run.outcome()).collect();
    assert_eq!(outcomes, [Success, Success, Attention, Success, Success]);
    assert_eq!(doc.workflow_runs_perfect, Some(false));
}

#[test]
fn a_turned_off_merged_list_is_disabled_not_unavailable() {
    use dashboard_core::status::RunOutcome::{Running, Success};
    let doc = StatusDocument::parse(REAL_V022_MERGED0).unwrap();
    assert_eq!(
        rows_of(&doc),
        owned(&[
            ("Main", "synced with origin/main · Clean working tree", true),
            ("Branches", "2 local (0 dangling) · 1 remote", false),
            ("Worktrees", "2 total · 1 cleanup candidate", false),
            ("Stashes", "0 stashes", true),
        ])
    );
    let merged = doc.section("pullRequests.merged").unwrap();
    assert!(merged.is_disabled() && !merged.is_unavailable());
    assert_eq!(merged.fetched_at, None);
    assert_eq!(doc.pull_requests.merged_limit, 0);
    assert!(doc.unavailable_sections().next().is_none());
    assert_eq!(doc.issues.len(), 2);
    let outcomes: Vec<_> = doc.workflow_runs.iter().map(|run| run.outcome()).collect();
    assert_eq!(outcomes, [Running, Running, Success, Success, Success]);
}

#[test]
fn every_documented_run_marker_has_its_meaning() {
    use dashboard_core::status::{RunOutcome, WorkflowRun};
    let outcome = |marker: &str| {
        WorkflowRun {
            status: marker.into(),
            ..WorkflowRun::default()
        }
        .outcome()
    };
    assert_eq!(outcome("✓"), RunOutcome::Success);
    assert_eq!(outcome("X"), RunOutcome::Failure);
    assert_eq!(outcome("*"), RunOutcome::Running);
    assert_eq!(outcome("○"), RunOutcome::Queued);
    assert_eq!(outcome("!"), RunOutcome::Attention);
    assert_eq!(outcome("—"), RunOutcome::Skipped);
    assert_eq!(outcome("·"), RunOutcome::Unknown);
    assert_eq!(outcome("?"), RunOutcome::Unknown);
}
