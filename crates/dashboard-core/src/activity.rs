//! A repository's last activity: the most recently updated open or merged pull request, open issue or workflow run in
//! its `gh x status --json` document. Only gh-x's own timestamps are read.

use chrono::{DateTime, Local, TimeZone, Utc};

use crate::status::StatusDocument;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    PullRequest,
    Issue,
    WorkflowRun,
}

/// The newest item, with what the table shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub kind: ActivityKind,
    /// "PR #12", "Issue #8" or "CI run".
    pub label: String,
    pub title: String,
    pub url: String,
    pub at: DateTime<Utc>,
}

impl Activity {
    /// How long ago it was updated: "14m ago".
    pub fn age(&self, now: DateTime<Utc>) -> String {
        ago(self.at, now)
    }

    /// When it was updated, on this computer's clock: "2026-10-21 02:16".
    pub fn local_time(&self) -> String {
        time_in(self.at, &Local)
    }
}

/// `at` on `zone`'s clock, to the minute: "2026-10-21 02:16".
pub fn time_in<Tz: TimeZone>(at: DateTime<Utc>, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at.with_timezone(zone).format("%Y-%m-%d %H:%M").to_string()
}

fn parse(at: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at?).ok().map(|at| at.with_timezone(&Utc))
}

/// The newest item across open and merged PRs (`updatedAt`), issues (`updatedAt`) and runs (`startedAt`, else
/// `createdAt`). Items without a readable timestamp are skipped. On a tie the first in that order wins: open PRs,
/// merged PRs, issues, then runs, each in gh-x's order.
pub fn last_activity(document: &StatusDocument) -> Option<Activity> {
    let prs = document
        .pull_requests
        .open
        .iter()
        .chain(&document.pull_requests.merged)
        .filter_map(|pr| {
            Some(Activity {
                kind: ActivityKind::PullRequest,
                label: format!("PR #{}", pr.number),
                title: pr.title.clone(),
                url: pr.url.clone(),
                at: parse(pr.updated_at.as_deref())?,
            })
        });
    let issues = document.issues.iter().filter_map(|issue| {
        Some(Activity {
            kind: ActivityKind::Issue,
            label: format!("Issue #{}", issue.number),
            title: issue.title.clone(),
            url: issue.url.clone(),
            at: parse(issue.updated_at.as_deref())?,
        })
    });
    let runs = document.workflow_runs.iter().filter_map(|run| {
        let at = parse(run.started_at.as_deref()).or_else(|| parse(run.created_at.as_deref()))?;
        let workflow = if run.workflow.is_empty() {
            "Workflow"
        } else {
            &run.workflow
        };
        Some(Activity {
            kind: ActivityKind::WorkflowRun,
            label: format!("{workflow} run"),
            title: run.title.clone(),
            url: run.url.clone(),
            at,
        })
    });
    prs.chain(issues)
        .chain(runs)
        .fold(None, |newest: Option<Activity>, item| match newest {
            Some(newest) if newest.at >= item.at => Some(newest),
            _ => Some(item),
        })
}

/// "just now", "14m ago", "3h ago", "2d ago". A time in the future (clock skew) reads "just now".
pub fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds().max(0);
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
