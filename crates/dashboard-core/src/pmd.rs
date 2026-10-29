//! Bounded read-only pmd adapter. No shell, git command, GitHub client or gh-x fallback.
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::priority::{Pmd, TIMEOUT};
use crate::source::{FetchError, StatusSource, TransportError};
use crate::status::{ParseError, StatusDocument};
use serde::{Deserialize, Serialize};

const MAX_RESPONSE: u64 = 4 * 1024 * 1024;
const MAX_PAGES: usize = 20;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosted {
    #[serde(default)]
    pub open_counts: OpenCounts,
    #[serde(default)]
    pub merge_counts: MergeCounts,
    pub service_url: String,
    /// When pmd last regenerated this status, which it does only when the repository changed.
    pub generated_at: String,
    /// When pmd last confirmed it: its last successful sweep, or `generated_at` if that's later. `None` when pmd
    /// hasn't swept since it started, or `/healthz` didn't answer; older caches don't have it either.
    #[serde(default)]
    pub checked_at: Option<String>,
    pub last_error: Option<LastError>,
    pub board: Option<WorkBoard>,
    pub work: Vec<WorkItem>,
    #[serde(default)]
    pub backlog: crate::backlog::Context,
    pub work_error: Option<String>,
    /// Preserve the transport classification through partial reads and cache serialization.
    #[serde(default)]
    pub work_transport_error: Option<TransportError>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastError {
    pub message: String,
    pub at: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBoard {
    pub generated_at: String,
    pub columns: Vec<WorkColumn>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkColumn {
    pub status: String,
    pub title: String,
    pub items: Vec<WorkItem>,
    pub truncated: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkItem {
    #[serde(flatten)]
    pub detail: crate::backlog::Detail,
    pub id: String,
    pub project: String,
    pub kind: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub status: String,
}
impl Hosted {
    /// When pmd last regenerated this status: when the repository last changed.
    pub fn changed(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        crate::activity::parse_time(&self.generated_at)
    }

    /// When pmd last confirmed this status, if known.
    pub fn checked(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.checked_at.as_deref().and_then(crate::activity::parse_time)
    }

    /// The open work items with `status`, from the complete work list. If that is empty, the Board's column, which
    /// may be truncated.
    pub fn items(&self, status: &str) -> Vec<&WorkItem> {
        if !self.work.is_empty() {
            return self.work.iter().filter(|item| item.status == status).collect();
        }
        self.board
            .iter()
            .flat_map(|board| &board.columns)
            .filter(|column| column.status == status)
            .flat_map(|column| &column.items)
            .collect()
    }

    /// The work item tracking issue or PR `number` (`kind` is `issue` or `pr`), if pmd has one open.
    pub fn item_for(&self, kind: &str, number: u64) -> Option<&WorkItem> {
        crate::work::STATUSES
            .iter()
            .flat_map(|status| self.items(status))
            .find(|item| item.kind == kind && item.number == number)
    }
}
/// The part of `/healthz` the dashboard reads: seconds since pmd's last successful sweep of every tracked repository.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    sweep_age_seconds: Option<i64>,
}
#[derive(Deserialize)]
pub(crate) struct Projects {
    pub projects: Vec<Project>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeCounts {
    pub merged_24h: Option<u64>,
    pub merged_7d: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCounts {
    pub open_prs: Option<u64>,
    pub open_issues: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct Project {
    #[serde(default)]
    pub counts: OpenCounts,
    #[serde(flatten)]
    pub merge_counts: MergeCounts,
    pub slug: String,
    pub repo: String,
    pub tracked: bool,
    pub priority: Option<u8>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    project: String,
    generated_at: String,
    status: serde_json::Value,
    last_error: Option<LastError>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkPage {
    items: Vec<WorkItem>,
    next_cursor: serde_json::Value,
}

/// Git comments end a physical line only outside double quotes. Keep quoting for section parsing
/// and value decoding; an escaped quote inside a value must not start or end a comment.
fn config_content(line: &str) -> Result<&str, FetchError> {
    let mut quoted = false;
    let mut escaped = false;
    for (offset, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => return Ok(line[..offset].trim()),
            _ => {}
        }
    }
    if quoted || escaped {
        return Err(FetchError::Identity);
    }
    Ok(line.trim())
}

fn config_value(value: &str) -> Result<String, FetchError> {
    let mut decoded = String::new();
    let mut characters = value.trim().chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => {}
            '\\' => decoded.push(match characters.next() {
                Some('"') => '"',
                Some('\\') => '\\',
                Some('n') => '\n',
                Some('t') => '\t',
                Some('b') => '\u{0008}',
                _ => return Err(FetchError::Identity),
            }),
            _ => decoded.push(character),
        }
    }
    Ok(decoded)
}

/// Read identity only, without invoking git. Linked worktrees use their common config.
/// Unsupported/conditional Git config is explicit, never guessed from the folder name.
pub fn checkout_identity(checkout: &Path) -> Result<String, FetchError> {
    let marker = checkout.join(".git");
    let mut dir = if marker.is_dir() {
        marker
    } else {
        let pointer = std::fs::read_to_string(marker).map_err(|_| FetchError::Identity)?;
        checkout.join(pointer.trim().strip_prefix("gitdir: ").ok_or(FetchError::Identity)?)
    };
    if let Ok(worktree_config) = std::fs::read_to_string(dir.join("config.worktree"))
        && worktree_config.lines().any(|line| {
            let line = line.trim().to_ascii_lowercase();
            line.starts_with("[remote") || line.starts_with("[include") || line.starts_with("[url ")
        })
    {
        return Err(FetchError::Identity);
    }
    if let Ok(common) = std::fs::read_to_string(dir.join("commondir")) {
        dir = dir.join(common.trim());
    }
    let config = std::fs::read_to_string(dir.join("config")).map_err(|_| FetchError::Identity)?;
    let mut origin = false;
    let mut urls = Vec::new();
    for line in config.lines() {
        let line = config_content(line)?;
        if line.starts_with('[') {
            // Conditional/includes and URL rewrites need Git's evaluator; report unsupported identity instead.
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("[include") || lower.starts_with("[url ") {
                return Err(FetchError::Identity);
            }
            origin = line.eq_ignore_ascii_case("[remote \"origin\"]");
        } else if origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("url")
        {
            urls.push(config_value(value)?);
        }
    }
    if urls.len() != 1 {
        return Err(FetchError::Identity);
    }
    let raw = &urls[0];
    let path = if let Some((host, path)) = raw.strip_prefix("git@").and_then(|url| url.split_once(':')) {
        if host != "github.com" && !host.starts_with("github-") {
            return Err(FetchError::Identity);
        }
        path.to_owned()
    } else {
        let url = reqwest::Url::parse(raw).map_err(|_| FetchError::Identity)?;
        if url.host_str() != Some("github.com") || !matches!(url.scheme(), "https" | "ssh") {
            return Err(FetchError::Identity);
        }
        url.path().trim_start_matches('/').to_owned()
    };
    let name = path.trim_end_matches('/').trim_end_matches(".git");
    let pieces: Vec<_> = name.split('/').collect();
    if pieces.len() != 2
        || pieces
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
    {
        return Err(FetchError::Identity);
    }
    Ok(name.to_ascii_lowercase())
}

pub(crate) struct ReadApi {
    client: reqwest::blocking::Client,
    base: reqwest::Url,
    token: String,
    deadline: Instant,
    timeout: Duration,
}
impl ReadApi {
    pub(crate) fn new(pmd: &Pmd, timeout: Duration) -> Result<Self, FetchError> {
        let mut token = String::new();
        std::fs::File::open(&pmd.token_path)
            .map_err(|_| FetchError::MissingToken)?
            .take(8193)
            .read_to_string(&mut token)
            .map_err(|_| FetchError::MissingToken)?;
        let token = token.trim().to_owned();
        if token.is_empty() || token.len() > 8192 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(FetchError::MissingToken);
        }
        let base =
            reqwest::Url::parse(&format!("{}/", pmd.url.trim_end_matches('/'))).map_err(|_| FetchError::Unavailable)?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(FetchError::Unavailable);
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| FetchError::Unavailable)?;
        Ok(Self {
            client,
            base,
            token,
            deadline: Instant::now() + TOTAL_TIMEOUT,
            timeout,
        })
    }
    pub(crate) fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, FetchError> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::Timeout);
        }
        let url = self.base.join(path).map_err(|_| FetchError::Malformed)?;
        let response = self
            .client
            .get(url)
            .query(query)
            .bearer_auth(&self.token)
            .timeout(self.timeout.min(remaining))
            .send()
            .map_err(|err| {
                if err.is_timeout() {
                    FetchError::Timeout
                } else if err.is_connect() {
                    FetchError::Connect
                } else {
                    FetchError::Unavailable
                }
            })?;
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(FetchError::Unauthorized);
        }
        if status.is_client_error() || status.is_server_error() {
            // 404 may carry the API no_status code; other HTTP errors need no body.
            if status.as_u16() != 404 {
                return Err(FetchError::Http(status.as_u16()));
            }
        }
        let mut bytes = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|err| {
            if !status.is_success() {
                return FetchError::Http(status.as_u16());
            }
            if matches!(
                err.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) || err
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
                .is_some_and(reqwest::Error::is_timeout)
            {
                FetchError::Timeout
            } else {
                FetchError::Unavailable
            }
        })?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err(FetchError::Malformed);
        }
        if !status.is_success() {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            return Err(if value["error"]["code"] == "no_status" {
                FetchError::NoStatus
            } else {
                FetchError::Http(status.as_u16())
            });
        }
        // Never include raw bodies, URLs, reqwest diagnostics or serde input in errors.
        serde_json::from_slice(&bytes).map_err(|_| FetchError::Malformed)
    }
    pub(crate) fn scrub(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => *s = s.replace(&self.token, "[redacted]"),
            serde_json::Value::Array(xs) => xs.iter_mut().for_each(|x| self.scrub(x)),
            serde_json::Value::Object(xs) => xs.values_mut().for_each(|x| self.scrub(x)),
            _ => {}
        }
    }
    fn work(&self, slug: &str) -> Result<(WorkBoard, Vec<WorkItem>), FetchError> {
        let board: WorkBoard = self.get("v1/board", &[("project", slug)])?;
        if chrono::DateTime::parse_from_rfc3339(&board.generated_at).is_err() {
            return Err(FetchError::Malformed);
        }
        for column in &board.columns {
            let mut ids = HashSet::new();
            if column
                .items
                .iter()
                .any(|item| item.project != slug || item.id.is_empty() || !ids.insert(&item.id))
            {
                return Err(FetchError::Malformed);
            }
        }
        if board.columns.len() != 4
            || board
                .columns
                .iter()
                .zip(["ready", "in_progress", "waiting_on_franz", "blocked"])
                .any(|(column, status)| column.status != status)
        {
            return Err(FetchError::Malformed);
        }
        let mut items = Vec::new();
        let mut cursor = String::new();
        let mut seen = HashSet::new();
        let mut ids = HashSet::new();
        for _ in 0..MAX_PAGES {
            let mut query = vec![("project", slug), ("limit", "500")];
            if !cursor.is_empty() {
                query.push(("cursor", &cursor));
            }
            let page: WorkPage = self.get("v1/work", &query)?;
            for item in page.items {
                if item.project != slug || item.id.is_empty() || !ids.insert(item.id.clone()) {
                    return Err(FetchError::Malformed);
                }
                items.push(item);
            }
            match page.next_cursor {
                serde_json::Value::Null => return Ok((board, items)),
                serde_json::Value::String(next) if !next.is_empty() && seen.insert(next.clone()) => cursor = next,
                _ => return Err(FetchError::Malformed),
            }
        }
        Err(FetchError::Failed(
            "pmd work pagination exceeded 20 pages; list incomplete.".into(),
        ))
    }
}
impl Pmd {
    /// Uses the same adapter as production, with a shorter deadline for controlled timeout tests.
    pub fn fetch_with_timeout(&self, checkout: &Path, timeout: Duration) -> Result<StatusDocument, FetchError> {
        let identity = checkout_identity(checkout)?;
        let api = ReadApi::new(self, timeout)?;
        let projects: Projects = api.get("v1/projects", &[])?;
        let matches: Vec<_> = projects
            .projects
            .iter()
            .filter(|p| p.tracked && p.repo.eq_ignore_ascii_case(&identity))
            .collect();
        let project = match matches.as_slice() {
            [] => return Err(FetchError::Untracked),
            [project] => *project,
            _ => return Err(FetchError::Ambiguous),
        };
        if project.slug.is_empty()
            || !project
                .slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(FetchError::Malformed);
        }
        let mut value: serde_json::Value = api.get(&format!("v1/projects/{}/status", project.slug), &[])?;
        api.scrub(&mut value);
        if value.get("lastError").is_none() {
            return Err(FetchError::Malformed);
        }
        let envelope: Envelope = serde_json::from_value(value).map_err(|_| FetchError::Malformed)?;
        if envelope.project != project.slug {
            return Err(FetchError::IdentityMismatch);
        }
        if chrono::DateTime::parse_from_rfc3339(&envelope.generated_at).is_err() {
            return Err(FetchError::Malformed);
        }
        for field in [
            "repository",
            "issues",
            "pullRequests",
            "workflowRuns",
            "sections",
            "workflowRunsPerfect",
        ] {
            if envelope.status.get(field).is_none_or(|v| v.is_null()) {
                return Err(FetchError::Malformed);
            }
        }
        let mut doc = StatusDocument::parse(&envelope.status.to_string()).map_err(|err| match err {
            ParseError::UnsupportedSchema(_) => FetchError::UnsupportedSchema,
            _ => FetchError::Malformed,
        })?;
        if !doc.repository.name_with_owner.eq_ignore_ascii_case(&identity) {
            return Err(FetchError::IdentityMismatch);
        }
        // Local fields from another machine are never meaningful for this checkout.
        doc.health = None;
        let checked_at = checked_at(&api, &envelope.generated_at);
        let (board, work, work_error, work_transport_error) = match api.work(&project.slug) {
            Ok((board, work)) => (Some(board), work, None, None),
            Err(error) => (None, Vec::new(), Some(error.to_string()), error.transport()),
        };
        let backlog = if work_error.is_none() {
            crate::backlog::read(&api, &project.slug).unwrap_or_else(|error| crate::backlog::Context {
                error: Some(error.to_string()),
                transport_error: error.transport(),
                ..Default::default()
            })
        } else {
            Default::default()
        };
        doc.hosted = Some(Hosted {
            open_counts: project.counts.clone(),
            merge_counts: project.merge_counts.clone(),
            backlog,
            service_url: self.url.clone(),
            generated_at: envelope.generated_at,
            checked_at,
            last_error: envelope.last_error,
            board,
            work,
            work_error,
            work_transport_error,
        });
        // Scrub echoed credentials in every persisted/rendered API field, including Board data.
        let mut value = serde_json::to_value(doc).map_err(|_| FetchError::Malformed)?;
        api.scrub(&mut value);
        let doc = serde_json::from_value(value).map_err(|_| FetchError::Malformed)?;
        if checkout_identity(checkout)? != identity {
            return Err(FetchError::IdentityMismatch);
        }
        Ok(doc)
    }
}
/// When pmd last confirmed a status generated at `generated_at`: the later of its last successful sweep and the status
/// itself. `None` if `/healthz` fails or pmd hasn't swept since it started; the status still loads.
fn checked_at(api: &ReadApi, generated_at: &str) -> Option<String> {
    let health: Health = api.get("healthz", &[]).ok()?;
    let swept = chrono::Utc::now() - chrono::Duration::seconds(health.sweep_age_seconds.filter(|age| *age >= 0)?);
    let generated = chrono::DateTime::parse_from_rfc3339(generated_at).ok()?.to_utc();
    Some(swept.max(generated).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
impl StatusSource for Pmd {
    fn fetch(&self, checkout: &Path, _: bool) -> Result<StatusDocument, FetchError> {
        self.fetch_with_timeout(checkout, TIMEOUT)
    }
}

#[cfg(test)]
mod config_tests {
    use super::{config_content, config_value};
    use crate::source::FetchError;

    #[test]
    fn quoted_hash_semicolon_and_escaped_quotes_survive_comment_removal() {
        for line in [
            r#"url = "https://github.com/owner/repo.git#hash;semicolon" # outside "#,
            r#"url = "https://github.com/owner/repo.git#hash;semicolon" ; outside "#,
        ] {
            let content = config_content(line).unwrap();
            assert_eq!(
                config_value(content.split_once('=').unwrap().1).unwrap(),
                "https://github.com/owner/repo.git#hash;semicolon"
            );
        }
        let content = config_content(r##"note = "literal \"#;\" and \\ path" # outside"##).unwrap();
        assert_eq!(
            config_value(content.split_once('=').unwrap().1).unwrap(),
            "literal \"#;\" and \\ path"
        );
    }

    #[test]
    fn malformed_quotes_and_escapes_fail_without_guessing_identity() {
        assert_eq!(config_content(r#"url = "unterminated #;"#), Err(FetchError::Identity));
        assert_eq!(config_value(r#""bad\qescape""#), Err(FetchError::Identity));
    }
}

#[cfg(test)]
mod merge_count_tests {
    use super::*;
    #[test]
    fn project_merge_counts_accept_current_and_older_pmd() {
        let current: Project = serde_json::from_str(
            r#"{"slug":"test","repo":"owner/test","tracked":true,"priority":1,"merged24h":0,"merged7d":42,"counts":{"openPrs":6,"openIssues":9}}"#,
        )
        .unwrap();
        assert_eq!(current.counts.open_prs, Some(6));
        assert_eq!(current.counts.open_issues, Some(9));
        assert_eq!(current.merge_counts.merged_24h, Some(0));
        assert_eq!(current.merge_counts.merged_7d, Some(42));
        let older: Project =
            serde_json::from_str(r#"{"slug":"test","repo":"owner/test","tracked":true,"priority":1}"#).unwrap();
        assert_eq!(older.counts, OpenCounts::default());
        assert_eq!(older.merge_counts, MergeCounts::default());
    }
}
