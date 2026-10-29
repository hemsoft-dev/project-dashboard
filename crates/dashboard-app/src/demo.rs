//! Demo data from the checked-in `gh x status --json` fixtures, for `--demo`, the UI tests and screenshots. Nothing
//! here runs gh.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use dashboard_core::fixtures::{self, ATTENTION, CLEAN, UNAVAILABLE};
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use dashboard_core::t3::{Lookup, NoThread, ThreadFinder, ThreadLink};

/// The fixtures' own "now": the clean and attention documents were generated at 06:30 UTC.
pub fn fixture_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-21T06:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// The first four repositories: clean, needing attention, partly unavailable, and failing.
const CORE: [&str; 4] = ["gh-x", "codexbar", "minibot", "weather-cli"];

/// Which item a demo repository touched last.
#[derive(Clone, Copy)]
enum Lead {
    PullRequest,
    Issue,
    Run,
}

/// Eight more, made from the clean and attention fixtures: (name, based on attention, the newest item, minutes
/// before the fixtures' now, open items kept).
const MORE: [(&str, bool, Lead, i64, usize); 8] = [
    ("hs-buddy", true, Lead::PullRequest, 3, 2),
    ("chief", false, Lead::Run, 40, 1),
    ("tickdown", false, Lead::PullRequest, 2_900, 0),
    ("project-dashboard", true, Lead::Issue, 14, 1),
    ("set-it-free-loop", true, Lead::Run, 180, 2),
    ("hs-pi-dashboard", true, Lead::Issue, 95, 1),
    ("yahtzee", false, Lead::Run, 9_000, 1),
    ("summarist", true, Lead::PullRequest, 600, 1),
];

/// The demo checkouts: four with a story each, then eight more, so the table has twelve rows.
pub fn checkouts() -> Vec<PathBuf> {
    CORE.iter()
        .chain(MORE.iter().map(|(name, ..)| name))
        .map(|name| PathBuf::from("/demo").join(name))
        .collect()
}

/// Dates every item in `document` an hour or more before `at`, then makes the `lead` item (falling back to a run)
/// the one touched at `at`.
fn date(document: &mut StatusDocument, lead: Lead, at: DateTime<Utc>) {
    let older = Some((at - Duration::minutes(61)).to_rfc3339());
    for pr in document
        .pull_requests
        .open
        .iter_mut()
        .chain(&mut document.pull_requests.merged)
    {
        pr.updated_at.clone_from(&older);
    }
    for issue in &mut document.issues {
        issue.updated_at.clone_from(&older);
    }
    for run in &mut document.workflow_runs {
        run.created_at.clone_from(&older);
        run.started_at.clone_from(&older);
    }
    let newest = Some(at.to_rfc3339());
    match lead {
        Lead::PullRequest if !document.pull_requests.open.is_empty() => {
            document.pull_requests.open[0].updated_at = newest;
        }
        Lead::PullRequest if !document.pull_requests.merged.is_empty() => {
            document.pull_requests.merged[0].updated_at = newest;
        }
        Lead::Issue if !document.issues.is_empty() => document.issues[0].updated_at = newest,
        _ => {
            if let Some(run) = document.workflow_runs.first_mut() {
                run.started_at = newest;
            }
        }
    }
}

fn variant(name: &str, attention: bool, lead: Lead, minutes: i64, keep: usize) -> StatusDocument {
    let mut document = fixtures::document(if attention { ATTENTION } else { CLEAN });
    document.repository.name_with_owner = format!("hemsoft-dev/{name}");
    document.repository.root_path = format!("/demo/{name}");
    document.pull_requests.open.truncate(keep);
    document.issues.truncate(keep);
    date(&mut document, lead, fixture_now() - Duration::minutes(minutes));
    document
}

pub struct DemoSource;

impl StatusSource for DemoSource {
    fn fetch(&self, checkout: &Path, _refresh: bool) -> Result<StatusDocument, FetchError> {
        let name = checkout.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        match name {
            "gh-x" => Ok(fixtures::document(CLEAN)),
            "project-manager" => Ok(fixtures::document(fixtures::ENRICHMENT_ERRORS)),
            "codexbar" => Ok(fixtures::document(ATTENTION)),
            "minibot" => Ok(fixtures::document(UNAVAILABLE)),
            "weather-cli" => Err(FetchError::Failed(
                "HTTP 401: Bad credentials (https://api.github.com/graphql). Try gh auth login.".into(),
            )),
            _ => MORE
                .iter()
                .find(|(more, ..)| *more == name)
                .map(|&(name, attention, lead, minutes, keep)| variant(name, attention, lead, minutes, keep))
                .ok_or_else(|| FetchError::Failed(format!("no demo data for {name}"))),
        }
    }
}

/// Demo T3 Code threads: a running one, idle ones, and the reasons a row has none.
pub struct DemoThreads;

impl ThreadFinder for DemoThreads {
    fn find(&self, checkouts: &[PathBuf]) -> Vec<Lookup> {
        let link = |name: &str, title: &str, running| {
            Ok(ThreadLink {
                title: title.to_owned(),
                url: format!("http://127.0.0.1:3773/demo-environment/demo%3A{name}"),
                running,
            })
        };
        checkouts
            .iter()
            .map(
                |checkout| match checkout.file_name().and_then(|name| name.to_str()).unwrap_or_default() {
                    "gh-x" => link("gh-x", "gh-x: release v0.22.1", true),
                    "codexbar" => link("codexbar", "codexbar: L1 audit -> issues", false),
                    "hs-buddy" => link("hs-buddy", "hs-buddy: tray icon on Linux", false),
                    "project-dashboard" => link("project-dashboard", "project-dashboard: resizable columns", true),
                    "minibot" | "tickdown" => Err(NoThread::NoOpenThread),
                    _ => Err(NoThread::NoProject),
                },
            )
            .collect()
    }
}

/// Illustrative pmd values; demo mode never reads a token or calls the API.
pub struct DemoPriorities;

impl dashboard_core::priority::PrioritySource for DemoPriorities {
    fn fetch(&self) -> dashboard_core::priority::Priorities {
        dashboard_core::priority::Priorities::parse(
            br#"{"projects":[
            {"slug":"gh-x","repo":"hemsoft-dev/gh-x","priority":3,"tracked":true},
            {"slug":"codexbar","repo":"hemsoft-dev/codexbar","priority":3,"tracked":true},
            {"slug":"hs-buddy","repo":"hemsoft-dev/hs-buddy","priority":2,"tracked":true},
            {"slug":"project-dashboard","repo":"hemsoft-dev/project-dashboard","priority":3,"tracked":true},
            {"slug":"set-it-free-loop","repo":"hemsoft-dev/set-it-free-loop","priority":1,"tracked":true},
            {"slug":"hs-pi-dashboard","repo":"hemsoft-dev/hs-pi-dashboard","priority":3,"tracked":true}
        ]}"#,
        )
        .unwrap()
    }
}

/// Illustrative API state for headless regression tests and captures, with no network access: the demo's documents
/// as pmd serves them, with Board and work items. codexbar has one item of each status, hs-buddy waits on Franz, and
/// minibot's Board didn't load.
#[cfg(test)]
pub struct HostedSource;
#[cfg(test)]
impl StatusSource for HostedSource {
    fn fetch(&self, checkout: &Path, _: bool) -> Result<StatusDocument, FetchError> {
        use dashboard_core::pmd::{Hosted, WorkBoard, WorkColumn, WorkItem};
        let mut document = DemoSource.fetch(checkout, false)?;
        document.health = None;
        document.verdict = None;
        let name = checkout.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let repo = &document.repository.name_with_owner;
        let item = |id: &str, kind: &str, number: u64, title: &str, status: &str| WorkItem {
            detail: Default::default(),
            id: id.into(),
            project: name.into(),
            kind: kind.into(),
            number,
            title: title.into(),
            url: format!(
                "https://github.com/{repo}/{}/{number}",
                if kind == "pr" { "pull" } else { "issues" }
            ),
            status: status.into(),
        };
        let work = match name {
            "codexbar" => vec![
                item("fixture-blocked", "issue", 240, "Sign the macOS build", "blocked"),
                item(
                    "fixture-waiting",
                    "pr",
                    232,
                    "feat: show the tray icon on Linux",
                    "waiting_on_franz",
                ),
                item(
                    "fixture-in-progress",
                    "issue",
                    219,
                    "Remember the window placement per display",
                    "in_progress",
                ),
                item("fixture-stable-id", "issue", 31, "Read shared API data", "ready"),
                item(
                    "fixture-ready-listed",
                    "issue",
                    231,
                    "Show the tray icon on Linux",
                    "ready",
                ),
            ],
            "hs-buddy" => vec![item(
                "fixture-hs-buddy",
                "pr",
                232,
                "feat: show the tray icon on Linux",
                "waiting_on_franz",
            )],
            _ => Vec::new(),
        };
        let at = |minutes: i64| (fixture_now() - Duration::minutes(minutes)).to_rfc3339();
        let board = (name != "minibot").then(|| WorkBoard {
            generated_at: at(4),
            columns: dashboard_core::work::STATUSES
                .iter()
                .map(|status| WorkColumn {
                    status: (*status).into(),
                    title: dashboard_core::work::status_label(status).into(),
                    items: work.iter().filter(|item| item.status == *status).cloned().collect(),
                    truncated: false,
                })
                .collect(),
        });
        document.hosted = Some(Hosted {
            open_counts: Default::default(),
            merge_counts: dashboard_core::pmd::MergeCounts {
                merged_24h: Some(3),
                merged_7d: Some(17),
            },
            backlog: Default::default(),
            service_url: "http://fixture.invalid".into(),
            generated_at: at(323),
            checked_at: Some(at(12)),
            last_error: None,
            work_transport_error: None,
            work_error: board.is_none().then(|| "pmd returned HTTP 503.".to_owned()),
            board,
            work,
        });
        Ok(document)
    }
}

/// The demo's sample row for the Manage menu's sample dialog (#33): kept in memory, never sent anywhere. Its first save
/// meets a change made "elsewhere", so the conflict flow can be tried; a priority above 5 is refused with pmd's kind of
/// field error.
pub struct DemoSample {
    row: std::sync::Mutex<(u64, String, Option<u64>)>,
    changed_elsewhere: std::sync::atomic::AtomicBool,
}

impl Default for DemoSample {
    fn default() -> Self {
        Self {
            row: std::sync::Mutex::new((3, "Ship the Manage menu".to_owned(), Some(1))),
            changed_elsewhere: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl DemoSample {
    fn json(row: &(u64, String, Option<u64>)) -> serde_json::Value {
        serde_json::json!({"id": "demo", "version": row.0, "title": row.1, "priority": row.2})
    }
}

impl dashboard_core::write::WriteBackend for DemoSample {
    fn token_path(&self) -> Option<PathBuf> {
        None
    }
    fn whoami(&self) -> Result<dashboard_core::write::Whoami, dashboard_core::write::WriteError> {
        Err(dashboard_core::write::WriteError::WritesOff)
    }
    fn store_token(&self, _: &str) -> Result<(), dashboard_core::write::WriteError> {
        Err(dashboard_core::write::WriteError::WritesOff)
    }
    fn read(&self, _: &str) -> Result<serde_json::Value, dashboard_core::write::WriteError> {
        Ok(Self::json(&self.row.lock().unwrap()))
    }
    fn write(
        &self,
        request: &dashboard_core::write::WriteRequest,
    ) -> Result<serde_json::Value, dashboard_core::write::WriteError> {
        use dashboard_core::write::{Precondition, WriteError};
        let mut row = self.row.lock().unwrap();
        if request.precondition != Precondition::IfMatch(row.0) {
            return Err(WriteError::StaleVersion);
        }
        let priority = request.body["priority"].as_u64();
        if priority.is_some_and(|p| p > 5) {
            return Err(WriteError::BadRequest {
                code: "bad_request".into(),
                message: "priority must be 0-5, or null to inherit".into(),
            });
        }
        if !self.changed_elsewhere.swap(true, std::sync::atomic::Ordering::SeqCst) {
            row.0 += 1;
            row.1 = "Ship the Manage menu and dialogs".to_owned();
            return Err(WriteError::StaleVersion);
        }
        row.0 += 1;
        row.1 = request.body["title"].as_str().unwrap_or_default().to_owned();
        row.2 = priority;
        Ok(Self::json(&row))
    }
}
