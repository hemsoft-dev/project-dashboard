//! Running `gh x status --json` in a checkout: the only way the app learns a repository's status.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::status::{ParseError, StatusDocument};

/// Why a repository's status couldn't be fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The GitHub CLI isn't installed or isn't on PATH.
    GhMissing,
    /// `gh` runs, but the gh-x extension (or its `--json` flag) isn't installed.
    GhXMissing(String),
    /// The checkout folder doesn't exist.
    MissingFolder,
    /// gh-x ran and reported a failure, or printed something unreadable.
    Failed(String),
}

impl FetchError {
    /// True for problems with the machine's setup rather than one repository, which the board shows once.
    pub fn is_setup(&self) -> bool {
        matches!(self, Self::GhMissing | Self::GhXMissing(_))
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GhMissing => f.write_str(
                "The GitHub CLI (gh) isn't installed. Install it from https://cli.github.com and sign in with gh auth login.",
            ),
            Self::GhXMissing(detail) => write!(
                f,
                "Project Dashboard needs the gh-x extension with gh x status --json. Install it with gh extension \
                 install hemsoft-dev/gh-x, or update it with gh extension upgrade gh-x. (gh said: {detail})"
            ),
            Self::MissingFolder => f.write_str("The folder doesn't exist."),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for FetchError {}

/// Anything that can report a checkout's status. The app uses [`GhX`]; tests use fakes.
pub trait StatusSource: Send + Sync {
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError>;
}

/// How long one `gh x status` may run before it is stopped and its card shows a failure.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// The real source: `gh x status --json`, run in the checkout.
#[derive(Clone, Debug)]
pub struct GhX {
    /// The `gh` executable; `gh` on PATH by default.
    pub program: std::ffi::OsString,
    /// Each run is stopped after this long, so a stalled network call or credential prompt can't hold a card (and a
    /// worker) forever.
    pub timeout: Duration,
}

impl Default for GhX {
    fn default() -> Self {
        Self {
            program: "gh".into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl GhX {
    pub fn args(refresh: bool) -> Vec<&'static str> {
        let mut args = vec!["x", "status", "--json"];
        if refresh {
            args.push("--refresh");
        }
        args
    }
}

impl StatusSource for GhX {
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError> {
        if !checkout.is_dir() {
            return Err(FetchError::MissingFolder);
        }
        let mut command = Command::new(&self.program);
        command
            .args(Self::args(refresh))
            .current_dir(checkout)
            .stdin(std::process::Stdio::null())
            // No colors, prompts or pagers: gh-x prints plain JSON.
            .env("NO_COLOR", "1")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // CREATE_NO_WINDOW: no console window flashes up for each repository.
            command.creation_flags(0x0800_0000);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => FetchError::GhMissing,
            _ => FetchError::Failed(format!("couldn't start gh: {err}")),
        })?;
        // Both pipes are drained on their own threads, so a full pipe can't stall the process while it is watched.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(FetchError::Failed(format!(
                        "gh x status didn't finish within {} s and was stopped.",
                        self.timeout.as_secs()
                    )));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(err) => return Err(FetchError::Failed(format!("couldn't wait for gh: {err}"))),
            }
        };
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        interpret(&stdout, &stderr, status.success())
    }
}

/// Reads a child's pipe to the end on its own thread.
fn drain(pipe: Option<impl std::io::Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// Turns gh's output into a document or an error. gh-x prints a JSON error object (and exits non-zero) when it can't
/// report, so stdout is read first whatever the exit code.
pub fn interpret(stdout: &[u8], stderr: &[u8], success: bool) -> Result<StatusDocument, FetchError> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stdout.trim().is_empty() {
        // gh's message for a missing extension, and a gh-x too old for --json (Go's flag package).
        let missing = stderr.lines().find(|line| {
            let line = line.to_lowercase();
            line.contains("unknown command \"x\"")
                || line.contains("flag provided but not defined: -json")
                || line.contains("unknown flag: --json")
        });
        if let Some(line) = missing {
            return Err(FetchError::GhXMissing(line.trim().to_owned()));
        }
        let message = if stderr.is_empty() {
            if success {
                "gh x status printed nothing".to_owned()
            } else {
                "gh x status failed without a message".to_owned()
            }
        } else {
            first_line(stderr)
        };
        return Err(FetchError::Failed(message));
    }
    StatusDocument::parse(&stdout).map_err(|err| match err {
        ParseError::Reported { message, .. } if !message.is_empty() => FetchError::Failed(message),
        other => FetchError::Failed(other.to_string()),
    })
}

/// The first line that says something: gh-x starts its output with a version banner (`gh-x v0.21.2 (…) © …`).
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("gh-x v"))
        .unwrap_or_default()
        .to_owned()
}
