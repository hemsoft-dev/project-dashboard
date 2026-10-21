//! The Project Dashboard data model, kept apart from the GPUI views so it builds and tests without a window.
//!
//! Status data always comes from `gh x status --json` (gh-x is the single source of truth); this crate models what the
//! app shows, it never decides a repository's status itself.

/// The app's name, as the window title and the taskbar show it.
pub const APP_NAME: &str = "Project Dashboard";

/// One configured local checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    /// The folder `gh x status` runs in.
    pub path: std::path::PathBuf,
}

/// Everything the window shows: the configured repositories, in display order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    repositories: Vec<Repository>,
}

impl Board {
    pub fn new(repositories: Vec<Repository>) -> Self {
        Self { repositories }
    }

    pub fn repositories(&self) -> &[Repository] {
        &self.repositories
    }

    pub fn is_empty(&self) -> bool {
        self.repositories.is_empty()
    }

    /// The message an empty board shows instead of cards, or `None` when there is something to show.
    pub fn empty_state(&self) -> Option<EmptyState> {
        self.is_empty().then_some(EmptyState::NO_REPOSITORIES)
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
        hint: "Add local checkouts to the dashboard's config file to see their gh x status here.",
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_board_shows_the_no_repositories_message() {
        let board = Board::default();
        assert!(board.is_empty());
        assert_eq!(board.empty_state(), Some(EmptyState::NO_REPOSITORIES));
    }

    #[test]
    fn board_with_repositories_has_no_empty_state() {
        let board = Board::new(vec![Repository {
            path: "/src/project-dashboard".into(),
        }]);
        assert!(!board.is_empty());
        assert_eq!(board.repositories().len(), 1);
        assert_eq!(board.empty_state(), None);
    }
}
