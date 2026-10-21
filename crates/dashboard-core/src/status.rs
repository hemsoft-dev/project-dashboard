//! The `gh x status --json` document ([gh-x#230](https://github.com/hemsoft-dev/gh-x/issues/230)), schema version 1.
//!
//! gh-x decides every verdict; this module only reads them. Every field defaults when it is missing, so additive
//! changes within schema version 1 keep parsing.

use serde::{Deserialize, Serialize};

/// The schema version this app understands.
pub const SCHEMA_VERSION: u32 = 1;

/// One repository's status, as `gh x status --json` prints it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusDocument {
    pub schema_version: u32,
    pub generated_at: String,
    pub repository: RepositoryIdentity,
    pub health: Health,
    pub issues: Vec<Issue>,
    pub pull_requests: PullRequests,
    pub workflow_runs: Vec<WorkflowRun>,
    /// Whether all five recent runs completed successfully.
    pub workflow_runs_perfect: Option<bool>,
    pub sections: Vec<Section>,
    /// An overall verdict, if gh-x reports one; the dashboard then shows it as is.
    pub verdict: Option<Verdict>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepositoryIdentity {
    pub name_with_owner: String,
    pub root_path: String,
    pub default_branch: String,
}

/// A health row's verdict, exactly as gh-x reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Verdict {
    /// `available` or `unavailable`.
    pub status: String,
    pub reason: String,
    pub clean: bool,
}

impl Verdict {
    pub fn is_unavailable(&self) -> bool {
        self.status == UNAVAILABLE
    }
}

pub const UNAVAILABLE: &str = "unavailable";
/// A section turned off by the invocation, such as the merged list with `--merged 0`.
pub const DISABLED: &str = "disabled";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Changes {
    pub clean: bool,
    pub staged: u32,
    pub modified: u32,
    pub deleted: u32,
    pub renamed: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BranchState {
    #[serde(flatten)]
    pub verdict: Verdict,
    pub name: String,
    pub checked_out: bool,
    pub upstream: String,
    pub synced: bool,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Changes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Branches {
    #[serde(flatten)]
    pub verdict: Verdict,
    pub local: u32,
    pub remote: u32,
    pub dangling: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Worktrees {
    #[serde(flatten)]
    pub verdict: Verdict,
    pub total: u32,
    pub cleanup_candidates: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stashes {
    #[serde(flatten)]
    pub verdict: Verdict,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Health {
    pub default_branch: BranchState,
    pub current_branch: BranchState,
    pub branches: Branches,
    pub worktrees: Worktrees,
    pub stashes: Stashes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Issue {
    pub number: u64,
    pub pull_requests: String,
    pub parent: String,
    pub sub_issues: String,
    pub title: String,
    pub author: String,
    pub state: String,
    pub labels: String,
    pub assignees: String,
    pub updated: String,
    /// RFC 3339; `null` when unknown.
    pub updated_at: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PullRequest {
    pub number: u64,
    pub issues: String,
    pub title: String,
    pub author: String,
    pub state: String,
    pub review: String,
    pub approvals: u32,
    pub checks: String,
    pub comments: String,
    pub ai_review: String,
    pub ai_clean: Option<bool>,
    pub branch: String,
    pub updated: String,
    pub updated_at: Option<String>,
    pub merged_at: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PullRequests {
    pub open: Vec<PullRequest>,
    pub merged: Vec<PullRequest>,
    pub merged_limit: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkflowRun {
    pub id: String,
    pub url: String,
    pub status: String,
    pub raw_status: String,
    pub title: String,
    pub workflow: String,
    pub branch: String,
    pub event: String,
    pub elapsed: String,
    pub age: String,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One GitHub section's load result: `available` or `unavailable` with a reason, and whether it came from gh-x's cache.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Section {
    pub name: String,
    pub status: String,
    pub reason: String,
    pub cached: bool,
    pub fetched_at: Option<String>,
    pub cache_age_seconds: f64,
}

impl Section {
    pub fn is_unavailable(&self) -> bool {
        self.status == UNAVAILABLE
    }

    pub fn is_disabled(&self) -> bool {
        self.status == DISABLED
    }
}

/// A workflow run's state, read from gh-x's terminal marker (`status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Success,
    Failure,
    Running,
    Queued,
    Attention,
    Skipped,
    Unknown,
}

impl WorkflowRun {
    /// The marker's meaning, per gh-x's status JSON reference: `✓` success, `X` failure, `*` running, `○` queued,
    /// `!` attention, `—` skipped or neutral, `·` unknown. Anything else is unknown too, and still shown as is.
    pub fn outcome(&self) -> RunOutcome {
        match self.status.as_str() {
            "✓" => RunOutcome::Success,
            "X" => RunOutcome::Failure,
            "*" => RunOutcome::Running,
            "○" => RunOutcome::Queued,
            "!" => RunOutcome::Attention,
            "—" => RunOutcome::Skipped,
            _ => RunOutcome::Unknown,
        }
    }
}

/// What `gh x status --json` prints when it can't report at all, such as outside a git repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ErrorDocument {
    error: Option<ErrorBody>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct ErrorBody {
    code: String,
    message: String,
}

/// Why a status document couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// gh-x reported an error object instead of a status.
    Reported { code: String, message: String },
    /// A schema version this app doesn't know.
    UnsupportedSchema(u32),
    /// Not JSON, or not shaped like a status document.
    Malformed(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reported { message, .. } => f.write_str(message),
            Self::UnsupportedSchema(version) => write!(
                f,
                "gh x status reported schema version {version}; this app reads version {SCHEMA_VERSION}. Update the app."
            ),
            Self::Malformed(detail) => write!(f, "gh x status printed an unreadable document: {detail}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl StatusDocument {
    /// Reads one `gh x status --json` document, or the error object gh-x prints in its place.
    pub fn parse(json: &str) -> Result<Self, ParseError> {
        let value: serde_json::Value =
            serde_json::from_str(json.trim()).map_err(|err| ParseError::Malformed(err.to_string()))?;
        if !value.is_object() {
            return Err(ParseError::Malformed("expected one JSON object".to_owned()));
        }
        if value.get("error").is_some_and(|error| !error.is_null()) {
            let error = serde_json::from_value::<ErrorDocument>(value)
                .ok()
                .and_then(|doc| doc.error)
                .unwrap_or_default();
            return Err(ParseError::Reported {
                code: error.code,
                message: error.message,
            });
        }
        let document: Self = serde_json::from_value(value).map_err(|err| ParseError::Malformed(err.to_string()))?;
        if document.schema_version != SCHEMA_VERSION {
            return Err(ParseError::UnsupportedSchema(document.schema_version));
        }
        Ok(document)
    }

    /// The repository's display name: `owner/name`, else the checkout's folder name.
    pub fn display_name(&self) -> &str {
        if !self.repository.name_with_owner.is_empty() {
            return &self.repository.name_with_owner;
        }
        let root = self.repository.root_path.trim_end_matches(['/', '\\']);
        root.rsplit(['/', '\\']).next().unwrap_or(root)
    }

    /// The health header, row by row, as the terminal shows it.
    pub fn health_rows(&self) -> Vec<HealthRow> {
        let health = &self.health;
        let default = &health.default_branch;
        let label = capitalized(if default.name.is_empty() {
            "default"
        } else {
            &default.name
        });
        let mut rows = vec![
            HealthRow {
                label,
                text: if default.verdict.is_unavailable() {
                    default.verdict.reason.clone()
                } else {
                    branch_text(default)
                },
                clean: default.verdict.clean,
                unavailable: default.verdict.is_unavailable(),
            },
            HealthRow::of(
                "Branches",
                format!(
                    "{} local ({} dangling) · {} remote",
                    health.branches.local, health.branches.dangling, health.branches.remote
                ),
                &health.branches.verdict,
            ),
            HealthRow::of(
                "Worktrees",
                format!(
                    "{} total · {} cleanup {}",
                    health.worktrees.total,
                    health.worktrees.cleanup_candidates,
                    if health.worktrees.cleanup_candidates == 1 {
                        "candidate"
                    } else {
                        "candidates"
                    }
                ),
                &health.worktrees.verdict,
            ),
            HealthRow::of(
                "Stashes",
                plural(health.stashes.count, "stash", "stashes"),
                &health.stashes.verdict,
            ),
        ];
        let current = &health.current_branch;
        // As the terminal does: a current branch other than the default (a detached HEAD has no name), or any current
        // branch when the default's state is unavailable. An absent `currentBranch` object has no status.
        let reported = !current.verdict.status.is_empty();
        if reported && (current.name != default.name || default.verdict.is_unavailable()) {
            let name = if current.name.is_empty() {
                "detached HEAD"
            } else {
                &current.name
            };
            rows.push(HealthRow::of(
                "Current",
                format!("{name} · {}", branch_text(current)),
                &current.verdict,
            ));
        }
        rows
    }

    /// GitHub sections that failed to load, with gh-x's reasons.
    pub fn unavailable_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|section| section.is_unavailable())
    }

    /// gh-x's overall verdict when the document carries one (a top-level `verdict`); otherwise clean when every
    /// health row is clean by gh-x's own verdict and every GitHub section loaded. The terminal view has no overall
    /// verdict of its own, so the fallback only combines gh-x's row and section verdicts and never re-derives them.
    pub fn is_clean(&self) -> bool {
        if let Some(verdict) = &self.verdict {
            return verdict.clean;
        }
        self.health_rows().iter().all(|row| row.clean) && self.unavailable_sections().next().is_none()
    }

    /// The section named `name`, if gh-x reported it.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.name == name)
    }

    /// The section named `name` when gh-x reports it unavailable.
    pub fn unavailable_section(&self, name: &str) -> Option<&Section> {
        self.unavailable_sections().find(|section| section.name == name)
    }
}

/// One row of the health header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRow {
    pub label: String,
    pub text: String,
    /// gh-x's verdict for the row.
    pub clean: bool,
    pub unavailable: bool,
}

impl HealthRow {
    fn of(label: &str, text: String, verdict: &Verdict) -> Self {
        let unavailable = verdict.is_unavailable();
        Self {
            label: label.to_owned(),
            text: if unavailable {
                format!("Unavailable: {}", verdict.reason)
            } else {
                text
            },
            clean: verdict.clean,
            unavailable,
        }
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn plural(count: u32, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// The terminal's branch cell: upstream state, then the working tree (or "not checked out").
fn branch_text(branch: &BranchState) -> String {
    let upstream = &branch.upstream;
    let sync = match (upstream.is_empty(), branch.ahead, branch.behind) {
        (true, _, _) => "no upstream configured".to_owned(),
        (false, 0, 0) => format!("synced with {upstream}"),
        (false, ahead, 0) => format!("ahead of {upstream} by {}", plural(ahead, "commit", "commits")),
        (false, 0, behind) => format!("behind {upstream} by {}", plural(behind, "commit", "commits")),
        (false, ahead, behind) => format!("diverged from {upstream}: {ahead} ahead, {behind} behind"),
    };
    let tree = if branch.checked_out {
        changes_text(&branch.changes)
    } else {
        "not checked out".to_owned()
    };
    format!("{sync} · {tree}")
}

fn changes_text(changes: &Changes) -> String {
    let parts: Vec<String> = [
        (changes.staged, "staged file", "staged files"),
        (changes.modified, "modified file", "modified files"),
        (changes.deleted, "deleted file", "deleted files"),
        (changes.renamed, "renamed file", "renamed files"),
        (changes.untracked, "untracked file", "untracked files"),
        (changes.conflicted, "conflicted file", "conflicted files"),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, singular, many)| plural(count, singular, many))
    .collect();
    if parts.is_empty() {
        "Clean working tree".to_owned()
    } else {
        parts.join(", ")
    }
}
