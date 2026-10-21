//! The last status shown for each repository, so the board renders instantly at startup before any refresh finishes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::status::StatusDocument;

pub const FILE_NAME: &str = "status-cache.json";

/// The cache file's own version; a different one is ignored rather than misread.
const CACHE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCache {
    version: u32,
    /// Keyed by the configured checkout path.
    entries: BTreeMap<PathBuf, StatusDocument>,
}

impl Default for StatusCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl StatusCache {
    pub fn get(&self, path: &Path) -> Option<&StatusDocument> {
        self.entries.get(path)
    }

    pub fn insert(&mut self, path: PathBuf, document: StatusDocument) {
        self.entries.insert(path, document);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Keeps only the configured repositories, so removed ones don't linger in the file.
    pub fn retain(&mut self, paths: &[PathBuf]) {
        self.entries.retain(|path, _| paths.contains(path));
    }

    /// Reads `dir/status-cache.json`. A missing, unreadable or other-version file is an empty cache: the cache only
    /// speeds up startup, and the next refresh rebuilds it.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE_NAME))
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .filter(|cache| cache.version == CACHE_VERSION)
            .unwrap_or_default()
    }

    /// Writes the cache through a temporary file, so a crash mid-write never leaves a torn file behind.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        let path = dir.join(FILE_NAME);
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &path)
    }
}
