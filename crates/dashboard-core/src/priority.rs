//! Project priorities come from pmd, independently of status refresh workers.

use crate::pmd::{Projects, ReadApi};
use crate::{Card, config::Config};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://100.97.164.73:8790";
pub const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Priorities(HashMap<String, u8>);
impl Priorities {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let projects: Projects = serde_json::from_slice(bytes).ok()?;
        let mut priorities = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        for project in projects.projects.into_iter().filter(|p| p.tracked) {
            let identity = project.repo.to_ascii_lowercase();
            if !seen.insert(identity.clone()) {
                priorities.remove(&identity);
            } else if let Some(priority) = project.priority.filter(|p| *p <= 5) {
                priorities.insert(identity, priority);
            }
        }
        Some(Self(priorities))
    }
    pub fn for_card(&self, card: &Card) -> Option<u8> {
        let identity = match &card.identity {
            Some(Ok(identity)) => identity.clone(),
            Some(Err(_)) => return None,
            None => card.document.as_ref()?.repository.name_with_owner.clone(),
        };
        self.0.get(&identity.to_ascii_lowercase()).copied()
    }
}

pub trait PrioritySource: Send + Sync + 'static {
    /// Failure means unknown priorities. Never return or log a credential or an HTTP error containing it.
    fn fetch(&self) -> Priorities;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pmd {
    pub url: String,
    pub token_path: PathBuf,
}

impl Pmd {
    pub fn from_config(config: &Config, dir: &Path) -> Self {
        Self {
            url: config.pmd_url.clone(),
            token_path: dir.join(&config.pmd_token_path),
        }
    }

    fn read(&self) -> Option<Priorities> {
        let api = ReadApi::new(self, TIMEOUT).ok()?;
        let value: serde_json::Value = api.get("v1/projects", &[]).ok()?;
        Priorities::parse(&serde_json::to_vec(&value).ok()?)
    }
}

impl PrioritySource for Pmd {
    fn fetch(&self) -> Priorities {
        self.read().unwrap_or_default()
    }
}
