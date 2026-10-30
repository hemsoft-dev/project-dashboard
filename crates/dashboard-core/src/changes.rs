//! Project row change detection ignores transport freshness, Backlog-only enrichment and the passage of time.
use crate::status::StatusDocument;

pub fn row_data(document: &StatusDocument) -> StatusDocument {
    let mut data = document.clone();
    data.generated_at.clear();
    // Relative display strings and cache bookkeeping advance even when the underlying events do not.
    // Keep absolute event timestamps, section verdicts, and item content in the comparison.
    for issue in &mut data.issues {
        issue.updated.clear();
    }
    for pr in data.pull_requests.open.iter_mut().chain(&mut data.pull_requests.merged) {
        pr.updated.clear();
    }
    for run in &mut data.workflow_runs {
        run.age.clear();
        run.elapsed.clear();
    }
    for section in &mut data.sections {
        section.fetched_at = None;
        section.cache_age_seconds = 0.;
        section.cached = false;
        if section.is_unavailable() {
            section.reason.clear();
        }
    }
    if let Some(hosted) = &mut data.hosted {
        hosted.generated_at.clear();
        hosted.checked_at = None;
        hosted.readiness.sources.clear();
        hosted.readiness.sources_unavailable = false;
        hosted.readiness.dispatch_unavailable = false;
        if let Some(dispatch) = &mut hosted.readiness.dispatch {
            dispatch.stale = false;
        }
        hosted.last_error = None;
        hosted.work_error = None;
        hosted.work_transport_error = None;
        // Project rows and details do not render Backlog enrichment. In particular, the dispatch plan is global,
        // so comparing it would highlight every project when unrelated work is dispatched or enrichment fails.
        hosted.backlog = Default::default();
        if let Some(board) = &mut hosted.board {
            board.generated_at.clear();
        }
    }
    data
}

/// A first successful load establishes the baseline, including when cached data was present.
pub fn changed(previous: Option<&StatusDocument>, next: &StatusDocument) -> bool {
    previous
        .is_some_and(|previous| availability(previous) == availability(next) && row_data(previous) != row_data(next))
}

/// Keep source availability separate from project data. A transition establishes a new baseline, even if
/// data changed while a source was unavailable. Error diagnostics themselves never count as project changes.
fn availability(document: &StatusDocument) -> (bool, bool, bool, bool, Vec<(&str, bool)>) {
    let mut sections: Vec<_> = document
        .sections
        .iter()
        .map(|s| (s.name.as_str(), !s.is_unavailable()))
        .collect();
    sections.sort_unstable();
    let status = document.hosted.as_ref().is_none_or(|h| h.last_error.is_none());
    let work = document
        .hosted
        .as_ref()
        .is_none_or(|h| h.work_error.is_none() && h.work_transport_error.is_none());
    let dispatch = document
        .hosted
        .as_ref()
        .is_none_or(|h| !h.readiness.dispatch_unavailable);
    let sources = document.hosted.as_ref().is_none_or(|h| {
        !h.readiness.sources_unavailable
            && h.readiness.sources.values().all(|s| {
                s.state == "fresh"
                    && s.last_success_at
                        .as_deref()
                        .and_then(crate::activity::parse_time)
                        .is_some()
            })
    });
    (status, work, dispatch, sources, sections)
}

/// No-project/no-session results come from a healthy source and can represent real session lifecycle changes.
/// Source outages and recoveries are rebaselines, including changing diagnostics during a persistent outage.
pub fn session_changed(previous: Option<&crate::t3::Lookup>, next: &crate::t3::Lookup) -> bool {
    fn available(lookup: &crate::t3::Lookup) -> bool {
        matches!(
            lookup,
            Ok(_) | Err(crate::t3::NoThread::NoProject | crate::t3::NoThread::NoOpenThread)
        )
    }
    previous.is_some_and(|previous| available(previous) && available(next) && previous != next)
}
