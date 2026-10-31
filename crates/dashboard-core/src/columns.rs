//! Resizable table columns (#14): each resizable column's default and least width, and the widths a user dragged to.
//!
//! Widths are in rems, so they scale with the zoom like the rest of the table. The last activity column isn't
//! listed: it takes whatever width the others leave.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The widest any column can be dragged, in rems.
pub const MAX_WIDTH: f32 = 60.0;

/// A table column whose right border can be dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Repository,
    Priority,
    PullRequests,
    Issues,
    Merged24h,
    Merged7d,
    Updated,
    FleetMachine,
    FleetConnectivity,
    FleetServer,
    FleetVersions,
    FleetClient,
    FleetSessions,
    FleetFree,
    FleetChecked,
    FleetWork,
}

impl Column {
    pub const ALL: [Column; 7] = [
        Self::Repository,
        Self::Priority,
        Self::PullRequests,
        Self::Issues,
        Self::Merged24h,
        Self::Merged7d,
        Self::Updated,
    ];

    pub const FLEET: [Column; 9] = [
        Self::FleetMachine,
        Self::FleetConnectivity,
        Self::FleetServer,
        Self::FleetVersions,
        Self::FleetClient,
        Self::FleetSessions,
        Self::FleetFree,
        Self::FleetChecked,
        Self::FleetWork,
    ];

    /// The width before any drag, in rems. The repository column fits a typical `owner/name` and its Cached tag.
    pub fn default_width(self) -> f32 {
        match self {
            Self::Repository => 18.0,
            Self::Priority => 4.5,
            Self::PullRequests | Self::Issues => 3.25,
            Self::Merged24h | Self::Merged7d => 6.5,
            Self::Updated => 4.5,
            Self::FleetMachine => 9.0,
            Self::FleetConnectivity => 8.0,
            Self::FleetServer => 13.0,
            Self::FleetVersions => 24.0,
            Self::FleetClient => 12.0,
            Self::FleetSessions => 9.0,
            Self::FleetFree => 7.0,
            Self::FleetChecked => 15.0,
            Self::FleetWork => 24.0,
        }
    }

    /// The least width a drag can leave, in rems: enough for the header to stay readable.
    pub fn min_width(self) -> f32 {
        match self {
            Self::Repository => 6.0,
            Self::Priority => 3.0,
            Self::PullRequests => 1.75,
            Self::Issues => 2.25,
            Self::Merged24h | Self::Merged7d => 4.5,
            Self::Updated => 3.5,
            Self::FleetMachine => 6.0,
            Self::FleetConnectivity => 7.0,
            Self::FleetServer => 8.0,
            Self::FleetVersions => 12.0,
            Self::FleetClient => 8.0,
            Self::FleetSessions => 8.0,
            Self::FleetFree => 6.0,
            Self::FleetChecked => 10.0,
            Self::FleetWork => 10.0,
        }
    }

    /// The column's name in `ui-state.json`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Priority => "priority",
            Self::PullRequests => "pullRequests",
            Self::Issues => "issues",
            Self::Merged24h => "merged24h",
            Self::Merged7d => "merged7d",
            Self::Updated => "updated",
            Self::FleetMachine => "fleetMachine",
            Self::FleetConnectivity => "fleetConnectivity",
            Self::FleetServer => "fleetServer",
            Self::FleetVersions => "fleetVersions",
            Self::FleetClient => "fleetClient",
            Self::FleetSessions => "fleetSessions",
            Self::FleetFree => "fleetFree",
            Self::FleetChecked => "fleetChecked",
            Self::FleetWork => "fleetWork",
        }
    }

    /// `width` kept between this column's minimum and [`MAX_WIDTH`], on a hundredth of a rem; anything that isn't a
    /// finite number is the default.
    pub fn clamp(self, width: f32) -> f32 {
        if !width.is_finite() {
            return self.default_width();
        }
        (width.clamp(self.min_width(), MAX_WIDTH) * 100.0).round() / 100.0
    }
}

/// The resizable columns' widths, in rems.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnWidths {
    repository: f32,
    priority: f32,
    pull_requests: f32,
    issues: f32,
    #[serde(default = "merge_width")]
    merged_24h: f32,
    #[serde(default = "merge_width")]
    merged_7d: f32,
    updated: f32,
    #[serde(default)]
    fleet: [f32; 9],
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            repository: Column::Repository.default_width(),
            priority: Column::Priority.default_width(),
            pull_requests: Column::PullRequests.default_width(),
            issues: Column::Issues.default_width(),
            merged_24h: merge_width(),
            merged_7d: merge_width(),
            updated: Column::Updated.default_width(),
            fleet: Column::FLEET.map(Column::default_width),
        }
    }
}

impl ColumnWidths {
    pub fn get(&self, column: Column) -> f32 {
        match column {
            Column::Repository => self.repository,
            Column::Priority => self.priority,
            Column::PullRequests => self.pull_requests,
            Column::Issues => self.issues,
            Column::Merged24h => self.merged_24h,
            Column::Merged7d => self.merged_7d,
            Column::Updated => self.updated,
            Column::FleetMachine => self.fleet[0],
            Column::FleetConnectivity => self.fleet[1],
            Column::FleetServer => self.fleet[2],
            Column::FleetVersions => self.fleet[3],
            Column::FleetClient => self.fleet[4],
            Column::FleetSessions => self.fleet[5],
            Column::FleetFree => self.fleet[6],
            Column::FleetChecked => self.fleet[7],
            Column::FleetWork => self.fleet[8],
        }
    }

    /// Sets `column` to `width`, kept within its bounds.
    pub fn set(&mut self, column: Column, width: f32) {
        let width = column.clamp(width);
        match column {
            Column::Repository => self.repository = width,
            Column::Priority => self.priority = width,
            Column::PullRequests => self.pull_requests = width,
            Column::Issues => self.issues = width,
            Column::Merged24h => self.merged_24h = width,
            Column::Merged7d => self.merged_7d = width,
            Column::Updated => self.updated = width,
            Column::FleetMachine => self.fleet[0] = width,
            Column::FleetConnectivity => self.fleet[1] = width,
            Column::FleetServer => self.fleet[2] = width,
            Column::FleetVersions => self.fleet[3] = width,
            Column::FleetClient => self.fleet[4] = width,
            Column::FleetSessions => self.fleet[5] = width,
            Column::FleetFree => self.fleet[6] = width,
            Column::FleetChecked => self.fleet[7] = width,
            Column::FleetWork => self.fleet[8] = width,
        }
    }

    pub fn reset(&mut self, column: Column) {
        self.set(column, column.default_width());
    }

    /// The widths saved in `value`, one column at a time: a missing, non-numeric or out-of-range width falls back to
    /// that column's default or bound without affecting the others.
    pub fn from_value(value: Option<&Value>) -> Self {
        let mut widths = Self::default();
        for column in Column::ALL.into_iter().chain(Column::FLEET) {
            if let Some(width) = value.and_then(|value| value.get(column.key())).and_then(Value::as_f64) {
                // Bounded before the cast, so a huge saved width clamps to the maximum instead of becoming infinite.
                let width = width.clamp(f64::from(-MAX_WIDTH), f64::from(MAX_WIDTH));
                widths.set(column, width as f32);
            }
        }
        if let Some(saved) = value.and_then(|v| v.get("fleet")).and_then(Value::as_array) {
            for (column, v) in Column::FLEET.into_iter().zip(saved) {
                if let Some(w) = v.as_f64() {
                    widths.set(column, w.clamp(-60.0, 60.0) as f32);
                }
            }
        }
        widths
    }

    /// The sum of every resizable column's width, in rems.
    pub fn total(&self) -> f32 {
        Column::ALL.iter().map(|&column| self.get(column)).sum()
    }
}

fn merge_width() -> f32 {
    6.5
}
