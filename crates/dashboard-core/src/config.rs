//! The dashboard's config file: which local checkouts to show and how often to refresh them.
//!
//! ```toml
//! # Folders whose immediate subfolders are scanned for git checkouts on first run.
//! root = "C:/Users/franz/github"
//! refresh_minutes = 5
//! parallelism = 4
//! repositories = ["C:/Users/franz/github/hemsoft/gh-x", "C:/Users/franz/github/hemsoft/codexbar"]
//! # Optional: the T3 Code home (its --base-dir) when it isn't T3CODE_HOME, ~/.t3 or ~/.t3-server.
//! t3_home = "D:/t3"
//! pmd_url = "http://100.97.164.73:8790"
//! pmd_token_path = "pmd-token" # relative to config.toml
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "config.toml";
pub const DEFAULT_REFRESH_MINUTES: u64 = 5;
pub const DEFAULT_PARALLELISM: usize = 4;
/// The longest automatic refresh interval: one day.
pub const MAX_REFRESH_MINUTES: u64 = 24 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The folder scanned for checkouts when the config file is first created.
    pub root: Option<PathBuf>,
    /// Minutes between automatic refreshes (at most a day); 0 turns them off.
    pub refresh_minutes: u64,
    /// How many pmd status reads run at once (1 to 16).
    pub parallelism: usize,
    /// Local checkouts, in display order.
    pub repositories: Vec<PathBuf>,
    /// Where the local T3 Code server keeps its data (its `--base-dir`), for the row's T3 button. Unset, the
    /// dashboard looks in `T3CODE_HOME`, `~/.t3` and `~/.t3-server`.
    pub t3_home: Option<PathBuf>,
    /// Base URL of the project-manager read API.
    pub pmd_url: String,
    /// Bearer-token file. Relative paths resolve beside config.toml.
    pub pmd_token_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: None,
            refresh_minutes: DEFAULT_REFRESH_MINUTES,
            parallelism: DEFAULT_PARALLELISM,
            repositories: Vec::new(),
            t3_home: None,
            pmd_url: crate::priority::DEFAULT_URL.to_owned(),
            pmd_token_path: PathBuf::from("pmd-token"),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Read(PathBuf, std::io::Error),
    Parse(PathBuf, String),
    Write(PathBuf, std::io::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(path, err) => write!(f, "can't read {}: {err}", path.display()),
            Self::Parse(path, err) => write!(f, "{} is not valid: {err}", path.display()),
            Self::Write(path, err) => write!(f, "can't write {}: {err}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The folder for the config file and cache: `%APPDATA%\ProjectDashboard` on Windows,
/// `~/.config/project-dashboard` on Linux, or `PROJECT_DASHBOARD_CONFIG_DIR` when set.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PROJECT_DASHBOARD_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = dirs::config_dir().unwrap_or_default();
    if cfg!(windows) {
        base.join("ProjectDashboard")
    } else {
        base.join("project-dashboard")
    }
}

/// Where first-run discovery looks when the config names no root: `~/github` if it exists, else the home folder.
pub fn default_root() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let github = home.join("github");
    if github.is_dir() { github } else { home }
}

impl Config {
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let mut config: Self =
            toml::from_str(text).map_err(|err| ConfigError::Parse(path.to_owned(), err.to_string()))?;
        config.parallelism = config.parallelism.clamp(1, crate::scheduler::MAX_PARALLELISM);
        config.refresh_minutes = config.refresh_minutes.min(MAX_REFRESH_MINUTES);
        Ok(config)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("a config always serializes")
    }

    /// Loads `dir/config.toml`. On first run, when it doesn't exist, discovers the git checkouts one level under the
    /// root and writes them as the starting list.
    pub fn load_or_create(dir: &Path) -> Result<Self, ConfigError> {
        Self::load_or_create_in(dir, default_root())
    }

    /// [`Config::load_or_create`] with the first-run root given.
    pub fn load_or_create_in(dir: &Path, root: PathBuf) -> Result<Self, ConfigError> {
        let path = dir.join(FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let config = Self {
                    repositories: discover(&root),
                    root: Some(root),
                    ..Self::default()
                };
                config.save(dir)?;
                Ok(config)
            }
            Err(err) => Err(ConfigError::Read(path, err)),
        }
    }

    /// Reads `dir/config.toml` as it is now. Unlike [`Config::load_or_create`], a missing file is an error rather than
    /// a reason to discover checkouts and write a new one: while the app runs, a missing file is a mistake to report.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).map_err(|err| ConfigError::Read(path.clone(), err))?;
        Self::parse(&text, &path)
    }

    pub fn save(&self, dir: &Path) -> Result<(), ConfigError> {
        let path = dir.join(FILE_NAME);
        std::fs::create_dir_all(dir).map_err(|err| ConfigError::Write(path.clone(), err))?;
        std::fs::write(&path, self.to_toml()).map_err(|err| ConfigError::Write(path, err))
    }

    pub fn refresh_interval(&self) -> Option<std::time::Duration> {
        let minutes = self.refresh_minutes.min(MAX_REFRESH_MINUTES);
        (minutes > 0).then(|| std::time::Duration::from_secs(minutes * 60))
    }
}

/// The git checkouts directly under `root`, sorted by path. Only primary checkouts (a `.git` folder) count: a linked
/// worktree (a `.git` file) is already part of its primary checkout's status.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(".git").is_dir())
        .collect();
    found.sort();
    found
}

/// Notices edits to `config.toml` while the app runs (#21) by comparing its bytes with the last applied read. The
/// file is a few hundred bytes, so reading it every couple of seconds is cheap, and unlike a timestamp and size it
/// can't miss a same-length edit. An unchanged file is never parsed again.
///
/// A change is applied only once it reads the same twice, so an editor that saves by truncating the file and then
/// writing it can't have its momentary empty file (valid TOML with no repositories) taken as the new config and empty
/// the table. The poll confirms on its next read; a refresh, which can't wait, reads again after a short pause.
#[derive(Debug)]
pub struct ConfigFile {
    dir: PathBuf,
    /// The bytes last applied (`Some(None)` when the file couldn't be read); `None` before the first apply.
    last: Option<Option<Vec<u8>>>,
    /// A different read waiting for the next poll to confirm it.
    pending: Option<Option<Vec<u8>>>,
}

/// How long a refresh waits before reading a changed file a second time.
pub const CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

impl ConfigFile {
    /// Watches `dir/config.toml`. Nothing counts as applied yet: the file may have changed since the app read it at
    /// startup, so the first stable read is applied too, which changes nothing when the file is as it was.
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            last: None,
            pending: None,
        }
    }

    fn contents(&self) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(FILE_NAME)).ok()
    }

    /// For the periodic poll: the file's new config once a change has read the same on two polls in a row, else
    /// `None`.
    pub fn changed(&mut self) -> Option<Result<Config, ConfigError>> {
        let read = self.contents();
        if self.last.as_ref() == Some(&read) {
            self.pending = None;
            return None;
        }
        if self.pending.as_ref() != Some(&read) {
            self.pending = Some(read);
            return None;
        }
        self.pending = None;
        Some(self.take(read))
    }

    /// For a refresh: the file's new config if it changed and reads the same again after [`CONFIRM_DELAY`], else
    /// `None`. A change still being written is left for the poll to confirm.
    pub fn reload(&mut self) -> Option<Result<Config, ConfigError>> {
        let read = self.contents();
        if self.last.as_ref() == Some(&read) {
            return None;
        }
        std::thread::sleep(CONFIRM_DELAY);
        if self.contents() != read {
            return None;
        }
        self.pending = None;
        Some(self.take(read))
    }

    /// Parses exactly the bytes that were compared and remembers them as applied.
    fn take(&mut self, read: Option<Vec<u8>>) -> Result<Config, ConfigError> {
        let path = self.dir.join(FILE_NAME);
        self.last = Some(read.clone());
        let Some(bytes) = read else {
            // Read once more for the reason; the bytes compared were "unreadable", whatever the reason now.
            let err = std::fs::read(&path)
                .err()
                .unwrap_or_else(|| std::io::Error::other("the file changed while being read"));
            return Err(ConfigError::Read(path, err));
        };
        match std::str::from_utf8(&bytes) {
            Ok(text) => Config::parse(text, &path),
            Err(err) => Err(ConfigError::Parse(path, err.to_string())),
        }
    }
}
