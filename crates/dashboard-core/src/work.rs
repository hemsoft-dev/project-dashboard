//! The Work items dialog's data, without a window (#35): pmd's work items, the list's filters and paging, and the
//! `PATCH /v1/work/{id}` an edit sends.
//!
//! The editor is an [`edit::Form`](crate::edit::Form) over four fields, as text: `status` (a manual status, or `auto`
//! while the rules decide), `statusReason`, `priority` (the item's own, empty to inherit the project's) and `rank`.
//! [`patch`] turns the fields that changed into pmd's body, after the same checks pmd makes, and
//! [`reason_note`] says when a save will clear a reason. pmd's rules are in its [Write API](
//! https://github.com/hemsoft-dev/project-manager#write-api).
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::edit::{Form, Row};
use crate::write::WriteError;

/// The editor's fields, as pmd names them, so a `400` that names one lands next to it.
pub const FIELDS: [&str; 4] = ["status", "statusReason", "priority", "rank"];
/// The manual statuses, in Board order. `auto` hands the status back to the rules.
pub const STATUSES: [&str; 4] = ["ready", "in_progress", "waiting_on_franz", "blocked"];
pub const AUTO: &str = "auto";
/// pmd's limits on a reason's length (characters) and on a rank.
pub const MAX_REASON: usize = 500;
pub const MAX_RANK: i64 = 1_000_000_000;
/// Items per page. pmd allows up to 500; a page this size keeps the dialog quick.
pub const PAGE: usize = 50;

/// A status's label, as the Board titles its columns.
pub fn status_label(status: &str) -> &str {
    match status {
        "ready" => "Ready",
        "in_progress" => "In progress",
        "waiting_on_franz" => "Waiting on Franz",
        "blocked" => "Blocked",
        "done" => "Done",
        AUTO => "Automatic",
        other => other,
    }
}

/// One work item as `GET /v1/work` and `PATCH /v1/work/{id}` return it; only what the dialog shows or edits.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub id: String,
    pub project: String,
    /// `issue` or `pr`.
    pub kind: String,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub url: String,
    /// `open`, `closed` or `merged`.
    #[serde(default)]
    pub gh_state: String,
    pub status: String,
    /// `auto` while pmd's rules decide the status, `manual` after a writer set it.
    pub status_source: String,
    pub status_reason: Option<String>,
    /// The effective priority: the item's own, else its project's; `None` for no tier.
    pub priority: Option<u8>,
    /// The item's own priority, when it overrides the project's.
    pub priority_override: Option<u8>,
    pub rank: i64,
    pub version: u64,
}

impl WorkItem {
    pub fn from_json(value: &Value) -> Result<Self, WriteError> {
        serde_json::from_value(value.clone()).map_err(|_| WriteError::Malformed)
    }

    /// The API path of this item.
    pub fn path(&self) -> String {
        item_path(&self.id)
    }

    pub fn is_manual(&self) -> bool {
        self.status_source == "manual"
    }

    /// Closed or merged on GitHub: its status is `done` and can't be changed.
    pub fn is_closed(&self) -> bool {
        self.status == "done" || matches!(self.gh_state.as_str(), "closed" | "merged")
    }

    /// The editor's starting values: a manual status as itself, an automatic one as `auto`.
    pub fn row(&self) -> Row {
        let status = if self.is_manual() { self.status.as_str() } else { AUTO };
        let values = [
            ("status", status.to_owned()),
            ("statusReason", self.status_reason.clone().unwrap_or_default()),
            (
                "priority",
                self.priority_override.map(|p| p.to_string()).unwrap_or_default(),
            ),
            ("rank", self.rank.to_string()),
        ]
        .into_iter()
        .map(|(field, value)| (field.to_owned(), value))
        .collect();
        Row {
            version: self.version,
            values,
        }
    }
}

/// The API path of a work item. pmd's ids are ULIDs; anything else is refused by the client as a malformed path.
pub fn item_path(id: &str) -> String {
    format!("v1/work/{id}")
}

/// One page of `GET /v1/work`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkPage {
    pub items: Vec<WorkItem>,
    /// Where the next page starts; `None` on the last page.
    pub next_cursor: Option<String>,
}

impl WorkPage {
    pub fn from_json(value: &Value) -> Result<Self, WriteError> {
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or(WriteError::Malformed)?
            .iter()
            .map(WorkItem::from_json)
            .collect::<Result<_, _>>()?;
        let next_cursor = value
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_owned);
        Ok(Self { items, next_cursor })
    }
}

/// `issue` or `pr`, for the kind filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Issue,
    Pr,
}

impl Kind {
    pub fn query(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Pr => "pr",
        }
    }
}

/// The list's filters. Empty means everything open, as pmd's default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkFilter {
    /// Project slugs, separated by commas or spaces.
    pub project: String,
    /// One of [`STATUSES`], or `None` for every open status.
    pub status: Option<&'static str>,
    pub kind: Option<Kind>,
    /// Items at this priority or more urgent (0 is the most).
    pub max_priority: Option<u8>,
}

impl WorkFilter {
    /// The query for one page, starting at `cursor`.
    pub fn query(&self, cursor: Option<&str>) -> Vec<(String, String)> {
        let projects: Vec<&str> = self
            .project
            .split([',', ' '])
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .collect();
        let mut query = vec![("limit".to_owned(), PAGE.to_string())];
        if !projects.is_empty() {
            query.push(("project".to_owned(), projects.join(",")));
        }
        if let Some(status) = self.status {
            query.push(("status".to_owned(), status.to_owned()));
        }
        if let Some(kind) = self.kind {
            query.push(("kind".to_owned(), kind.query().to_owned()));
        }
        if let Some(priority) = self.max_priority {
            query.push(("maxPriority".to_owned(), priority.to_string()));
        }
        if let Some(cursor) = cursor {
            query.push(("cursor".to_owned(), cursor.to_owned()));
        }
        query
    }
}

/// A problem with one field, found before sending: `(field, reason)`.
pub type FieldProblem = (&'static str, String);

/// The changed fields as pmd's `PATCH` body, or the problems that stop the save. `Ok(None)` when nothing changed.
///
/// A new status carries the reason when there is one, since a status sent alone clears it; `auto` never carries one.
/// A reason changed under an unchanged manual status goes alone, `null` when emptied. An empty priority inherits the
/// project's.
pub fn patch(form: &Form) -> Result<Option<Value>, Vec<FieldProblem>> {
    let Some(base) = form.base() else {
        return Ok(None);
    };
    let before = |field: &str| base.values.get(field).map_or("", String::as_str);
    let status = form.value("status");
    let reason = form.value("statusReason").trim();
    let mut problems = Vec::new();
    if status != AUTO && !STATUSES.contains(&status) {
        problems.push(("status", format!("Pick a status; {status:?} isn't one.")));
    }
    if status != AUTO && reason.chars().count() > MAX_REASON {
        problems.push(("statusReason", format!("At most {MAX_REASON} characters.")));
    }
    let priority = form.value("priority").trim();
    let priority_value = match priority {
        "" => Value::Null,
        text => match text.parse::<u8>() {
            Ok(number) if number <= 5 => number.into(),
            _ => {
                problems.push(("priority", "Priority is P0 to P5, or the project's.".to_owned()));
                Value::Null
            }
        },
    };
    let rank = form.value("rank").trim();
    let rank_value = match rank.parse::<i64>() {
        Ok(number) if number.abs() <= MAX_RANK => Some(number),
        _ => {
            problems.push((
                "rank",
                format!("Rank is a whole number from -{MAX_RANK} to {MAX_RANK}."),
            ));
            None
        }
    };
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut body = Map::new();
    if status != before("status") {
        body.insert("status".to_owned(), status.into());
        if status != AUTO && !reason.is_empty() {
            body.insert("statusReason".to_owned(), reason.into());
        }
    } else if status != AUTO && reason != before("statusReason").trim() {
        let value = if reason.is_empty() { Value::Null } else { reason.into() };
        body.insert("statusReason".to_owned(), value);
    }
    if priority != before("priority") {
        body.insert("priority".to_owned(), priority_value);
    }
    if rank != before("rank")
        && let Some(rank) = rank_value
        && Some(rank) != before("rank").parse().ok()
    {
        body.insert("rank".to_owned(), rank.into());
    }
    Ok((!body.is_empty()).then_some(Value::Object(body)))
}

/// Whether a save of these values would reorder the list, so it reloads from the first page: pmd warns that a listing
/// in progress can skip or repeat an item after a rank or priority change.
pub fn reorders(body: &Value) -> bool {
    body.get("rank").is_some() || body.get("priority").is_some()
}

/// The note the editor shows before Save when the save would drop the item's current reason.
pub fn reason_note(form: &Form) -> Option<String> {
    let base = form.base()?;
    let old = base.values.get("statusReason").map_or("", |reason| reason.trim());
    if old.is_empty() {
        return None;
    }
    let status = form.value("status");
    let changed = status != base.values.get("status").map_or("", String::as_str);
    if status == AUTO && changed {
        return Some("Back to automatic clears the current reason.".to_owned());
    }
    if changed && form.value("statusReason").trim().is_empty() {
        return Some("A new status without a reason clears the current reason.".to_owned());
    }
    if !changed && status != AUTO && form.value("statusReason").trim().is_empty() {
        return Some("Saving clears the reason.".to_owned());
    }
    None
}
