//! The Project Dashboard data model, kept apart from the GPUI views so it builds and tests without a window.
//!
//! Status data always comes from `gh x status --json` (gh-x is the single source of truth): [`source`] runs it,
//! [`status`] reads its document, and the board only arranges and aggregates gh-x's own verdicts.

pub mod activity;
pub mod cache;
pub mod columns;
pub mod config;
pub mod scheduler;
pub mod source;
pub mod status;
pub mod t3;
pub mod ui_state;
pub mod zoom;

use std::path::{Path, PathBuf};

use crate::activity::{Activity, last_activity};
use crate::cache::StatusCache;
use crate::scheduler::Outcome;
use crate::source::FetchError;
use crate::status::StatusDocument;

/// The app's name, as the window title and the taskbar show it.
pub const APP_NAME: &str = "Project Dashboard";

/// One configured checkout and what is known about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub checkout: PathBuf,
    /// The latest status, fresh or from the cache.
    pub document: Option<StatusDocument>,
    /// True while `document` is the cached copy from an earlier run.
    pub from_cache: bool,
    /// True while a refresh for this checkout is running.
    pub refreshing: bool,
    /// The latest refresh's failure. The last good document stays alongside it.
    pub error: Option<FetchError>,
}

/// A card's overall state, for its badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overall {
    /// No status yet: the first refresh is still running.
    Loading,
    /// Every health row is clean by gh-x's verdict and every GitHub section loaded.
    Clean,
    /// gh-x reports something to look at: a row that isn't clean, or a section that didn't load.
    Attention,
    /// The latest refresh failed.
    Failed,
}

impl Card {
    pub fn new(checkout: PathBuf) -> Self {
        Self {
            checkout,
            document: None,
            from_cache: false,
            refreshing: false,
            error: None,
        }
    }

    /// `owner/name` from gh-x, else the checkout's folder name.
    pub fn name(&self) -> String {
        match &self.document {
            Some(document) => document.display_name().to_owned(),
            None => folder_name(&self.checkout),
        }
    }

    /// The newest PR, issue or run in the latest status.
    pub fn last_activity(&self) -> Option<Activity> {
        self.document.as_ref().and_then(last_activity)
    }

    pub fn overall(&self) -> Overall {
        if self.error.is_some() {
            return Overall::Failed;
        }
        match &self.document {
            None => Overall::Loading,
            Some(document) if document.is_clean() => Overall::Clean,
            Some(_) => Overall::Attention,
        }
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Everything the window shows: one card per configured checkout, in config order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    cards: Vec<Card>,
}

impl Board {
    /// One card per checkout, showing its cached status (if any) until the first refresh lands. A checkout listed
    /// twice gets one card, at its first position.
    pub fn new(checkouts: &[PathBuf], cache: &StatusCache) -> Self {
        let mut seen = std::collections::HashSet::new();
        let cards = checkouts
            .iter()
            .filter(|checkout| seen.insert(*checkout))
            .map(|checkout| {
                let mut card = Card::new(checkout.clone());
                if let Some(document) = cache.get(checkout) {
                    card.document = Some(document.clone());
                    card.from_cache = true;
                }
                card
            })
            .collect();
        Self { cards }
    }

    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    pub fn card(&self, checkout: &Path) -> Option<&Card> {
        self.cards.iter().find(|card| card.checkout == checkout)
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// The message an empty board shows instead of cards, or `None` when there is something to show.
    pub fn empty_state(&self) -> Option<EmptyState> {
        self.is_empty().then_some(EmptyState::NO_REPOSITORIES)
    }

    /// Marks these checkouts as refreshing.
    pub fn start_refresh(&mut self, checkouts: &[PathBuf]) {
        for card in &mut self.cards {
            if checkouts.contains(&card.checkout) {
                card.refreshing = true;
            }
        }
    }

    /// Applies one refresh result to its card and the cache. Returns false for a checkout no longer on the board.
    pub fn apply(&mut self, outcome: Outcome, cache: &mut StatusCache) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.checkout == outcome.checkout) else {
            return false;
        };
        card.refreshing = false;
        match outcome.result {
            Ok(document) => {
                cache.insert(outcome.checkout, document.clone());
                card.document = Some(document);
                card.from_cache = false;
                card.error = None;
            }
            Err(error) => card.error = Some(error),
        }
        true
    }

    pub fn is_refreshing(&self) -> bool {
        self.cards.iter().any(|card| card.refreshing)
    }

    /// A missing `gh` or gh-x, which every card would hit: the board shows it once instead of on each card.
    pub fn setup_problem(&self) -> Option<&FetchError> {
        self.cards
            .iter()
            .filter_map(|card| card.error.as_ref())
            .find(|error| error.is_setup())
    }

    /// The cards' indices in display order: config order, or by update time (the last activity's), newest or oldest
    /// first. Cards without an update time come after the rest in either direction, in config order.
    pub fn order(&self, sort: SortOrder) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.cards.len()).collect();
        if sort == SortOrder::Config {
            return order;
        }
        let updated: Vec<_> = self
            .cards
            .iter()
            .map(|card| card.last_activity().map(|activity| activity.at))
            .collect();
        // Stable, so equal times and cards without an update time keep config order.
        order.sort_by(|&a, &b| match (updated[a], updated[b]) {
            (Some(a), Some(b)) if sort == SortOrder::NewestFirst => b.cmp(&a),
            (Some(a), Some(b)) => a.cmp(&b),
            (a, b) => b.is_some().cmp(&a.is_some()),
        });
        order
    }

    pub fn checkouts(&self) -> Vec<PathBuf> {
        self.cards.iter().map(|card| card.checkout.clone()).collect()
    }
}

/// How the table orders its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    /// The order of `repositories` in config.toml.
    #[default]
    Config,
    /// Most recently updated first.
    NewestFirst,
    /// Least recently updated first.
    OldestFirst,
}

impl SortOrder {
    /// The order after a click on the Updated header: newest first, then oldest first, then config order again.
    pub fn next(self) -> Self {
        match self {
            Self::Config => Self::NewestFirst,
            Self::NewestFirst => Self::OldestFirst,
            Self::OldestFirst => Self::Config,
        }
    }
}

/// A headline and a hint, for a board with nothing to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmptyState {
    pub title: &'static str,
    pub hint: &'static str,
}

impl EmptyState {
    pub const NO_REPOSITORIES: Self = Self {
        title: "No repositories yet",
        hint: "Add local checkouts to the repositories list in the dashboard's config.toml to see their gh x status here.",
    };
}

/// The checked-in `gh x status --json` fixtures, for tests and screenshots.
pub mod fixtures {
    use crate::status::StatusDocument;

    pub const CLEAN: &str = include_str!("../tests/fixtures/status-clean.json");
    pub const ATTENTION: &str = include_str!("../tests/fixtures/status-attention.json");
    pub const UNAVAILABLE: &str = include_str!("../tests/fixtures/status-unavailable.json");
    pub const ERROR: &str = include_str!("../tests/fixtures/status-error.json");

    pub fn document(json: &str) -> StatusDocument {
        StatusDocument::parse(json).expect("fixtures parse")
    }
}
