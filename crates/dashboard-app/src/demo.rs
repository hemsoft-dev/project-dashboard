//! Demo data from the checked-in `gh x status --json` fixtures, for `--demo`, the UI tests and screenshots. Nothing
//! here runs gh.

use std::path::{Path, PathBuf};

use dashboard_core::fixtures::{self, ATTENTION, CLEAN, UNAVAILABLE};
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;

/// Four checkouts: clean, needing attention, partly unavailable, and one whose gh x status fails.
pub fn checkouts() -> Vec<PathBuf> {
    ["gh-x", "codexbar", "minibot", "weather-cli"]
        .into_iter()
        .map(|name| PathBuf::from("/demo").join(name))
        .collect()
}

pub struct DemoSource;

impl StatusSource for DemoSource {
    fn fetch(&self, checkout: &Path, _refresh: bool) -> Result<StatusDocument, FetchError> {
        let name = checkout.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        match name {
            "gh-x" => Ok(fixtures::document(CLEAN)),
            "codexbar" => Ok(fixtures::document(ATTENTION)),
            "minibot" => Ok(fixtures::document(UNAVAILABLE)),
            _ => Err(FetchError::Failed(
                "HTTP 401: Bad credentials (https://api.github.com/graphql). Try gh auth login.".into(),
            )),
        }
    }
}
