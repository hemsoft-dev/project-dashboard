//! Project priorities come only from pmd, independently of gh-x status and its refresh workers.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::{Card, config::Config};

pub const DEFAULT_URL: &str = "http://100.97.164.73:8790";
pub const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESPONSE: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Priorities(HashMap<String, u8>);

#[derive(Deserialize)]
struct Projects {
    projects: Vec<Project>,
}

#[derive(Deserialize)]
struct Project {
    slug: String,
    priority: Option<u8>,
    tracked: bool,
}

impl Priorities {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let projects: Projects = serde_json::from_slice(bytes).ok()?;
        Some(Self(
            projects
                .projects
                .into_iter()
                .filter_map(|project| {
                    let priority = project.priority.filter(|priority| *priority <= 5)?;
                    project.tracked.then(|| (project.slug.to_ascii_lowercase(), priority))
                })
                .collect(),
        ))
    }

    pub fn for_card(&self, card: &Card) -> Option<u8> {
        // gh-x identifies renamed checkouts correctly; before status arrives use the checkout folder name.
        let name = card.name();
        self.0.get(&name.rsplit('/').next()?.to_ascii_lowercase()).copied()
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
        let mut token = String::new();
        std::fs::File::open(&self.token_path)
            .ok()?
            .take(8193)
            .read_to_string(&mut token)
            .ok()?;
        let token = token.trim();
        if token.is_empty() || token.len() > 8192 {
            return None;
        }
        // No redirects or system proxies: the read token goes only to the configured pmd endpoint.
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .ok()?;
        let response = client
            .get(format!("{}/v1/projects", self.url.trim_end_matches('/')))
            .bearer_auth(token)
            .send()
            .ok()?
            .error_for_status()
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let mut bytes = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return None;
        }
        Priorities::parse(&bytes)
    }
}

impl PrioritySource for Pmd {
    fn fetch(&self) -> Priorities {
        self.read().unwrap_or_default()
    }
}
