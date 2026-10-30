//! The Project Dashboard data model, kept apart from the GPUI views so it builds and tests without a window.
//!
//! Production status, priority and Board/work data come from pmd's read API.
//! Hosted verdicts are rendered as supplied; local health is unavailable.

pub mod activity;
pub mod backlog;
pub mod cache;
pub mod columns;
pub mod config;
pub mod edit;
pub mod fleet;
pub mod placement;
pub mod pmd;
pub mod priority;
pub mod projects;
pub mod recovery;
pub mod scheduler;
pub mod source;
pub mod status;
pub mod t3;
pub mod ui_state;
pub mod work;
pub mod write;
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
    pub recovery: recovery::Recovery,
    pub checkout: PathBuf,
    /// The latest status, fresh or from the cache.
    pub document: Option<StatusDocument>,
    /// True for a startup cache, stale API status or a last good result retained after a failed refresh.
    pub from_cache: bool,
    /// True while a refresh for this checkout is running.
    pub refreshing: bool,
    /// The latest refresh's failure. The last good document stays alongside it.
    pub error: Option<FetchError>,
    /// Identifies this card's current data/source generation. Replaced credentials, services and identities
    /// supersede older refreshes, even when the displayed data is retained.
    pub generation: u64,
    pub identity: Option<Result<String, FetchError>>,
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
    /// pmd's status loaded but its Board didn't, and nothing else is flagged: whether anything is blocked is unknown.
    Unavailable,
    /// pmd has a work item marked blocked.
    Blocked,
    /// pmd has a work item waiting on Franz.
    WaitingOnFranz,
    /// An open PR's checks fail, or the default branch's newest run of a workflow failed.
    ChecksFailing,
}

/// What a pmd-hosted project's state dot reflects. pmd supplies no overall verdict, so the dashboard reads the work
/// items and checks it does supply, as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub blocked: usize,
    pub waiting_on_franz: usize,
    /// "PR #12" or "CI on main", per [`StatusDocument::failing_checks`].
    pub failing: Vec<String>,
    /// Whether pmd's Board loaded; without it, blocked and waiting are unknown.
    pub board: bool,
}

impl Signals {
    /// The state these signals give, most urgent first: blocked, waiting on Franz, failing checks.
    pub fn overall(&self) -> Overall {
        if self.blocked > 0 {
            Overall::Blocked
        } else if self.waiting_on_franz > 0 {
            Overall::WaitingOnFranz
        } else if !self.failing.is_empty() {
            Overall::ChecksFailing
        } else if !self.board {
            Overall::Unavailable
        } else {
            Overall::Clean
        }
    }

    /// Every signal, for the dot's tooltip: "Blocked: 1 · Waiting on Franz: 2 · Checks failing: PR #12".
    pub fn describe(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.blocked > 0 {
            parts.push(format!("Blocked: {}", self.blocked));
        }
        if self.waiting_on_franz > 0 {
            parts.push(format!("Waiting on Franz: {}", self.waiting_on_franz));
        }
        if !self.failing.is_empty() {
            parts.push(format!("Checks failing: {}", self.failing.join(", ")));
        }
        if !self.board {
            parts.push("Board unavailable".to_owned());
        }
        parts
    }
}

impl Card {
    pub fn new(checkout: PathBuf) -> Self {
        Self {
            checkout,
            recovery: Default::default(),
            document: None,
            from_cache: false,
            refreshing: false,
            error: None,
            generation: 0,
            identity: None,
        }
    }

    /// Verified hosted identity, then checkout origin, then a folder label when identity is unavailable.
    pub fn name(&self) -> String {
        match &self.document {
            Some(document) => document.display_name().to_owned(),
            None => self
                .identity
                .as_ref()
                .and_then(|identity| identity.as_ref().ok())
                .cloned()
                .unwrap_or_else(|| folder_name(&self.checkout)),
        }
    }

    /// The newest PR, issue or run in the latest status.
    pub fn last_activity(&self) -> Option<Activity> {
        self.document.as_ref().and_then(last_activity)
    }

    /// What pmd's data flags for this project, or `None` without pmd data.
    pub fn signals(&self) -> Option<Signals> {
        let document = self.document.as_ref()?;
        let hosted = document.hosted.as_ref()?;
        Some(Signals {
            blocked: hosted.items("blocked").len(),
            waiting_on_franz: hosted.items("waiting_on_franz").len(),
            failing: document.failing_checks(),
            board: hosted.board.is_some(),
        })
    }

    /// The visible failure: transport errors are shown once on the board, but cannot hide a retained upstream error.
    pub fn display_error(&self) -> Option<FetchError> {
        self.error
            .as_ref()
            .filter(|error| !error.is_transient())
            .cloned()
            .or_else(|| {
                let error = self.document.as_ref()?.hosted.as_ref()?.last_error.as_ref()?;
                Some(FetchError::Failed(format!(
                    "pmd last refresh failed at {}: {}",
                    error.at, error.message
                )))
            })
    }

    pub fn overall(&self) -> Overall {
        if self.display_error().is_some() {
            return Overall::Failed;
        }
        match &self.document {
            None => Overall::Loading,
            Some(document) if document.hosted.is_some() && document.verdict.is_none() => {
                self.signals().unwrap_or_default().overall()
            }
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
    priorities: priority::Priorities,
    service_url: Option<String>,
    /// The next generation for a new card or a replaced source.
    next_generation: u64,
}

impl Board {
    /// One card per checkout, showing its cached status (if any) until the first refresh lands. A checkout listed
    /// twice gets one card, at its first position.
    pub fn new(checkouts: &[PathBuf], cache: &StatusCache) -> Self {
        let mut board = Self::default();
        board.set_checkouts(checkouts, cache);
        board
    }

    /// Follows an edited checkout list (#21): cards for checkouts still listed keep everything they know, in the new
    /// order; new checkouts get a card from the cache, as at startup; the rest are dropped. A checkout listed twice
    /// gets one card, at its first position. Returns the checkouts that are new to the board.
    pub fn set_checkouts(&mut self, checkouts: &[PathBuf], cache: &StatusCache) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        let mut old = std::mem::take(&mut self.cards);
        let mut added = Vec::new();
        for checkout in checkouts.iter().filter(|checkout| seen.insert(*checkout)) {
            match old.iter().position(|card| &card.checkout == checkout) {
                Some(ix) => self.cards.push(old.swap_remove(ix)),
                None => {
                    let mut card = Card::new(checkout.clone());
                    card.generation = self.next_generation;
                    self.next_generation += 1;
                    if let Some(document) = cache.get(checkout) {
                        card.document = Some(document.clone());
                        card.from_cache = true;
                        card.error = document
                            .hosted
                            .as_ref()
                            .and_then(|hosted| hosted.last_error.as_ref())
                            .map(|error| {
                                FetchError::Failed(format!(
                                    "pmd last refresh failed at {}: {}",
                                    error.at, error.message
                                ))
                            });
                    }
                    self.cards.push(card);
                    added.push(checkout.clone());
                }
            }
        }
        added
    }

    /// Invalidate old service/identity results, including in-flight generations and disk cache.
    pub fn invalidate(&mut self, checkout: &Path, cache: &mut StatusCache) {
        if let Some(card) = self.cards.iter_mut().find(|card| card.checkout == checkout) {
            card.generation = self.next_generation;
            self.next_generation += 1;
            card.recovery = Default::default();
            card.document = None;
            card.error = None;
            card.refreshing = false;
            card.from_cache = false;
            cache.remove(checkout);
        }
    }

    /// A new credential source supersedes in-flight results without discarding last good data.
    /// A different service or checkout identity still invalidates data through the normal identity check.
    pub fn replace_pmd_source(&mut self, url: &str, cache: &mut StatusCache) {
        if self.service_url.as_deref() != Some(url) {
            self.priorities = priority::Priorities::default();
        }
        for card in &mut self.cards {
            card.generation = self.next_generation;
            self.next_generation += 1;
            card.refreshing = false;
        }
        self.sync_pmd_identities(url, cache);
    }

    /// Called before refresh and before applying completions, as well as during config polling.
    pub fn sync_pmd_identities(&mut self, url: &str, cache: &mut StatusCache) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        let service_changed = self.service_url.as_deref().is_some_and(|old| old != url);
        self.service_url = Some(url.to_owned());
        for checkout in self.checkouts() {
            let identity = pmd::checkout_identity(&checkout);
            let card = self.card(&checkout).expect("listed card");
            let different = card.identity.as_ref().is_some_and(|old| old != &identity)
                || card.document.as_ref().is_some_and(|doc| {
                    doc.hosted.as_ref().is_none_or(|hosted| hosted.service_url != url)
                        || identity
                            .as_ref()
                            .ok()
                            .is_none_or(|name| !doc.repository.name_with_owner.eq_ignore_ascii_case(name))
                });
            if different || service_changed {
                self.invalidate(&checkout, cache);
                changed.push(checkout.clone());
            }
            self.cards
                .iter_mut()
                .find(|card| card.checkout == checkout)
                .expect("listed card")
                .identity = Some(identity);
        }
        changed
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

    /// Marks these checkouts as refreshing, and returns each one's card generation for [`Board::apply_started`].
    pub fn start_refresh(&mut self, checkouts: &[PathBuf]) -> std::collections::HashMap<PathBuf, u64> {
        let mut started = std::collections::HashMap::new();
        for card in &mut self.cards {
            if checkouts.contains(&card.checkout) {
                card.refreshing = true;
                started.insert(card.checkout.clone(), card.generation);
            }
        }
        started
    }

    /// [`Board::apply`] for a refresh started for card `generation`: an outcome for a card that has since been
    /// removed and added back is ignored, so old work can't mark the new card idle or overwrite a newer result.
    pub fn apply_started(&mut self, outcome: Outcome, generation: u64, cache: &mut StatusCache) -> bool {
        let current = self
            .cards
            .iter()
            .find(|card| card.checkout == outcome.checkout)
            .map(|card| card.generation);
        if current != Some(generation) {
            return false;
        }
        self.apply(outcome, cache)
    }

    /// Applies one refresh result to its card and the cache. Returns false for a checkout no longer on the board.
    pub fn apply(&mut self, outcome: Outcome, cache: &mut StatusCache) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.checkout == outcome.checkout) else {
            return false;
        };
        card.refreshing = false;
        card.recovery
            .record(&outcome.result, std::time::Instant::now(), chrono::Local::now());
        match outcome.result {
            Ok(mut document) => {
                if let Some(hosted) = &mut document.hosted
                    && let Some(previous) = card.document.as_ref().and_then(|doc| doc.hosted.as_ref())
                    && previous.service_url == hosted.service_url
                {
                    if hosted.work_error.is_some() {
                        hosted.board = previous.board.clone();
                        hosted.work = previous.work.clone();
                        hosted.backlog = previous.backlog.clone();
                    } else if hosted.backlog.error.is_some() {
                        // Keep only the failed enrichment. Fresh Project board/work remains authoritative.
                        hosted.backlog.assignments = previous.backlog.assignments.clone();
                        hosted.backlog.plan = previous.backlog.plan.clone();
                    }
                }
                if let (Some(next), Some(previous)) = (
                    document.hosted.as_mut(),
                    card.document.as_ref().and_then(|d| d.hosted.as_ref()),
                ) && next.service_url == previous.service_url
                {
                    if next.readiness.dispatch_unavailable {
                        next.readiness.dispatch = previous.readiness.dispatch.clone();
                    }
                    if next.readiness.sources_unavailable {
                        next.readiness.sources = previous.readiness.sources.clone();
                    }
                }
                cache.insert(outcome.checkout, document.clone());
                card.document = Some(document);
                card.from_cache = false;
                card.error = card
                    .document
                    .as_ref()
                    .and_then(|doc| doc.hosted.as_ref())
                    .and_then(|hosted| hosted.last_error.as_ref())
                    .map(|error| {
                        FetchError::Failed(format!("pmd last refresh failed at {}: {}", error.at, error.message))
                    });
                card.from_cache = card.error.is_some();
            }
            Err(error) => {
                card.from_cache = card.document.is_some();
                card.error = Some(error);
            }
        }
        true
    }

    pub fn is_refreshing(&self) -> bool {
        self.cards.iter().any(|card| card.refreshing)
    }

    /// A missing or rejected pmd token: the board shows it once instead of on each card.
    pub fn setup_problem(&self) -> Option<&FetchError> {
        self.cards
            .iter()
            .filter_map(|card| card.error.as_ref())
            .find(|error| error.is_setup())
    }

    /// The cards' indices in config, update-time, or priority order. Unknown values sort last in either direction;
    /// ties retain config order.
    pub fn order(&self, sort: SortOrder) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.cards.len()).collect();
        if sort == SortOrder::Config {
            return order;
        }
        if matches!(sort, SortOrder::PriorityFirst | SortOrder::PriorityLast) {
            order.sort_by(
                |&a, &b| match (self.priority(&self.cards[a]), self.priority(&self.cards[b])) {
                    (Some(a), Some(b)) if sort == SortOrder::PriorityFirst => a.cmp(&b),
                    (Some(a), Some(b)) => b.cmp(&a),
                    (a, b) => b.is_some().cmp(&a.is_some()),
                },
            );
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

    pub fn set_priorities(&mut self, priorities: priority::Priorities) {
        self.priorities = priorities;
    }

    pub fn priority(&self, card: &Card) -> Option<u8> {
        self.priorities.for_card(card)
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
    /// P0 first, unknown priorities last; ties keep config order.
    PriorityFirst,
    /// Highest priority number first, unknown priorities last; ties keep config order.
    PriorityLast,
}

impl SortOrder {
    /// The order after a click on the Updated header: newest first, then oldest first, then config order again.
    pub fn next(self) -> Self {
        match self {
            Self::Config | Self::PriorityFirst | Self::PriorityLast => Self::NewestFirst,
            Self::NewestFirst => Self::OldestFirst,
            Self::OldestFirst => Self::Config,
        }
    }

    pub fn next_priority(self) -> Self {
        match self {
            Self::PriorityFirst => Self::PriorityLast,
            Self::PriorityLast => Self::Config,
            _ => Self::PriorityFirst,
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
        hint: "Add local checkouts to the repositories list in the dashboard's config.toml to read their hosted pmd status here.",
    };
}

/// The checked-in `gh x status --json` fixtures, for tests and screenshots.
pub mod fixtures {
    use crate::status::StatusDocument;

    pub const CLEAN: &str = include_str!("../tests/fixtures/status-clean.json");
    pub const ATTENTION: &str = include_str!("../tests/fixtures/status-attention.json");
    pub const UNAVAILABLE: &str = include_str!("../tests/fixtures/status-unavailable.json");
    pub const ENRICHMENT_ERRORS: &str = include_str!("../tests/fixtures/status-enrichment-errors.json");
    pub const ERROR: &str = include_str!("../tests/fixtures/status-error.json");

    pub fn document(json: &str) -> StatusDocument {
        StatusDocument::parse(json).expect("fixtures parse")
    }
}

/// Field-based refresh comparison, excluding polling/provenance timestamps.
pub mod changes;
pub mod readiness;
