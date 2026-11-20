//! The Project table's row order and name filter (#98), and the Backlog's column sort (#118).
//!
//! Every column of either table sorts in both directions, and rows without a value for the sorted column follow every
//! row that has one, in either direction. In the Project table, rows that compare equal keep config order, and a
//! header click cycles its first direction, the reverse, then config order again. The Backlog sorts within each stage
//! group: equal rows keep pmd's dispatch order, and a header click cycles ascending, descending, then dispatch order.

use std::cmp::Ordering;

use crate::Card;
use crate::backlog::Row;
use crate::columns::Column;

/// A Project-table column the rows can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortColumn {
    /// The displayed project name, ignoring case.
    Repository,
    /// pmd's priority tier, P0 first when ascending.
    Priority,
    /// Open pull requests.
    PullRequests,
    /// Open issues.
    Issues,
    Merged24h,
    Merged7d,
    /// The newest PR, issue or run's timestamp.
    Updated,
    /// The newest activity's displayed text ("PR #12 Fix the thing"), ignoring case.
    Activity,
    /// The linked T3 thread's title, ignoring case.
    Session,
}

impl SortColumn {
    /// Every column, in table order.
    pub const ALL: [SortColumn; 9] = [
        Self::Repository,
        Self::Priority,
        Self::PullRequests,
        Self::Issues,
        Self::Merged24h,
        Self::Merged7d,
        Self::Updated,
        Self::Activity,
        Self::Session,
    ];

    /// The resizable table column this sorts.
    pub fn column(self) -> Column {
        match self {
            Self::Repository => Column::Repository,
            Self::Priority => Column::Priority,
            Self::PullRequests => Column::PullRequests,
            Self::Issues => Column::Issues,
            Self::Merged24h => Column::Merged24h,
            Self::Merged7d => Column::Merged7d,
            Self::Updated => Column::Updated,
            Self::Activity => Column::Activity,
            Self::Session => Column::Session,
        }
    }

    /// The header's text.
    pub fn title(self) -> &'static str {
        match self {
            Self::Repository => "Repository",
            Self::Priority => "Priority",
            Self::PullRequests => "PRs",
            Self::Issues => "Issues",
            Self::Merged24h => "Merged 24h",
            Self::Merged7d => "Merged 7d",
            Self::Updated => "Updated",
            Self::Activity => "Last activity",
            Self::Session => "T3 session",
        }
    }

    /// The direction a first click sorts in: text A to Z, P0 first, counts and update times largest first.
    pub fn first_direction(self) -> Direction {
        match self {
            Self::Repository | Self::Priority | Self::Activity | Self::Session => Direction::Ascending,
            Self::PullRequests | Self::Issues | Self::Merged24h | Self::Merged7d | Self::Updated => {
                Direction::Descending
            }
        }
    }

    /// What the direction means for this column, for the header's accessible label.
    fn meaning(self, direction: Direction) -> &'static str {
        match (self, direction) {
            (Self::Repository | Self::Activity | Self::Session, Direction::Ascending) => "A to Z",
            (Self::Repository | Self::Activity | Self::Session, Direction::Descending) => "Z to A",
            (Self::Priority, Direction::Ascending) => "highest first",
            (Self::Priority, Direction::Descending) => "lowest first",
            (Self::Updated, Direction::Ascending) => "oldest first",
            (Self::Updated, Direction::Descending) => "newest first",
            (_, Direction::Ascending) => "fewest first",
            (_, Direction::Descending) => "most first",
        }
    }
}

/// Smallest value first, or largest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

impl Direction {
    pub fn reverse(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// The header's arrow: up for ascending, down for descending.
    pub fn arrow(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }
}

/// How the table orders its rows. Never saved: every start sorts by Updated, newest first (#29).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    /// The order of `repositories` in config.toml.
    #[default]
    Config,
    /// By one column's value, in one direction.
    By(SortColumn, Direction),
}

impl SortOrder {
    /// Most recently updated first; the order at startup.
    pub const NEWEST_FIRST: Self = Self::By(SortColumn::Updated, Direction::Descending);
    /// Least recently updated first.
    pub const OLDEST_FIRST: Self = Self::By(SortColumn::Updated, Direction::Ascending);
    /// P0 first, unknown priorities last.
    pub const PRIORITY_FIRST: Self = Self::By(SortColumn::Priority, Direction::Ascending);
    /// Highest priority number first, unknown priorities last.
    pub const PRIORITY_LAST: Self = Self::By(SortColumn::Priority, Direction::Descending);

    /// The order after a click on `column`'s header: its first direction, the reverse, then config order again.
    /// A click on another column than the sorted one starts that column at its first direction.
    pub fn next_for(self, column: SortColumn) -> Self {
        let first = column.first_direction();
        match self {
            Self::By(sorted, direction) if sorted == column && direction == first => Self::By(column, first.reverse()),
            Self::By(sorted, _) if sorted == column => Self::Config,
            _ => Self::By(column, first),
        }
    }

    /// The order after a click on the Updated header: newest first, then oldest first, then config order again.
    pub fn next(self) -> Self {
        self.next_for(SortColumn::Updated)
    }

    /// The order after a click on the Priority header: highest first, then lowest first, then config order again.
    pub fn next_priority(self) -> Self {
        self.next_for(SortColumn::Priority)
    }

    /// The direction `column` is sorted in, or `None` when the table is sorted by something else.
    pub fn direction_of(self, column: SortColumn) -> Option<Direction> {
        match self {
            Self::By(sorted, direction) if sorted == column => Some(direction),
            _ => None,
        }
    }

    /// `column`'s header label: "PRs, most first" or "PRs, not sorted".
    pub fn label(self, column: SortColumn) -> String {
        match self.direction_of(column) {
            Some(direction) => format!("{}, {}", column.title(), column.meaning(direction)),
            None => format!("{}, not sorted", column.title()),
        }
    }
}

/// One row's value for the sorted column. Text is lowercased, so case never decides the order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Number(u64),
    Time(jiff::Timestamp),
    Text(String),
    /// A work item: whether it is a pull request (issues first), then its number.
    Item(bool, u64),
}

/// What a row shows for `column`, as a comparable key, or `None` when it shows no value.
fn key(card: &Card, column: SortColumn, priority: Option<u8>, session: Option<String>) -> Option<Key> {
    match column {
        SortColumn::Repository => Some(Key::Text(card.name().to_lowercase())),
        SortColumn::Priority => priority.map(|p| Key::Number(p.into())),
        SortColumn::PullRequests => card.open_pull_requests().value().map(Key::Number),
        SortColumn::Issues => card.open_issues().value().map(Key::Number),
        SortColumn::Merged24h => card.merged_24h().map(Key::Number),
        SortColumn::Merged7d => card.merged_7d().map(Key::Number),
        SortColumn::Updated => card.last_activity().map(|activity| Key::Time(activity.at)),
        SortColumn::Activity => card.activity_text().map(|text| Key::Text(text.to_lowercase())),
        SortColumn::Session => session.map(|title| Key::Text(title.to_lowercase())),
    }
}

/// Orders `rows` (card indices) by `sort`. Stable: equal values and rows without one keep their order.
pub(crate) fn order(
    cards: &[Card],
    rows: &mut [usize],
    sort: SortOrder,
    priority: impl Fn(&Card) -> Option<u8>,
    session: impl Fn(&Card) -> Option<String>,
) {
    let SortOrder::By(column, direction) = sort else {
        return;
    };
    let keys: Vec<Option<Key>> = cards
        .iter()
        .map(|card| {
            let priority = (column == SortColumn::Priority).then(|| priority(card)).flatten();
            let session = (column == SortColumn::Session).then(|| session(card)).flatten();
            key(card, column, priority, session)
        })
        .collect();
    rows.sort_by(|&a, &b| compare(keys[a].as_ref(), keys[b].as_ref(), direction));
}

/// Values in `direction`; a missing value after every present one, in either direction.
fn compare<K: Ord>(a: Option<&K>, b: Option<&K>, direction: Direction) -> Ordering {
    match (a, b, direction) {
        (Some(a), Some(b), Direction::Ascending) => a.cmp(b),
        (Some(a), Some(b), Direction::Descending) => b.cmp(a),
        (a, b, _) => b.is_some().cmp(&a.is_some()),
    }
}

/// A Backlog column the rows can be sorted by (#118).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BacklogColumn {
    /// The project slug, ignoring case.
    Project,
    /// pmd's priority tier, P0 first when ascending.
    Priority,
    /// The row's first item: issues before pull requests, then by number.
    Item,
    /// Why the work waits, ignoring case.
    Reason,
    /// The live holders ("machine · model"), ignoring case.
    Holder,
}

impl BacklogColumn {
    /// Every column, in table order.
    pub const ALL: [BacklogColumn; 5] = [Self::Project, Self::Priority, Self::Item, Self::Reason, Self::Holder];

    /// The header's text.
    pub fn title(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Priority => "Priority",
            Self::Item => "Item",
            Self::Reason => "Reason",
            Self::Holder => "Holder",
        }
    }

    /// What the direction means for this column, for the header's accessible label. A first click is always
    /// ascending: text A to Z, P0 first, issues and low numbers first.
    fn meaning(self, direction: Direction) -> &'static str {
        match (self, direction) {
            (Self::Priority, Direction::Ascending) => "highest first",
            (Self::Priority, Direction::Descending) => "lowest first",
            (Self::Item, Direction::Ascending) => "issues first, lowest number first",
            (Self::Item, Direction::Descending) => "pull requests first, highest number first",
            (_, Direction::Ascending) => "A to Z",
            (_, Direction::Descending) => "Z to A",
        }
    }

    /// What `row` shows for this column, as a comparable key, or `None` when it shows no value.
    fn key(self, row: &Row) -> Option<Key> {
        match self {
            Self::Project => Some(Key::Text(row.project.to_lowercase())),
            Self::Priority => row.priority().map(|p| Key::Number(p.into())),
            Self::Item => row.items.first().map(|i| Key::Item(i.kind != "issue", i.number)),
            Self::Reason => (!row.reason.is_empty()).then(|| Key::Text(row.reason.to_lowercase())),
            Self::Holder => (!row.workers.is_empty()).then(|| Key::Text(row.workers.join(" / ").to_lowercase())),
        }
    }
}

/// How the Backlog orders the rows within each stage group (#118). Never saved: every start uses pmd's dispatch order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BacklogSort {
    /// pmd's dispatch order: priority, rank, then the dispatch plan (`backlog::rows`).
    #[default]
    Dispatch,
    /// By one column's value, in one direction.
    By(BacklogColumn, Direction),
}

impl BacklogSort {
    /// The order after a click on `column`'s header: ascending, descending, then dispatch order again. A click on
    /// another column than the sorted one starts that column ascending.
    pub fn next_for(self, column: BacklogColumn) -> Self {
        match self {
            Self::By(sorted, Direction::Ascending) if sorted == column => Self::By(column, Direction::Descending),
            Self::By(sorted, _) if sorted == column => Self::Dispatch,
            _ => Self::By(column, Direction::Ascending),
        }
    }

    /// The direction `column` is sorted in, or `None` when the Backlog is sorted by something else.
    pub fn direction_of(self, column: BacklogColumn) -> Option<Direction> {
        match self {
            Self::By(sorted, direction) if sorted == column => Some(direction),
            _ => None,
        }
    }

    /// `column`'s header label: "Project, A to Z" or "Project, not sorted".
    pub fn label(self, column: BacklogColumn) -> String {
        match self.direction_of(column) {
            Some(direction) => format!("{}, {}", column.title(), column.meaning(direction)),
            None => format!("{}, not sorted", column.title()),
        }
    }

    /// Orders `rows`, which `backlog::rows` returned in dispatch order, within each stage group. Groups keep their
    /// order; rows without a value follow every row with one; equal rows keep dispatch order.
    pub fn apply(self, rows: &mut [Row]) {
        let Self::By(column, direction) = self else {
            return;
        };
        rows.sort_by(|a, b| {
            a.group
                .cmp(&b.group)
                .then_with(|| compare(column.key(a).as_ref(), column.key(b).as_ref(), direction))
        });
    }
}

/// Whether the displayed project name contains `filter`, ignoring case and surrounding spaces. An empty filter
/// matches every row.
pub fn name_matches(name: &str, filter: &str) -> bool {
    let filter = filter.trim();
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}
