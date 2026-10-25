//! Bounded read-only pmd adapter. No shell, git command, GitHub client or gh-x fallback.
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::priority::{Pmd, TIMEOUT};
use crate::source::{FetchError, StatusSource};
use crate::status::{ParseError, StatusDocument};
use serde::{Deserialize, Serialize};

const MAX_RESPONSE: u64 = 4 * 1024 * 1024;
const MAX_PAGES: usize = 20;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosted {
    pub service_url: String,
    pub generated_at: String,
    pub last_error: Option<LastError>,
    pub board: Option<WorkBoard>,
    pub work: Vec<WorkItem>,
    pub work_error: Option<String>,
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
    pub id: String,
    pub project: String,
    pub kind: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub status: String,
}
#[derive(Deserialize)]
pub(crate) struct Projects {
    pub projects: Vec<Project>,
}
#[derive(Deserialize)]
pub(crate) struct Project {
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
    for line in config.lines().map(str::trim) {
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
            urls.push(value.trim().trim_matches('"').to_owned());
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
                } else {
                    FetchError::Unavailable
                }
            })?;
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(FetchError::Unauthorized);
        }
        let mut bytes = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|err| {
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
    fn scrub(&self, value: &mut serde_json::Value) {
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
        let (board, work, work_error) = match api.work(&project.slug) {
            Ok((board, work)) => (Some(board), work, None),
            Err(error) => (None, Vec::new(), Some(error.to_string())),
        };
        doc.hosted = Some(Hosted {
            service_url: self.url.clone(),
            generated_at: envelope.generated_at,
            last_error: envelope.last_error,
            board,
            work,
            work_error,
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
impl StatusSource for Pmd {
    fn fetch(&self, checkout: &Path, _: bool) -> Result<StatusDocument, FetchError> {
        self.fetch_with_timeout(checkout, TIMEOUT)
    }
}
