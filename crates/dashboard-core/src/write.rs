//! Writes to pmd for the Manage menu's dialogs (#33): the write token, `GET /v1/whoami`, and a bounded write client.
//!
//! Writes use their own token, kept apart from the read token and from `config.toml`, `ui-state.json` and the status
//! cache. Unset `pmd_write_token_path` means writes are off. Like the read client, the write client sends no redirect
//! or proxy, bounds the body and the response, has a timeout, and never puts a token, raw body, URL or reqwest
//! diagnostic in an error. It never retries a write on its own: after a create times out it reads the row, so the
//! dialog can say whether the create landed before it offers a retry ([pmd README](
//! https://github.com/hemsoft-dev/project-manager#fleet-config)).
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::config::Config;

/// The longest token accepted, as for the read token.
const MAX_TOKEN: usize = 8192;
/// pmd refuses a body over 64 KB with `413 too_large`; a bigger body isn't sent at all.
pub const MAX_BODY: usize = 64 * 1024;
const MAX_RESPONSE: u64 = 1024 * 1024;
/// A project create checks the repository with GitHub for up to 30 s before pmd answers, so writes wait longer than
/// reads.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(40);
/// pmd's messages are one line; anything longer is cut, so a dialog never shows a wall of text.
const MAX_MESSAGE: usize = 500;

/// The management dialogs behind the Manage menu, and the scope each needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Projects,
    WorkItems,
    Fleet,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Projects, Feature::WorkItems, Feature::Fleet];

    /// The scope pmd requires for this dialog's writes.
    pub fn scope(self) -> &'static str {
        match self {
            Self::Projects => "write:projects",
            Self::WorkItems => "write:work",
            Self::Fleet => "write:fleet",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Projects => "Projects",
            Self::WorkItems => "Work items",
            Self::Fleet => "Fleet",
        }
    }

    /// The project-dashboard issue that builds this dialog.
    pub fn issue(self) -> u32 {
        match self {
            Self::Projects => 34,
            Self::WorkItems => 35,
            Self::Fleet => 36,
        }
    }
}

/// `GET /v1/whoami`: the calling token's name and scopes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Whoami {
    pub name: String,
    pub scopes: Vec<String>,
}

impl Whoami {
    /// `admin` implies every scope; no other scope implies another.
    pub fn has(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope || s == "admin")
    }

    pub fn allows(&self, feature: Feature) -> bool {
        self.has(feature.scope())
    }
}

/// What the dashboard knows about write access right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// `pmd_write_token_path` is unset.
    Off,
    /// Set, but not checked with pmd yet.
    Unchecked,
    Checking,
    Known(Whoami),
    Failed(WriteError),
}

/// Why the write token can't write for `feature` right now, in lower case, or `None` when it can.
fn token_reason(feature: Feature, access: &Access) -> Option<String> {
    match access {
        Access::Off => Some("writes are off: set pmd_write_token_path".to_owned()),
        Access::Unchecked | Access::Checking => Some("checking write access…".to_owned()),
        Access::Failed(WriteError::MissingToken) => Some("no write token yet: add one in Write access…".to_owned()),
        Access::Failed(_) => Some("write access unknown: see Write access…".to_owned()),
        Access::Known(whoami) if !whoami.allows(feature) => Some(format!("the token lacks {}", feature.scope())),
        Access::Known(_) => None,
    }
}

/// Sentence case for a line under a menu entry or at the top of a dialog.
fn sentence(reasons: Vec<String>) -> Result<(), String> {
    if reasons.is_empty() {
        return Ok(());
    }
    let mut reason = reasons.join(" · ");
    if let Some(first) = reason.get(..1) {
        reason = first.to_uppercase() + &reason[1..];
    }
    Err(reason)
}

/// Whether a Manage entry can open, or the one-line reason it can't. A built dialog always opens: it reads with the
/// read token, and without write access it is read-only ([`write_state`] says why).
pub fn entry_state(feature: Feature, built: bool, access: &Access) -> Result<(), String> {
    if built {
        return Ok(());
    }
    sentence(
        std::iter::once(format!("not built yet (#{})", feature.issue()))
            .chain(token_reason(feature, access))
            .collect(),
    )
}

/// Whether a dialog can write, or the one-line reason it is read-only, such as the scope the token lacks.
pub fn write_state(feature: Feature, access: &Access) -> Result<(), String> {
    sentence(token_reason(feature, access).into_iter().collect())
}

/// Whether a create already landed when its answer timed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateCheck {
    /// The row exists now: the create probably landed. Reload before doing anything else.
    Exists,
    /// The row is still absent: nothing was created, so saving again is safe.
    Absent,
    /// The check failed too.
    Unknown,
}

/// A write's failure, typed so a dialog can show it. No variant carries a token: pmd's messages are scrubbed of both
/// tokens before they are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// `pmd_write_token_path` is unset.
    WritesOff,
    /// The write token file is missing, unreadable or not a token.
    MissingToken,
    /// The read token file is missing, unreadable or not a token, so nothing can be read.
    MissingReadToken,
    /// `400` with pmd's code (`bad_request`, `unknown_repo`, `unknown_machine`, …) and message.
    BadRequest {
        code: String,
        message: String,
    },
    /// `401`: pmd doesn't know the token.
    Unauthorized,
    /// `403 insufficient_scope`, with pmd's message naming the missing scope.
    InsufficientScope {
        message: String,
    },
    NotFound,
    /// `409 stale_version`: the row changed since it was read.
    StaleVersion,
    /// `409 already_exists`.
    AlreadyExists,
    /// `409 item_closed`: a status for a closed or merged item.
    ItemClosed,
    /// Any other `409`, such as `not_tracked`.
    Conflict {
        code: String,
        message: String,
    },
    /// `413 too_large`, or a body over [`MAX_BODY`] that was never sent.
    TooLarge,
    /// `428 precondition_required`.
    PreconditionRequired,
    /// `429 rate_limited`, with `Retry-After` in seconds when pmd sent it.
    RateLimited {
        retry_after: Option<u64>,
    },
    /// A `5xx` with pmd's code and message, such as `503 github_unavailable` or `504 github_timeout`.
    Server {
        status: u16,
        code: String,
        message: String,
    },
    /// No answer in time. For a create, whether the row exists now.
    Timeout {
        create: Option<CreateCheck>,
    },
    /// pmd couldn't be reached.
    Network,
    /// pmd answered with something that isn't its JSON.
    Malformed,
    /// Any other status.
    Http(u16),
    /// Storing the token failed.
    Storage(String),
}

impl WriteError {
    /// The form field a `400` names, when its message starts with one of `fields` (pmd writes `maxSessions must be
    /// 0-64`, `tailnetIp "x" is not…`, `roles: …`). Otherwise the error belongs to the whole dialog.
    pub fn field<'a>(&self, fields: &[&'a str]) -> Option<&'a str> {
        let Self::BadRequest { message, .. } = self else {
            return None;
        };
        let first = message.split_whitespace().next()?.trim_end_matches(':');
        fields.iter().copied().find(|field| first.eq_ignore_ascii_case(field))
    }

    /// pmd's own message, for the errors that carry one.
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::BadRequest { message, .. }
            | Self::InsufficientScope { message }
            | Self::Conflict { message, .. }
            | Self::Server { message, .. } => Some(message),
            _ => None,
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WritesOff => f.write_str("Writes are off. Set pmd_write_token_path in config.toml."),
            Self::MissingToken => f.write_str("The pmd write token is missing, unreadable or invalid."),
            Self::MissingReadToken => f.write_str("The pmd read token is missing, unreadable or invalid."),
            Self::BadRequest { message, .. } => f.write_str(message),
            Self::Unauthorized => f.write_str("pmd rejected the write token (HTTP 401)."),
            Self::InsufficientScope { message } => write!(f, "Not allowed: {message}."),
            Self::NotFound => f.write_str("Not found: the row no longer exists in pmd."),
            Self::StaleVersion => f.write_str("Changed elsewhere since this dialog read it."),
            Self::AlreadyExists => f.write_str("Already exists in pmd."),
            Self::ItemClosed => f.write_str("The item is closed or merged; its status comes from GitHub now."),
            Self::Conflict { message, .. } => f.write_str(message),
            Self::TooLarge => f.write_str("Too large: pmd accepts at most 64 KB."),
            Self::PreconditionRequired => f.write_str("pmd needs a version for this write (HTTP 428)."),
            Self::RateLimited {
                retry_after: Some(seconds),
            } => {
                write!(f, "pmd is busy; try again in {seconds} s.")
            }
            Self::RateLimited { retry_after: None } => f.write_str("pmd is busy; try again shortly."),
            Self::Server { status, message, .. } => write!(f, "pmd couldn't do it (HTTP {status}): {message}"),
            Self::Timeout { create: None } => f.write_str("pmd didn't answer in time; the write may have landed."),
            Self::Timeout {
                create: Some(CreateCheck::Exists),
            } => f.write_str("pmd didn't answer in time, but the row exists now. Reload it before trying again."),
            Self::Timeout {
                create: Some(CreateCheck::Absent),
            } => f.write_str("pmd didn't answer in time and nothing was created. Save again to retry."),
            Self::Timeout {
                create: Some(CreateCheck::Unknown),
            } => f.write_str("pmd didn't answer in time, and checking for the row failed. Reload before retrying."),
            Self::Network => f.write_str("pmd is unavailable."),
            Self::Malformed => f.write_str("pmd returned malformed data."),
            Self::Http(code) => write!(f, "pmd returned HTTP {code}."),
            Self::Storage(reason) => write!(f, "Can't store the token: {reason}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// How a write is guarded against changes made elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precondition {
    /// Replace the row only if it is still at this version.
    IfMatch(u64),
    /// Create the row only if it doesn't exist.
    IfNoneMatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Put,
    Patch,
}

/// One write: `PUT` or `PATCH` of a JSON body at an API path such as `v1/work/01J…`, guarded by a precondition.
#[derive(Clone, Debug, PartialEq)]
pub struct WriteRequest {
    pub method: Method,
    pub path: String,
    pub precondition: Precondition,
    pub body: Value,
}

/// Everything the dialogs need from pmd: write access, writes, and reads of single rows for the conflict flow. The
/// live implementation is [`PmdWriter`]; tests and the demo supply their own.
pub trait WriteBackend: Send + Sync {
    /// The write token's file, or `None` when writes are off.
    fn token_path(&self) -> Option<PathBuf>;
    /// The write token's name and scopes.
    fn whoami(&self) -> Result<Whoami, WriteError>;
    /// Stores a pasted token at [`Self::token_path`], owner-only.
    fn store_token(&self, token: &str) -> Result<(), WriteError>;
    /// Reads one row with the read token, as the conflict flow does after `409 stale_version`.
    fn read(&self, path: &str) -> Result<Value, WriteError>;
    /// Reads a listing with the read token and a query, such as `v1/work` with filters and a cursor.
    fn list(&self, path: &str, query: &[(String, String)]) -> Result<Value, WriteError> {
        if query.is_empty() {
            self.read(path)
        } else {
            Err(WriteError::Malformed)
        }
    }
    /// Sends one write with the write token. Never retried automatically.
    fn write(&self, request: &WriteRequest) -> Result<Value, WriteError>;
}

/// No writes at all: `pmd_write_token_path` unset, for the demo and tests.
pub struct Off;

impl WriteBackend for Off {
    fn token_path(&self) -> Option<PathBuf> {
        None
    }
    fn whoami(&self) -> Result<Whoami, WriteError> {
        Err(WriteError::WritesOff)
    }
    fn store_token(&self, _: &str) -> Result<(), WriteError> {
        Err(WriteError::WritesOff)
    }
    fn read(&self, _: &str) -> Result<Value, WriteError> {
        Err(WriteError::WritesOff)
    }
    fn write(&self, _: &WriteRequest) -> Result<Value, WriteError> {
        Err(WriteError::WritesOff)
    }
}

/// The live backend: pmd's URL and both token files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PmdWriter {
    pub url: String,
    pub read_token_path: PathBuf,
    /// `None` when writes are off.
    pub write_token_path: Option<PathBuf>,
    pub timeout: Duration,
}

impl PmdWriter {
    /// Relative token paths resolve beside config.toml, in `dir`.
    pub fn from_config(config: &Config, dir: &Path) -> Self {
        Self {
            url: config.pmd_url.clone(),
            read_token_path: dir.join(&config.pmd_token_path),
            write_token_path: config.pmd_write_token_path.as_ref().map(|path| dir.join(path)),
            timeout: WRITE_TIMEOUT,
        }
    }

    fn client(&self) -> Result<Client, WriteError> {
        let path = self.write_token_path.as_deref().ok_or(WriteError::WritesOff)?;
        let write = load_token(path)?;
        // The read token is only needed for re-reads; a missing one just means it can't be scrubbed or used.
        let read = load_token(&self.read_token_path).ok();
        Client::new(&self.url, Some(write), read, self.timeout)
    }

    /// Reads need only the read token, so a dialog opens read-only when writes are off. A write token, when there is
    /// one, is still scrubbed from anything pmd says.
    fn reader(&self) -> Result<Client, WriteError> {
        let read = load_token(&self.read_token_path).map_err(|_| WriteError::MissingReadToken)?;
        let write = self.write_token_path.as_deref().and_then(|path| load_token(path).ok());
        Client::new(&self.url, write, Some(read), self.timeout)
    }
}

impl WriteBackend for PmdWriter {
    fn token_path(&self) -> Option<PathBuf> {
        self.write_token_path.clone()
    }

    fn whoami(&self) -> Result<Whoami, WriteError> {
        let client = self.client()?;
        let value = client.send(reqwest::Method::GET, "v1/whoami", &[], None, None, Auth::Write)?;
        serde_json::from_value(value).map_err(|_| WriteError::Malformed)
    }

    fn store_token(&self, token: &str) -> Result<(), WriteError> {
        let path = self.write_token_path.as_deref().ok_or(WriteError::WritesOff)?;
        store_token(path, token)
    }

    fn read(&self, path: &str) -> Result<Value, WriteError> {
        self.reader()?
            .send(reqwest::Method::GET, path, &[], None, None, Auth::Read)
    }

    fn list(&self, path: &str, query: &[(String, String)]) -> Result<Value, WriteError> {
        self.reader()?
            .send(reqwest::Method::GET, path, query, None, None, Auth::Read)
    }

    fn write(&self, request: &WriteRequest) -> Result<Value, WriteError> {
        let client = self.client()?;
        let method = match request.method {
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
        };
        let result = client.send(
            method,
            &request.path,
            &[],
            Some(request.precondition),
            Some(&request.body),
            Auth::Write,
        );
        match result {
            // A create that timed out may still have landed: look before the dialog offers a retry.
            Err(WriteError::Timeout { .. }) if request.precondition == Precondition::IfNoneMatch => {
                let create = match client.send(reqwest::Method::GET, &request.path, &[], None, None, Auth::Read) {
                    Ok(_) => CreateCheck::Exists,
                    Err(WriteError::NotFound) => CreateCheck::Absent,
                    Err(_) => CreateCheck::Unknown,
                };
                Err(WriteError::Timeout { create: Some(create) })
            }
            other => other,
        }
    }
}

#[derive(Clone, Copy)]
enum Auth {
    Read,
    Write,
}

/// One configured HTTP client with the tokens it has. Deliberately not `Debug`, so a token can't be printed by
/// accident.
struct Client {
    client: reqwest::blocking::Client,
    base: reqwest::Url,
    write: Option<String>,
    read: Option<String>,
}

impl Client {
    fn new(url: &str, write: Option<String>, read: Option<String>, timeout: Duration) -> Result<Self, WriteError> {
        let base = reqwest::Url::parse(&format!("{}/", url.trim_end_matches('/'))).map_err(|_| WriteError::Network)?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(WriteError::Network);
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(5)))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| WriteError::Network)?;
        Ok(Self {
            client,
            base,
            write,
            read,
        })
    }

    /// Replaces either token wherever pmd echoed it, and keeps a message to one bounded line.
    fn scrub(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for token in [&self.write, &self.read].into_iter().flatten() {
            text = text.replace(token.as_str(), "[redacted]");
        }
        let mut line: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_MESSAGE)
            .collect();
        line.truncate(line.trim_end().len());
        line
    }

    fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(String, String)],
        precondition: Option<Precondition>,
        body: Option<&Value>,
        auth: Auth,
    ) -> Result<Value, WriteError> {
        let token = match auth {
            Auth::Write => self.write.as_ref().ok_or(WriteError::MissingToken)?,
            Auth::Read => self.read.as_ref().ok_or(WriteError::MissingToken)?,
        };
        // Paths come from the dialogs; one that escapes the API or carries its own query is a bug, not a request.
        // A query goes in `query`, encoded here.
        if path.starts_with('/') || path.contains("..") || path.contains(['?', '#']) || !path.starts_with("v1/") {
            return Err(WriteError::Malformed);
        }
        let mut url = self.base.join(path).map_err(|_| WriteError::Malformed)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let mut request = self.client.request(method, url).bearer_auth(token);
        match precondition {
            Some(Precondition::IfMatch(version)) => request = request.header("If-Match", version.to_string()),
            Some(Precondition::IfNoneMatch) => request = request.header("If-None-Match", "*"),
            None => {}
        }
        if let Some(body) = body {
            let bytes = serde_json::to_vec(body).map_err(|_| WriteError::Malformed)?;
            if bytes.len() > MAX_BODY {
                return Err(WriteError::TooLarge);
            }
            request = request.header("Content-Type", "application/json").body(bytes);
        }
        let response = request.send().map_err(|err| {
            if err.is_timeout() {
                WriteError::Timeout { create: None }
            } else {
                WriteError::Network
            }
        })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let mut bytes = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|err| {
            let timed_out = matches!(
                err.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) || err
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
                .is_some_and(reqwest::Error::is_timeout);
            if timed_out {
                WriteError::Timeout { create: None }
            } else {
                WriteError::Network
            }
        })?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err(WriteError::Malformed);
        }
        if (200..300).contains(&status) {
            // Never include raw bodies, URLs, reqwest diagnostics or serde input in errors.
            return serde_json::from_slice(&bytes).map_err(|_| WriteError::Malformed);
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let code = self.scrub(value["error"]["code"].as_str().unwrap_or_default());
        let message = self.scrub(value["error"]["message"].as_str().unwrap_or_default());
        Err(match (status, code.as_str()) {
            (400, _) => WriteError::BadRequest {
                message: if message.is_empty() {
                    "pmd refused the request (HTTP 400).".to_owned()
                } else {
                    message
                },
                code,
            },
            (401, _) => WriteError::Unauthorized,
            (403, _) => WriteError::InsufficientScope {
                message: if message.is_empty() {
                    "the token lacks the scope for this".to_owned()
                } else {
                    message
                },
            },
            (404, _) => WriteError::NotFound,
            (409, "stale_version") => WriteError::StaleVersion,
            (409, "already_exists") => WriteError::AlreadyExists,
            (409, "item_closed") => WriteError::ItemClosed,
            (409, _) => WriteError::Conflict {
                message: if message.is_empty() {
                    format!("pmd refused it: {code}")
                } else {
                    message
                },
                code,
            },
            (413, _) => WriteError::TooLarge,
            (428, _) => WriteError::PreconditionRequired,
            (429, _) => WriteError::RateLimited { retry_after },
            (500..=599, _) if !code.is_empty() => WriteError::Server { status, code, message },
            _ => WriteError::Http(status),
        })
    }
}

/// Reads a token file: one line of printable ASCII, at most 8 KB.
pub fn load_token(path: &Path) -> Result<String, WriteError> {
    let mut token = String::new();
    std::fs::File::open(path)
        .map_err(|_| WriteError::MissingToken)?
        .take(MAX_TOKEN as u64 + 1)
        .read_to_string(&mut token)
        .map_err(|_| WriteError::MissingToken)?;
    let token = token.trim();
    if !is_token(token) {
        return Err(WriteError::MissingToken);
    }
    Ok(token.to_owned())
}

fn is_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_graphic())
}

/// Stores `token` at `path` so only its owner can read it, replacing any older token in one step.
///
/// The token goes into a new file beside `path` that is owner-only before any byte is written (mode 600 on Unix; on
/// Windows the file is created with a protected DACL that grants only its owner, with nothing inherited), and that file
/// then replaces `path`. A rename keeps the new file's permissions, so the token is never readable by others, even
/// briefly.
pub fn store_token(path: &Path, token: &str) -> Result<(), WriteError> {
    let token = token.trim();
    if !is_token(token) {
        return Err(WriteError::Storage(
            "that isn't a pmd token (one line of printable characters).".to_owned(),
        ));
    }
    let storage = |err: std::io::Error| WriteError::Storage(format!("{} ({})", path.display(), err.kind()));
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(storage)?;
    let name = path
        .file_name()
        .ok_or_else(|| storage(std::io::ErrorKind::InvalidInput.into()))?;
    let temp = dir.join(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let result = (|| {
        let mut file = owner_only_file(&temp)?;
        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(storage)
}

#[cfg(unix)]
fn owner_only_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    // The umask can only remove bits, but make the mode exact anyway.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(windows)]
fn owner_only_file(path: &Path) -> std::io::Result<std::fs::File> {
    windows_acl::create_owner_only(path)
}

#[cfg(not(any(unix, windows)))]
fn owner_only_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// The DACL [`store_token`] gives a token file on Windows, as SDDL.
#[cfg(windows)]
pub const OWNER_ONLY_SDDL: &str = windows_acl::OWNER_ONLY;

/// The DACL a file has, as SDDL, for tests on Windows.
#[cfg(windows)]
pub fn token_file_sddl(path: &Path) -> std::io::Result<String> {
    windows_acl::dacl_sddl(path)
}

#[cfg(windows)]
mod windows_acl {
    //! An owner-only file on Windows: a protected DACL with one entry, full access for OWNER RIGHTS (`OW`), so neither
    //! the folder's inherited entries nor other users apply.
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};

    pub const OWNER_ONLY: &str = "D:P(A;;FA;;;OW)";

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Creates `path` (it must not exist) with the owner-only DACL attached from the start, opened for writing with no
    /// sharing. Tightening a file's DACL after creating it would leave a window in which another process could open it
    /// under the folder's inherited entries and keep that handle.
    pub fn create_owner_only(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::io::FromRawHandle as _;
        let sddl = wide(OWNER_ONLY.as_ref());
        let path = wide(path.as_os_str());
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `sddl` is NUL-terminated; on success the descriptor is freed with LocalFree below.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if converted == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: `path` is NUL-terminated and `attributes` points to a valid descriptor for the call's duration.
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
        let error = (handle == INVALID_HANDLE_VALUE).then(std::io::Error::last_os_error);
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW; CreateFileW copied what it needs.
        unsafe { LocalFree(descriptor as _) };
        match error {
            Some(error) => Err(error),
            // SAFETY: a valid handle that nothing else owns; the File closes it.
            None => Ok(unsafe { std::fs::File::from_raw_handle(handle as _) }),
        }
    }

    pub fn dacl_sddl(path: &Path) -> std::io::Result<String> {
        let path = wide(path.as_os_str());
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `path` is NUL-terminated; the descriptor is freed with LocalFree below.
        let status = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        let mut text: *mut u16 = null_mut();
        let mut len = 0u32;
        // SAFETY: `descriptor` is valid; `text` is allocated by the call and freed with LocalFree.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                &mut len,
            )
        };
        let result = if converted == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            // SAFETY: the call wrote `len` UTF-16 units, including the terminating NUL, at `text`.
            let units = unsafe { std::slice::from_raw_parts(text, len as usize) };
            Ok(String::from_utf16_lossy(units).trim_end_matches('\0').to_owned())
        };
        // SAFETY: both were allocated by the calls above (LocalFree ignores null).
        unsafe {
            LocalFree(text as _);
            LocalFree(descriptor as _);
        }
        result
    }
}
