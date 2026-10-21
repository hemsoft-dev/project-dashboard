//! The T3 Code thread working on a checkout (#15), for the row button that opens it.
//!
//! T3 Code has no supported way for another app to ask about its threads: its MCP endpoint needs the credential it
//! gives its own agents, and nothing outside it can open a thread in its desktop window. So this reads the local T3
//! server's state database, read-only, and links to the thread in T3's web UI (`<origin>/<environmentId>/<threadId>`).
//! The database is T3's internal storage, so every step fails soft: any surprise becomes a reason the button shows
//! instead of a link. The dashboard never writes to T3's files.
//!
//! The thread for a checkout is the newest unsettled, unarchived top-level thread of the T3 project whose workspace
//! root is that checkout, with a running thread ahead of any other. Each machine runs its own T3 server for its own
//! checkouts, so the local server is the one to ask.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

/// The state database T3 Code's server keeps under its `userdata` folder.
pub const STATE_DB: &str = "statev2.sqlite";

/// A thread to open, and its title for the button's tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadLink {
    pub title: String,
    pub url: String,
    pub running: bool,
}

/// Why a checkout has no thread to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoThread {
    /// No T3 Code server data on this computer.
    NotInstalled,
    /// T3 Code is installed but its server isn't answering.
    NotRunning,
    /// T3's files couldn't be read, or weren't as expected.
    Unreadable(String),
    /// T3 has no project for this checkout.
    NoProject,
    /// The project has no open thread.
    NoOpenThread,
}

impl std::fmt::Display for NoThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "T3 Code isn't installed on this computer"),
            Self::NotRunning => write!(f, "T3 Code isn't running"),
            Self::Unreadable(reason) => write!(f, "Can't read T3 Code's threads: {reason}"),
            Self::NoProject => write!(f, "No T3 Code project for this checkout"),
            Self::NoOpenThread => write!(f, "No open T3 Code thread for this checkout"),
        }
    }
}

/// What the dashboard can find out about each checkout's thread.
pub type Lookup = Result<ThreadLink, NoThread>;

/// T3's `userdata` folder: `T3CODE_USERDATA` when set (tests, other layouts), else `~/.t3-server/userdata`.
pub fn default_userdata() -> PathBuf {
    if let Some(dir) = std::env::var_os("T3CODE_USERDATA").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_default().join(".t3-server").join("userdata")
}

/// The local T3 server, as its `server-runtime.json` and `environment-id` describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// `http://127.0.0.1:3773`.
    pub origin: String,
    pub environment_id: String,
    pub state_db: PathBuf,
}

#[derive(Deserialize)]
struct Runtime {
    origin: String,
}

impl Server {
    /// Reads where the server listens and which environment it is. Doesn't check that it is running.
    pub fn locate(userdata: &Path) -> Result<Self, NoThread> {
        let state_db = userdata.join(STATE_DB);
        if !state_db.is_file() {
            return Err(NoThread::NotInstalled);
        }
        let runtime =
            std::fs::read_to_string(userdata.join("server-runtime.json")).map_err(|_| NoThread::NotRunning)?;
        let runtime: Runtime = serde_json::from_str(&runtime)
            .map_err(|err| NoThread::Unreadable(format!("server-runtime.json: {err}")))?;
        let environment_id = std::fs::read_to_string(userdata.join("environment-id"))
            .map_err(|err| NoThread::Unreadable(format!("environment-id: {err}")))?
            .trim()
            .to_owned();
        if environment_id.is_empty() || !runtime.origin.starts_with("http") {
            return Err(NoThread::Unreadable("unexpected server identity".to_owned()));
        }
        Ok(Self {
            origin: runtime.origin.trim_end_matches('/').to_owned(),
            environment_id,
            state_db,
        })
    }

    /// Whether something accepts connections at the origin's host and port, within `timeout`.
    pub fn is_listening(&self, timeout: Duration) -> bool {
        let Some(authority) = self.origin.split("://").nth(1) else {
            return false;
        };
        let authority = authority.split('/').next().unwrap_or(authority);
        use std::net::ToSocketAddrs as _;
        authority
            .to_socket_addrs()
            .into_iter()
            .flatten()
            .any(|address| std::net::TcpStream::connect_timeout(&address, timeout).is_ok())
    }

    /// The web UI's address for a thread. The ID is percent-encoded as T3's router expects (`mcp:` is `mcp%3A`).
    pub fn thread_url(&self, thread_id: &str) -> String {
        format!("{}/{}/{}", self.origin, encode(&self.environment_id), encode(thread_id))
    }
}

/// Percent-encodes everything but unreserved URL characters.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Finds each checkout's thread; the dashboard's view of T3, so tests and the demo can stand in for it.
pub trait ThreadFinder: Send + Sync {
    /// One result per checkout, in the order given. May block briefly: callers run it off the UI thread.
    fn find(&self, checkouts: &[PathBuf]) -> Vec<Lookup>;
}

/// The T3 Code server on this computer.
pub struct LocalT3 {
    pub userdata: PathBuf,
}

impl ThreadFinder for LocalT3 {
    fn find(&self, checkouts: &[PathBuf]) -> Vec<Lookup> {
        find(&self.userdata, checkouts)
    }
}

/// The row button's tooltip: the thread to open, or why there is none.
pub fn tooltip(lookup: &Lookup) -> String {
    match lookup {
        Ok(link) if link.running => format!("Open the running T3 Code thread: {}", link.title),
        Ok(link) => format!("Open the T3 Code thread: {}", link.title),
        Err(reason) => reason.to_string(),
    }
}

/// The thread for each checkout, in the order given, from the local T3 server in `userdata`.
pub fn find(userdata: &Path, checkouts: &[PathBuf]) -> Vec<Lookup> {
    let all = |reason: NoThread| checkouts.iter().map(|_| Err(reason.clone())).collect();
    let server = match Server::locate(userdata) {
        Ok(server) => server,
        Err(reason) => return all(reason),
    };
    if !server.is_listening(Duration::from_millis(300)) {
        return all(NoThread::NotRunning);
    }
    match read(&server.state_db) {
        Ok(state) => checkouts
            .iter()
            .map(|checkout| state.lookup(&server, checkout))
            .collect(),
        Err(reason) => all(reason),
    }
}

/// A T3 project and its candidate threads, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub projects: Vec<Project>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub workspace_root: PathBuf,
    /// Open top-level threads, running ones first, then newest first.
    pub threads: Vec<Thread>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub running: bool,
}

impl State {
    /// The thread link for `checkout`, or why there isn't one.
    pub fn lookup(&self, server: &Server, checkout: &Path) -> Lookup {
        let projects: Vec<&Project> = self
            .projects
            .iter()
            .filter(|project| same_path(&project.workspace_root, checkout))
            .collect();
        if projects.is_empty() {
            return Err(NoThread::NoProject);
        }
        // A checkout registered twice: a running thread in either wins, then the first project's newest.
        let thread = projects
            .iter()
            .flat_map(|project| project.threads.iter())
            .find(|thread| thread.running)
            .or_else(|| projects.iter().find_map(|project| project.threads.first()))
            .ok_or(NoThread::NoOpenThread)?;
        Ok(ThreadLink {
            title: thread.title.clone(),
            url: server.thread_url(&thread.id),
            running: thread.running,
        })
    }
}

/// Opens the database read-only and reads every live project with its open top-level threads. Any error, including
/// a missing table or column after a T3 update, is a reason rather than a panic.
pub fn read(state_db: &Path) -> Result<State, NoThread> {
    let unreadable = |err: rusqlite::Error| NoThread::Unreadable(err.to_string());
    let connection = Connection::open_with_flags(
        state_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(unreadable)?;
    // T3 may be writing; wait briefly for its lock rather than failing at once.
    connection
        .busy_timeout(Duration::from_millis(500))
        .map_err(unreadable)?;
    let mut projects: Vec<(String, Project)> = connection
        .prepare("SELECT project_id, workspace_root FROM projection_projects WHERE deleted_at IS NULL")
        .map_err(unreadable)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Project {
                    workspace_root: PathBuf::from(row.get::<_, String>(1)?),
                    threads: Vec::new(),
                },
            ))
        })
        .map_err(unreadable)?
        .collect::<Result<_, _>>()
        .map_err(unreadable)?;
    let threads: Vec<(String, Thread)> = connection
        .prepare(
            "SELECT t.project_id, t.thread_id, t.title,
                    EXISTS (SELECT 1 FROM orchestration_v2_projection_runs r
                            WHERE r.thread_id = t.thread_id
                              AND r.status IN ('preparing', 'queued', 'starting', 'running', 'waiting'))
             FROM orchestration_v2_projection_threads t
             WHERE t.archived_at IS NULL
               AND t.deleted_at IS NULL
               AND json_extract(t.payload_json, '$.settledAt') IS NULL
               AND json_extract(t.payload_json, '$.lineage.parentThreadId') IS NULL
             ORDER BY t.updated_at DESC",
        )
        .map_err(unreadable)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Thread {
                    id: row.get(1)?,
                    title: row.get(2)?,
                    running: row.get(3)?,
                },
            ))
        })
        .map_err(unreadable)?
        .collect::<Result<_, _>>()
        .map_err(unreadable)?;
    for (project_id, thread) in threads {
        if let Some((_, project)) = projects.iter_mut().find(|(id, _)| *id == project_id) {
            project.threads.push(thread);
        }
    }
    for (_, project) in &mut projects {
        // Stable: newest first stays within the running and the rest.
        project.threads.sort_by_key(|thread| !thread.running);
    }
    Ok(State {
        projects: projects.into_iter().map(|(_, project)| project).collect(),
    })
}

/// Whether two paths name the same folder, ignoring separators, a trailing separator, and case on Windows.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let normal = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_owned();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    normal(a) == normal(b)
}
