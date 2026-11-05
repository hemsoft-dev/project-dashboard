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
//! # Optional: turns on the Manage menu's writes. The file holds a write-scoped token, owner-only.
//! pmd_write_token_path = "pmd-write-token"
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
    pub view: View,
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
    /// The write token's file, separate from the read token. Unset, writes are off. Relative paths resolve beside
    /// config.toml. The token itself never goes in this file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pmd_write_token_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    #[default]
    Project,
    Backlog,
    Fleet,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            view: View::Project,
            root: None,
            refresh_minutes: DEFAULT_REFRESH_MINUTES,
            parallelism: DEFAULT_PARALLELISM,
            repositories: Vec::new(),
            t3_home: None,
            pmd_url: crate::priority::DEFAULT_URL.to_owned(),
            pmd_token_path: PathBuf::from("pmd-token"),
            pmd_write_token_path: None,
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
    pub fn save_view(dir: &Path, view: View) -> Result<(), ConfigError> {
        let path = dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).map_err(|e| ConfigError::Read(path.clone(), e))?;
        let mut expected = Self::parse(&text, &path)?;
        expected.view = view;
        let name = match view {
            View::Project => "project",
            View::Backlog => "backlog",
            View::Fleet => "fleet",
        };
        // Use TOML value spans so quoted keys, multiline strings and inline comments survive.
        let values: std::collections::BTreeMap<String, toml::Spanned<toml::Value>> =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(path.clone(), e.to_string()))?;
        let mut edited = text.clone();
        let value = format!("\"{name}\"");
        if let Some(old) = values.get("view") {
            edited.replace_range(old.span(), &value);
        } else {
            let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
            edited.insert_str(0, &format!("view = {value}{newline}"));
        }
        if Self::parse(&edited, &path)? != expected {
            return Err(ConfigError::Parse(path, "view could not be saved safely".into()));
        }
        replace_config(&path, edited.as_bytes())
    }

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

    /// Adds `checkout` to `repositories` in `dir/config.toml`, keeping every other byte of the file, comments
    /// included: the path goes into the existing array as text. The edited file must read back as the old config plus
    /// that one path, or nothing is written. Returns `false` when the checkout is listed already.
    pub fn add_repository(dir: &Path, checkout: &Path) -> Result<bool, ConfigError> {
        let path = dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).map_err(|err| ConfigError::Read(path.clone(), err))?;
        let config = Self::parse(&text, &path)?;
        if config.repositories.iter().any(|known| known == checkout) {
            return Ok(false);
        }
        let mut expected = config.clone();
        expected.repositories.push(checkout.to_path_buf());
        let edited = insert_repository(&text, &checkout.to_string_lossy())
            .filter(|edited| Self::parse(edited, &path).is_ok_and(|parsed| parsed == expected))
            .ok_or_else(|| {
                ConfigError::Parse(
                    path.clone(),
                    "its repositories list couldn't be edited safely; add the folder to it by hand".to_owned(),
                )
            })?;
        replace_config(&path, edited.as_bytes())?;
        Ok(true)
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
        replace_config(&path, self.to_toml().as_bytes())
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

/// `text` with `entry` added to its top-level `repositories` array, or a new `repositories` line at the top when there
/// is none. Strings and comments inside the array are skipped while looking for its closing bracket. `None` when the
/// array can't be found.
fn insert_repository(text: &str, entry: &str) -> Option<String> {
    let value = toml::Value::String(entry.to_owned()).to_string();
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            // A table: top-level keys end here.
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("repositories")
            && let Some(rest) = rest.trim_start().strip_prefix('=')
            && rest.trim_start().starts_with('[')
        {
            let bracket = line.len() - rest.trim_start().len();
            start = Some(offset + bracket);
            break;
        }
        offset += line.len();
    }
    let Some(open) = start else {
        return Some(format!("repositories = [{value}]\n{text}"));
    };
    // Find the closing bracket, skipping strings and comments, and whether anything is listed.
    let bytes = text.as_bytes();
    let mut i = open + 1;
    let mut last_value_end = None;
    while i < bytes.len() {
        match bytes[i] {
            b']' => break,
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            quote @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if quote == b'"' && bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                last_value_end = Some(i + 1);
            }
            b',' => last_value_end = None,
            _ => {}
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let close = i;
    let inner = &text[open + 1..close];
    let multiline = inner.contains('\n');
    // A new line in a multi-line list takes the indent of the list's last entry.
    let indent = inner
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map_or("  ", |line| &line[..line.len() - line.trim_start().len()]);
    let mut edited = String::with_capacity(text.len() + value.len() + 8);
    match last_value_end {
        // The last entry has no trailing comma: add one right after it.
        Some(end) => {
            edited.push_str(&text[..end]);
            edited.push(',');
            if multiline {
                edited.push_str(&text[end..close]);
                let before = edited.trim_end_matches([' ', '\t']).len();
                edited.truncate(before);
                edited.push_str(&format!("{indent}{value},\n"));
            } else {
                edited.push_str(&format!(" {value}"));
                edited.push_str(&text[end..close]);
            }
        }
        None => {
            edited.push_str(&text[..close]);
            let empty = inner.trim().is_empty();
            if multiline {
                let before = edited.trim_end_matches([' ', '\t']).len();
                edited.truncate(before);
                edited.push_str(&format!("{indent}{value},\n"));
            } else if empty {
                edited.push_str(&value);
            } else {
                edited.push_str(&format!(" {value}"));
            }
        }
    }
    edited.push_str(&text[close..]);
    Some(edited)
}

#[cfg(test)]
mod view_tests {
    use super::*;
    #[test]
    fn view_defaults_to_project_and_round_trips() {
        assert_eq!(Config::parse("", Path::new("config.toml")).unwrap().view, View::Project);
        let config = Config {
            view: View::Backlog,
            ..Default::default()
        };
        assert_eq!(
            Config::parse(&config.to_toml(), Path::new("config.toml")).unwrap().view,
            View::Backlog
        );
    }
}

/// Linux and Windows write beside the destination, then close the handle before atomic replacement.
/// Other Unix targets retain the legacy in-place write for compatibility: they still have the known
/// partial-write risk. Native macOS/BSD ACL preservation is outside this Windows/Linux maintenance fix.
fn replace_config(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        use std::io::Write as _;
        replace_config_with(path, bytes, |file, bytes| file.write_all(bytes))
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        write_config_in_place(path, bytes)
    }
}

#[cfg(any(test, all(unix, not(target_os = "linux"))))]
fn write_config_in_place(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    std::fs::write(path, bytes).map_err(|err| ConfigError::Write(path.to_owned(), err))
}

// Never remove the destination first: failed metadata application or rename leaves its bytes intact.
#[cfg(any(windows, target_os = "linux"))]
fn replace_config_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<(), ConfigError> {
    let security = ConfigSecurity::read(path).map_err(|err| ConfigError::Write(path.to_owned(), err))?;
    replace_config_using(&security, path, bytes, write)
}

#[cfg(any(windows, target_os = "linux"))]
fn replace_config_using(
    security: &ConfigSecurity,
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<(), ConfigError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or(Path::new("."));
    let (temporary, mut file) = loop {
        let temporary = parent.join(format!(
            ".config.toml.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match security.create(&temporary) {
            Ok(file) => break (temporary, file),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(ConfigError::Write(path.to_owned(), err)),
        }
    };
    let result = security
        .apply(&file)
        .and_then(|()| write(&mut file, bytes))
        .and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|err| ConfigError::Write(path.to_owned(), err))
}

#[cfg(all(test, any(windows, target_os = "linux")))]
#[path = "config_atomic_73.rs"]
mod config_atomic_73;

#[cfg(target_os = "linux")]
struct ConfigSecurity {
    metadata: Option<std::fs::Metadata>,
    acl: Option<Vec<u8>>,
}

#[cfg(target_os = "linux")]
impl ConfigSecurity {
    fn read(path: &Path) -> std::io::Result<Self> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(Self {
                metadata: Some(metadata),
                acl: read_config_acl(path)?,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                metadata: None,
                acl: None,
            }),
            Err(err) => Err(err),
        }
    }

    fn create(&self, path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    fn apply(&self, file: &std::fs::File) -> std::io::Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        if let Some(old) = &self.metadata {
            let new = file.metadata()?;
            if (old.uid(), old.gid()) != (new.uid(), new.gid()) {
                std::os::unix::fs::fchown(file, Some(old.uid()), Some(old.gid()))?;
            }
        }
        apply_config_acl(file, None)?;
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(self.metadata.as_ref().map_or_else(
            || std::fs::Permissions::from_mode(0o600),
            std::fs::Metadata::permissions,
        ))?;
        if self.acl.is_some() {
            apply_config_acl(file, self.acl.as_deref())?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
const CONFIG_ACL: &str = "system.posix_acl_access";

#[cfg(target_os = "linux")]
fn read_config_acl(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match rustix::fs::getxattr(path, CONFIG_ACL, &mut [] as &mut [u8]) {
        Ok(size) => {
            let mut acl = vec![0; size];
            let read = rustix::fs::getxattr(path, CONFIG_ACL, acl.as_mut_slice())?;
            acl.truncate(read);
            Ok(Some(acl))
        }
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

#[cfg(target_os = "linux")]
fn apply_config_acl(file: &std::fs::File, acl: Option<&[u8]>) -> std::io::Result<()> {
    if let Some(acl) = acl {
        rustix::fs::fsetxattr(file, CONFIG_ACL, acl, rustix::fs::XattrFlags::empty())?;
    } else {
        // A default ACL on the parent can be inherited even by a mode-600 temporary.
        match rustix::fs::fremovexattr(file, CONFIG_ACL) {
            Ok(()) | Err(rustix::io::Errno::NODATA | rustix::io::Errno::NOTSUP) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

#[cfg(windows)]
use config_security_windows::ConfigSecurity;

#[cfg(windows)]
mod config_security_windows {
    use std::os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_INVALID_OWNER, ERROR_SUCCESS, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorGroup,
        InitializeSecurityDescriptor, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
        SetSecurityDescriptorGroup,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_WRITE_DATA, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
    };

    /// `SECURITY_DESCRIPTOR_REVISION`, which windows-sys keeps behind a feature nothing else here needs.
    const DESCRIPTOR_REVISION: u32 = 1;

    // Own the snapshot until CreateFileW has copied it. Never create a readable temporary and tighten it later.
    pub(super) struct ConfigSecurity {
        descriptor: PSECURITY_DESCRIPTOR,
        destination: Vec<u16>,
    }

    impl ConfigSecurity {
        pub(super) fn read(path: &Path) -> std::io::Result<Self> {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut descriptor = null_mut();
            // SAFETY: path is terminated, all unused output pointers are null. On success LocalFree owns the result.
            let status = unsafe {
                GetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    &mut descriptor,
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                let sddl: Vec<u16> = super::super::write::OWNER_ONLY_SDDL
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                // SAFETY: terminated SDDL, valid output pointer. The descriptor is released by Drop.
                if unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        sddl.as_ptr(),
                        SDDL_REVISION_1,
                        &mut descriptor,
                        null_mut(),
                    )
                } == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            } else if status != ERROR_SUCCESS {
                return Err(std::io::Error::from_raw_os_error(status as i32));
            }
            Self::protected(descriptor, path)
        }

        fn protected(descriptor: PSECURITY_DESCRIPTOR, destination: Vec<u16>) -> std::io::Result<Self> {
            let security = Self {
                descriptor,
                destination,
            };
            // Freeze the existing access rules: inheriting new parent ACEs could broaden access on replacement.
            // SAFETY: descriptor was returned by the security API and remains owned by security.
            if unsafe { SetSecurityDescriptorControl(security.descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(security)
        }

        /// A snapshot as [`ConfigSecurity::read`] would take it, for an owner a test can't give a real file. Takes
        /// ownership of a descriptor allocated by the security API.
        #[cfg(test)]
        pub(super) fn from_descriptor(descriptor: PSECURITY_DESCRIPTOR, destination: &Path) -> Self {
            let destination = destination.as_os_str().encode_wide().chain(Some(0)).collect();
            Self::protected(descriptor, destination).unwrap()
        }

        /// Windows refuses to create a file with an owner the token can't assign (1307), as with an
        /// Administrators-owned config under UAC's filtered token. The temporary then keeps the group and DACL and takes
        /// the token's default owner, but only for a caller already able to rewrite the destination's bytes and DACL,
        /// which is what owning it would add. Anyone else still gets the 1307.
        pub(super) fn create(&self, path: &Path) -> std::io::Result<std::fs::File> {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            match create_temporary(&path, self.descriptor) {
                Err(err) if err.raw_os_error() == Some(ERROR_INVALID_OWNER as i32) && self.caller_can_rewrite() => {
                    self.create_with_default_owner(&path)
                }
                result => result,
            }
        }

        /// Whether the destination opens for writing its bytes and its DACL. Opening changes nothing.
        fn caller_can_rewrite(&self) -> bool {
            // SAFETY: the destination is terminated and no security attributes or template are passed.
            let handle = unsafe {
                CreateFileW(
                    self.destination.as_ptr(),
                    FILE_WRITE_DATA | READ_CONTROL | WRITE_DAC,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return false;
            }
            // SAFETY: the valid handle has no other owner. File closes it.
            drop(unsafe { std::fs::File::from_raw_handle(handle as _) });
            true
        }

        fn create_with_default_owner(&self, path: &[u16]) -> std::io::Result<std::fs::File> {
            // SAFETY: all-zero is a valid SECURITY_DESCRIPTOR, and InitializeSecurityDescriptor fills it in below.
            let mut storage: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
            let descriptor: PSECURITY_DESCRIPTOR = (&raw mut storage).cast();
            let (mut group, mut group_defaulted) = (null_mut(), 0);
            let (mut dacl_present, mut dacl, mut dacl_defaulted) = (0, null_mut(), 0);
            // SAFETY: the outputs are valid. The new descriptor only points into self.descriptor, which outlives it.
            let built = unsafe {
                InitializeSecurityDescriptor(descriptor, DESCRIPTOR_REVISION) != 0
                    && GetSecurityDescriptorGroup(self.descriptor, &mut group, &mut group_defaulted) != 0
                    && SetSecurityDescriptorGroup(descriptor, group, group_defaulted) != 0
                    && GetSecurityDescriptorDacl(self.descriptor, &mut dacl_present, &mut dacl, &mut dacl_defaulted)
                        != 0
                    && SetSecurityDescriptorDacl(descriptor, dacl_present, dacl, dacl_defaulted) != 0
                    && SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED) != 0
            };
            if !built {
                return Err(std::io::Error::last_os_error());
            }
            create_temporary(path, descriptor)
        }

        pub(super) fn apply(&self, _file: &std::fs::File) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn create_temporary(path: &[u16], descriptor: PSECURITY_DESCRIPTOR) -> std::io::Result<std::fs::File> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: path and security descriptor remain valid for the call. CREATE_NEW never opens another file.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error())
        } else {
            // SAFETY: the valid handle has no other owner. File closes it.
            Ok(unsafe { std::fs::File::from_raw_handle(handle as _) })
        }
    }

    impl Drop for ConfigSecurity {
        fn drop(&mut self) {
            // SAFETY: allocated by GetNamedSecurityInfoW or ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe { LocalFree(self.descriptor as _) };
        }
    }
}
