//! The Projects dialog's data, without a window (#34): pmd's projects, their checks, the full-row `PUT`, and linking
//! a local checkout to a project.
//!
//! pmd's `projects` table is the source of truth for project priority (Franz, 2026-10-26, option C of
//! [project-manager#50](https://github.com/hemsoft-dev/project-manager/issues/50)): a `PUT` sets it and PORTFOLIO.md
//! is a view generated from pmd, so there is no override to reset. No tier is `priority: null`, and P5 (dormant) must
//! be untracked. A project is never deleted; untracking keeps its data. See pmd's
//! [Project writes](https://github.com/hemsoft-dev/project-manager#project-writes).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::{Config, ConfigError};
use crate::edit::Row;
use crate::fleet::words;
use crate::work::FieldProblem;
use crate::write::WriteError;

/// The editor's fields, as pmd names them, so a `400` naming one lands next to it.
pub const FIELDS: [&str; 7] = ["slug", "repo", "priority", "goal", "notes", "requires", "tracked"];
/// pmd's limits.
pub const MAX_TEXT: usize = 1000;
pub const MAX_REQUIRES: usize = 32;

pub fn label(field: &str) -> &'static str {
    match field {
        "slug" => "Slug",
        "repo" => "Repository",
        "priority" => "Priority",
        "goal" => "Goal",
        "notes" => "Notes",
        "requires" => "Requires",
        "tracked" => "Tracked",
        _ => "Field",
    }
}

/// One project as `GET /v1/projects?all=1` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub slug: String,
    /// `owner/name`.
    pub repo: String,
    /// P0 to P5, or `None` for no tier.
    pub priority: Option<u8>,
    pub goal: String,
    pub notes: String,
    pub requires: Vec<String>,
    pub tracked: bool,
    /// `portfolio` if PORTFOLIO.md's seed added it and no `PUT` changed it, else `api`.
    pub source: String,
    pub version: u64,
}

impl Project {
    pub fn from_json(value: &Value) -> Result<Self, WriteError> {
        let text = |field: &str| value.get(field).and_then(Value::as_str).map(str::to_owned);
        // A tier outside P0–P5 is unknown: it reads as none and shows as `—`, as the Board's do.
        let priority = match value.get("priority") {
            None | Some(Value::Null) => None,
            Some(number) => number
                .as_u64()
                .ok_or(WriteError::Malformed)?
                .try_into()
                .ok()
                .filter(|p: &u8| *p <= 5),
        };
        Ok(Self {
            slug: text("slug").ok_or(WriteError::Malformed)?,
            repo: text("repo").ok_or(WriteError::Malformed)?,
            priority,
            goal: text("goal").unwrap_or_default(),
            notes: text("notes").unwrap_or_default(),
            requires: value
                .get("requires")
                .and_then(Value::as_array)
                .map(|words| words.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
            tracked: value
                .get("tracked")
                .and_then(Value::as_bool)
                .ok_or(WriteError::Malformed)?,
            source: text("source").unwrap_or_default(),
            version: value
                .get("version")
                .and_then(Value::as_u64)
                .ok_or(WriteError::Malformed)?,
        })
    }

    /// The list from `GET /v1/projects?all=1`.
    pub fn list_from_json(value: &Value) -> Result<Vec<Self>, WriteError> {
        value
            .get("projects")
            .and_then(Value::as_array)
            .ok_or(WriteError::Malformed)?
            .iter()
            .map(Self::from_json)
            .collect()
    }

    pub fn path(&self) -> String {
        path(&self.slug)
    }

    /// `P0` to `P5`, or `—` for no tier.
    pub fn priority_label(&self) -> String {
        self.priority.map_or("—".to_owned(), |p| format!("P{p}"))
    }

    /// The editor's starting values.
    pub fn row(&self) -> Row {
        let values = [
            ("slug", self.slug.clone()),
            ("repo", self.repo.clone()),
            ("priority", self.priority.map(|p| p.to_string()).unwrap_or_default()),
            ("goal", self.goal.clone()),
            ("notes", self.notes.clone()),
            ("requires", self.requires.join(", ")),
            ("tracked", self.tracked.to_string()),
        ]
        .into_iter()
        .map(|(field, value)| (field.to_owned(), value))
        .collect();
        Row {
            version: self.version,
            values,
        }
    }

    /// Whether the text filter matches its slug, repo, goal or notes, ignoring case.
    pub fn matches(&self, filter: &str) -> bool {
        let filter = filter.trim().to_lowercase();
        filter.is_empty()
            || [&self.slug, &self.repo, &self.goal, &self.notes]
                .iter()
                .any(|text| text.to_lowercase().contains(&filter))
    }
}

/// The API path of a project.
pub fn path(slug: &str) -> String {
    format!("v1/projects/{slug}")
}

/// The list's order: priority (no tier last), then slug.
pub fn sort(projects: &mut [Project]) {
    projects.sort_by(|a, b| {
        (a.priority.unwrap_or(u8::MAX), a.slug.to_lowercase())
            .cmp(&(b.priority.unwrap_or(u8::MAX), b.slug.to_lowercase()))
    });
}

/// What a new project starts with: tracked, no tier.
pub fn defaults() -> BTreeMap<String, String> {
    [("tracked", "true")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// The slug a repo suggests: its name, lower case.
pub fn slug_for(repo: &str) -> String {
    repo.trim().rsplit('/').next().unwrap_or_default().to_lowercase()
}

fn is_repo_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 100
        && text != "."
        && text != ".."
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn is_owner(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 39
        && !text.starts_with('-')
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn is_tag(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && text.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The whole project as pmd's `PUT` body, with its slug, or the problems that stop the save. Goal and notes are
/// trimmed, since pmd refuses outer spaces.
pub fn body(values: &BTreeMap<String, String>) -> Result<(String, Value), Vec<FieldProblem>> {
    let get = |field: &str| values.get(field).map_or("", |v| v.trim());
    let mut problems: Vec<FieldProblem> = Vec::new();
    let slug = get("slug");
    let repo = get("repo");
    if !is_repo_name(slug) {
        problems.push((
            "slug",
            "A repository name: letters, digits, '.', '_' and '-', at most 100.".to_owned(),
        ));
    }
    match repo.split_once('/') {
        Some((owner, name)) if is_owner(owner) && is_repo_name(name) => {
            if is_repo_name(slug) && !name.eq_ignore_ascii_case(slug) {
                problems.push((
                    "slug",
                    format!("The slug is the repository's name: {}.", name.to_lowercase()),
                ));
            }
        }
        _ => problems.push(("repo", "owner/name, such as hemsoft-dev/gh-x.".to_owned())),
    }
    let priority = match get("priority") {
        "" => Value::Null,
        text => match text.parse::<u8>() {
            Ok(n) if n <= 5 => n.into(),
            _ => {
                problems.push(("priority", "P0 to P5, or no tier.".to_owned()));
                Value::Null
            }
        },
    };
    let tracked = get("tracked") != "false";
    if tracked && priority == json!(5) {
        problems.push((
            "priority",
            "P5 is dormant: untrack the project, or pick P0 to P4.".to_owned(),
        ));
    }
    for field in ["goal", "notes"] {
        let text = get(field);
        if text.chars().count() > MAX_TEXT {
            problems.push((field, format!("At most {MAX_TEXT} characters.")));
        } else if text.chars().any(char::is_control) {
            problems.push((field, "One line, without control characters.".to_owned()));
        }
    }
    let requires = words(get("requires"));
    if let Some(bad) = requires.iter().find(|word| !is_tag(word)) {
        problems.push((
            "requires",
            format!("{bad:?}: lower-case letters, digits and _, starting with a letter."),
        ));
    } else if requires.len() > MAX_REQUIRES {
        problems.push(("requires", format!("At most {MAX_REQUIRES}.")));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok((
        slug.to_owned(),
        json!({
            "repo": repo, "priority": priority, "goal": get("goal"), "notes": get("notes"),
            "requires": requires, "tracked": tracked
        }),
    ))
}

/// The field a project write error belongs to, beyond a `400` whose message names one.
pub fn error_field(error: &WriteError) -> Option<&'static str> {
    match error {
        WriteError::BadRequest { code, .. } if code == "unknown_repo" || code == "repo_not_installed" => Some("repo"),
        WriteError::AlreadyExists => Some("slug"),
        _ => None,
    }
}

/// Why a checkout couldn't be linked to a project.
#[derive(Debug)]
pub enum LinkError {
    /// The folder's origin is another repository, or none the dashboard can read.
    Mismatch { found: Option<String> },
    /// `config.toml` couldn't be read or written.
    Config(ConfigError),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mismatch { found: Some(found) } => {
                write!(f, "That folder's origin is {found}; nothing was written.")
            }
            Self::Mismatch { found: None } => f.write_str(
                "That folder has no GitHub origin the dashboard can read (one [remote \"origin\"] url); nothing was \
                 written.",
            ),
            Self::Config(error) => write!(f, "{error}"),
        }
    }
}

/// Adds `checkout` to `repositories` in the `config.toml` in `config_dir`, after checking its origin is `repo` with
/// the same identity rules the dashboard reads status with, and keeping the file's comments. A checkout already
/// listed is left as it is.
pub fn link_checkout(config_dir: &Path, checkout: &Path, repo: &str) -> Result<PathBuf, LinkError> {
    let found = crate::pmd::checkout_identity(checkout).ok();
    if !found.as_deref().is_some_and(|found| found.eq_ignore_ascii_case(repo)) {
        return Err(LinkError::Mismatch { found });
    }
    Config::add_repository(config_dir, checkout).map_err(LinkError::Config)?;
    Ok(checkout.to_path_buf())
}

/// The configured checkouts' `owner/name`, lower case, for the list's "checkout" column. Unreadable ones are skipped.
pub fn linked_repos(checkouts: &[PathBuf]) -> Vec<String> {
    checkouts
        .iter()
        .filter_map(|checkout| crate::pmd::checkout_identity(checkout).ok())
        .collect()
}
