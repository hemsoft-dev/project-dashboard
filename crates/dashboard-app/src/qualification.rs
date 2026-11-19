//! Repeated-refresh qualification (#102, docs/qualification.md).
//!
//! The real `BoardView` in an in-process test window, against a loopback pmd with a known number of checkouts and
//! known payload sizes, through refresh, a failing then recovering refresh, a config replacement and Project, Backlog
//! and Fleet switching, cycle after cycle. Never the user's running app or pmd. Warm-up cycles are kept apart from the
//! steady-state samples. Each cycle records refresh latencies and the process's private memory, threads and handles
//! (file descriptors on Linux). The run then closes the window and checks that the work stops: threads come back and
//! no request reaches pmd while time runs on. Budgets are in `BUDGETS`; the raw samples go to `PERF_DIR`.
//!
//! ```text
//! PERF_DIR=target/perf PERF_REVISION=$(git rev-parse HEAD) \
//!   cargo test -p dashboard-app --locked qualification -- --ignored --test-threads=1
//! PERF_FIXTURE=leak ... # the same, with a source that leaks a thread and memory per fetch: must fail
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dashboard_core::cache::StatusCache;
use dashboard_core::config::Config;
use dashboard_core::source::{FetchError, StatusSource};
use dashboard_core::status::StatusDocument;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

use crate::board_view::{BoardView, Options};
// The loopback pmd shared with dashboard-core's tests, loaded once by ui_tests.
use crate::ui_tests::{open_board, options, pmd_http};

const TOKEN: &str = "fixture-read-token";
/// Checkouts on the board, and Board and work items per project: the payload sizes the report records.
const CHECKOUTS: usize = 8;
const ITEMS: usize = 20;
const WARMUP: usize = 5;
const STEADY: usize = 30;
/// How long any one wait may take before the run counts as stuck, in real time.
const WAIT: Duration = Duration::from_secs(30);

/// What a run may not exceed. Set from measured baselines; docs/qualification.md says how to change them.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Budgets {
    /// 95th percentile of each refresh kind, in milliseconds of real time.
    pub refresh_p95_ms: f64,
    /// Private memory growth between the first and last five steady-state samples (medians), in bytes.
    pub memory_growth_bytes: i64,
    /// Threads above the count after warm-up, at any steady-state sample.
    pub thread_growth: i64,
    /// Handles (file descriptors on Linux) above the count after warm-up, at any steady-state sample.
    pub handle_growth: i64,
    /// Threads above the count before the window opened, once teardown has settled.
    pub threads_after_teardown: i64,
}

pub(crate) const BUDGETS: Budgets = Budgets {
    refresh_p95_ms: 3000.0,
    memory_growth_bytes: 24 * 1024 * 1024,
    thread_growth: 8,
    handle_growth: 64,
    threads_after_teardown: 2,
};

/// One reading of the process's resources. `None` where the platform can't say.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct Resources {
    pub private_bytes: Option<i64>,
    pub threads: Option<i64>,
    /// Kernel handles on Windows, open file descriptors on Linux.
    pub handles: Option<i64>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct Cycle {
    pub warmup: bool,
    /// Real milliseconds from the request to every card's result.
    pub refresh_ms: f64,
    pub failure_ms: f64,
    pub recovery_ms: f64,
    pub config_ms: f64,
    /// Real milliseconds from clicking Fleet to its data.
    pub fleet_ms: f64,
    /// After the cycle, with everything settled.
    pub resources: Resources,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct Teardown {
    pub view_released: bool,
    pub resources: Resources,
    /// Requests pmd received in the 2 simulated minutes after the window closed.
    pub requests_after: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct Report {
    pub revision: String,
    pub platform: String,
    pub fixture: String,
    pub profile: String,
    pub checkouts: usize,
    pub items_per_project: usize,
    /// Requests pmd answered during the workload, teardown's quiet period excluded.
    pub requests: usize,
    pub status_bytes: usize,
    pub work_bytes: usize,
    pub before: Resources,
    pub cycles: Vec<Cycle>,
    pub teardown: Teardown,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Summary {
    pub p50_ms: Vec<(String, f64)>,
    pub p95_ms: Vec<(String, f64)>,
    pub memory_growth_bytes: Option<i64>,
    pub thread_growth: Option<i64>,
    pub handle_growth: Option<i64>,
    pub threads_after_teardown: Option<i64>,
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if values.is_empty() {
        return f64::NAN;
    }
    let rank = (p / 100.0 * (values.len() - 1) as f64).round() as usize;
    values[rank.min(values.len() - 1)]
}

fn median(values: &[i64]) -> Option<i64> {
    let mut v = values.to_vec();
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

type Kind = (&'static str, fn(&Cycle) -> f64);
const KINDS: [Kind; 5] = [
    ("refresh", |c| c.refresh_ms),
    ("failure", |c| c.failure_ms),
    ("recovery", |c| c.recovery_ms),
    ("config", |c| c.config_ms),
    ("fleet", |c| c.fleet_ms),
];

pub(crate) fn summarize(report: &Report) -> Summary {
    let steady: Vec<&Cycle> = report.cycles.iter().filter(|c| !c.warmup).collect();
    let mut p50 = Vec::new();
    let mut p95 = Vec::new();
    for (name, get) in KINDS {
        let mut values: Vec<f64> = steady.iter().map(|c| get(c)).collect();
        p50.push((name.to_string(), percentile(&mut values, 50.0)));
        p95.push((name.to_string(), percentile(&mut values, 95.0)));
    }
    let series =
        |get: fn(&Resources) -> Option<i64>| -> Option<Vec<i64>> { steady.iter().map(|c| get(&c.resources)).collect() };
    let after_warmup = report.cycles.iter().rfind(|c| c.warmup).map(|c| c.resources);
    let growth_from_warmup = |get: fn(&Resources) -> Option<i64>| -> Option<i64> {
        let start = get(&after_warmup?)?;
        Some(series(get)?.into_iter().max()? - start)
    };
    let memory_growth = series(|r| r.private_bytes).and_then(|s| {
        if s.len() < 10 {
            return None;
        }
        Some(median(&s[s.len() - 5..])? - median(&s[..5])?)
    });
    Summary {
        p50_ms: p50,
        p95_ms: p95,
        memory_growth_bytes: memory_growth,
        thread_growth: growth_from_warmup(|r| r.threads),
        handle_growth: growth_from_warmup(|r| r.handles),
        threads_after_teardown: report
            .teardown
            .resources
            .threads
            .zip(report.before.threads)
            .map(|(after, before)| after - before),
    }
}

/// Every way the report misses its budgets, or is incomplete. Empty when it passes.
pub(crate) fn evaluate(report: &Report, budgets: &Budgets) -> Vec<String> {
    let mut failures = Vec::new();
    if report.revision.len() != 40 || !report.revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        failures.push(format!("no candidate revision: PERF_REVISION is {:?}", report.revision));
    }
    let warm = report.cycles.iter().filter(|c| c.warmup).count();
    let steady = report.cycles.len() - warm;
    if warm < WARMUP || steady < STEADY {
        failures.push(format!(
            "incomplete run: {warm} warm-up and {steady} steady cycles of {WARMUP} and {STEADY}"
        ));
    }
    let s = summarize(report);
    for (name, p95) in &s.p95_ms {
        if !p95.is_finite() || *p95 > budgets.refresh_p95_ms {
            failures.push(format!(
                "{name} p95 {p95:.0} ms is over {:.0} ms",
                budgets.refresh_p95_ms
            ));
        }
    }
    let mut check = |what: &str, value: Option<i64>, limit: i64| match value {
        None => failures.push(format!("{what}: not measured on {}", report.platform)),
        Some(v) if v > limit => failures.push(format!("{what} {v} is over {limit}")),
        Some(_) => {}
    };
    check(
        "private memory growth (bytes)",
        s.memory_growth_bytes,
        budgets.memory_growth_bytes,
    );
    check("threads above warm-up", s.thread_growth, budgets.thread_growth);
    check("handles above warm-up", s.handle_growth, budgets.handle_growth);
    check(
        "threads left after teardown",
        s.threads_after_teardown,
        budgets.threads_after_teardown,
    );
    if !report.teardown.view_released {
        failures.push("the board view was still alive after its window closed".into());
    }
    if report.teardown.requests_after > 0 {
        failures.push(format!(
            "{} requests reached pmd after the window closed",
            report.teardown.requests_after
        ));
    }
    failures
}

pub(crate) fn markdown(report: &Report, budgets: &Budgets, failures: &[String]) -> String {
    let s = summarize(report);
    let mut out = format!(
        "# Refresh qualification ({}, {})\n\n- Revision: `{}`\n- Fixture: {}\n- Profile: {}\n- Workload: {} checkouts, {} items per project ({} B status, {} B work per project), {} warm-up + {} steady cycles\n- Gate: {}\n\n| Refresh | p50 ms | p95 ms |\n| --- | ---: | ---: |\n",
        report.platform,
        report.fixture,
        report.revision,
        report.fixture,
        report.profile,
        report.checkouts,
        report.items_per_project,
        report.status_bytes,
        report.work_bytes,
        WARMUP,
        STEADY,
        if failures.is_empty() { "pass" } else { "FAIL" }
    );
    for ((name, p50), (_, p95)) in s.p50_ms.iter().zip(&s.p95_ms) {
        out += &format!("| {name} | {p50:.0} | {p95:.0} |\n");
    }
    out += &format!(
        "\n| Resource | Measured | Budget |\n| --- | ---: | ---: |\n| private memory growth (B) | {:?} | {} |\n| threads above warm-up | {:?} | {} |\n| handles above warm-up | {:?} | {} |\n| threads after teardown | {:?} | {} |\n| requests after teardown | {} | 0 |\n",
        s.memory_growth_bytes,
        budgets.memory_growth_bytes,
        s.thread_growth,
        budgets.thread_growth,
        s.handle_growth,
        budgets.handle_growth,
        s.threads_after_teardown,
        budgets.threads_after_teardown,
        report.teardown.requests_after
    );
    out += &format!("\n## Failures ({})\n\n", failures.len());
    for f in failures {
        out += &format!("- {f}\n");
    }
    out
}

/// The process's own resources, from the OS.
pub(crate) fn sample() -> Resources {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|v| v.split_whitespace().next()?.parse::<i64>().ok())
        };
        Resources {
            // Anonymous resident memory: heap and stacks, not shared libraries or files.
            private_bytes: field("RssAnon:").map(|kb| kb * 1024),
            threads: field("Threads:"),
            handles: std::fs::read_dir("/proc/self/fd").ok().map(|d| d.count() as i64),
        }
    }
    #[cfg(windows)]
    {
        windows::sample()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Resources::default()
    }
}

#[cfg(windows)]
mod windows {
    use super::Resources;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount};

    pub(super) fn sample() -> Resources {
        // SAFETY: the pseudo-handle needs no closing; each out-parameter is a valid, correctly sized local.
        unsafe {
            let process = GetCurrentProcess();
            let mut memory: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            memory.cb = size;
            let private_bytes = (K32GetProcessMemoryInfo(
                process,
                (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                size,
            ) != 0)
                .then_some(memory.PrivateUsage as i64);
            let mut handles = 0u32;
            let handles = (GetProcessHandleCount(process, &mut handles) != 0).then_some(i64::from(handles));
            Resources {
                private_bytes,
                threads: threads(GetCurrentProcessId()),
                handles,
            }
        }
    }

    /// Threads of this process, from a system thread snapshot.
    fn threads(pid: u32) -> Option<i64> {
        // SAFETY: the snapshot handle is checked, walked with a sized entry and closed once.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut count = 0;
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    count += 1;
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            Some(count)
        }
    }
}

/// The loopback pmd: `CHECKOUTS` projects, each with `ITEMS` Board and work items, and the Fleet routes.
struct Pmd {
    server: pmd_http::Server,
    failing: Arc<AtomicBool>,
    status_bytes: usize,
    work_bytes: usize,
}

fn item(project: &str, n: usize) -> Value {
    json!({
        "id": format!("{project}-{n}"),
        "project": project,
        "kind": "issue",
        "number": n,
        "title": format!("Work item {n} of {project}, long enough to look like a real title"),
        "url": format!("https://github.com/hemsoft-dev/{project}/issues/{n}"),
        "status": "ready",
    })
}

fn project_routes(path: &str) -> Option<(u16, String)> {
    let projects: Vec<Value> = (0..CHECKOUTS)
        .map(|k| json!({"slug": format!("repo-{k}"), "repo": format!("hemsoft-dev/repo-{k}"), "priority": k % 5, "tracked": true}))
        .collect();
    if path == "/v1/projects" {
        return Some((200, json!({"projects": projects}).to_string()));
    }
    let project = |p: &str| (0..CHECKOUTS).map(|k| format!("repo-{k}")).find(|name| name == p);
    if let Some(rest) = path.strip_prefix("/v1/projects/")
        && let Some(slug) = rest.strip_suffix("/status").and_then(project)
    {
        // LF line endings whatever the checkout did to the fixture file, so the payload is the same size everywhere.
        let status = dashboard_core::fixtures::CLEAN
            .replace("\r\n", "\n")
            .replace("hemsoft-dev/gh-x", &format!("hemsoft-dev/{slug}"));
        return Some((
            200,
            format!(
                r#"{{"project":"{slug}","generatedAt":"2026-11-19T08:00:00Z","lastError":null,"status":{status}}}"#
            ),
        ));
    }
    if let Some(slug) = path.strip_prefix("/v1/board?project=").and_then(project) {
        let items: Vec<Value> = (0..ITEMS).map(|n| item(&slug, n)).collect();
        let columns = json!([
            {"status": "ready", "title": "Ready", "items": items, "truncated": false},
            {"status": "in_progress", "title": "In progress", "items": [], "truncated": false},
            {"status": "waiting_on_franz", "title": "Waiting on Franz", "items": [], "truncated": false},
            {"status": "blocked", "title": "Blocked", "items": [], "truncated": false},
        ]);
        return Some((
            200,
            json!({"generatedAt": "2026-11-19T08:00:00Z", "columns": columns}).to_string(),
        ));
    }
    if let Some(slug) = path
        .strip_prefix("/v1/work?project=")
        .and_then(|p| p.strip_suffix("&limit=500"))
        .and_then(project)
    {
        let items: Vec<Value> = (0..ITEMS).map(|n| item(&slug, n)).collect();
        return Some((200, json!({"items": items, "nextCursor": null}).to_string()));
    }
    if path.starts_with("/v1/assignments?") {
        return Some((200, r#"{"assignments":[],"truncated":false}"#.into()));
    }
    let fixed = match path {
        "/v1/portfolio" => "{}",
        "/v1/dispatch/plan" => r#"{"recommendations":[],"skipped":[]}"#,
        "/healthz" => r#"{"sweepAgeSeconds":60,"webhookAgeSeconds":null,"githubLow":false}"#,
        "/v1/fleet/activity" => r#"{"schema":1,"machines":[]}"#,
        "/v1/machines" => r#"{"machines":[{"name":"mini","os":"linux","maxSessions":3}]}"#,
        "/v1/capacity" => r#"{"machines":[{"name":"mini","used":1,"free":2}]}"#,
        "/v1/fleet/status" => r#"{"schema":1,"machines":[]}"#,
        _ => return None,
    };
    Some((200, fixed.into()))
}

impl Pmd {
    fn new() -> Self {
        let failing = Arc::new(AtomicBool::new(false));
        let broken = failing.clone();
        let server = pmd_http::Server::with_auth(move |path, token| {
            assert_eq!(token, TOKEN);
            // A failing pmd answers status reads with 503: each card shows its error and keeps its last good data.
            if broken.load(Ordering::SeqCst) && path.ends_with("/status") && path.starts_with("/v1/projects/") {
                return pmd_http::Reply {
                    code: 503,
                    body: r#"{"error":{"code":"unavailable","message":"down"}}"#.into(),
                };
            }
            let (code, body) = project_routes(path).unwrap_or_else(|| panic!("unexpected fixture path {path}"));
            pmd_http::Reply { code, body }
        });
        Self {
            server,
            failing,
            status_bytes: project_routes("/v1/projects/repo-0/status").map_or(0, |(_, b)| b.len()),
            work_bytes: project_routes("/v1/work?project=repo-0&limit=500").map_or(0, |(_, b)| b.len()),
        }
    }

    /// Requests since the last call. The fixture's log is emptied each time, so it doesn't grow with the run and count
    /// against the dashboard's memory.
    fn take(&self) -> usize {
        let mut log = self.server.requests.lock().unwrap();
        let n = log.len();
        log.clear();
        n
    }
}

/// A source that leaks a parked thread and a megabyte per fetch: the controlled failure the qualification must catch.
struct Leaking(Arc<dyn StatusSource>);

impl StatusSource for Leaking {
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError> {
        std::thread::spawn(|| {
            loop {
                std::thread::park();
            }
        });
        std::mem::forget(vec![1u8; 1024 * 1024]);
        self.0.fetch(checkout, refresh)
    }
}

struct Workload {
    dir: PathBuf,
    config: Config,
    all: Vec<PathBuf>,
    handle: AnyWindowHandle,
    view: Entity<BoardView>,
}

impl Workload {
    fn open(cx: &mut TestAppContext, pmd: &Pmd, leak: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("project-dashboard-qualification-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut repositories = Vec::new();
        for k in 0..CHECKOUTS {
            let checkout = dir.join(format!("repo-{k}"));
            std::fs::create_dir_all(checkout.join(".git")).unwrap();
            std::fs::write(
                checkout.join(".git/config"),
                format!("[remote \"origin\"]\nurl = git@github.com:hemsoft-dev/repo-{k}.git\n"),
            )
            .unwrap();
            repositories.push(checkout);
        }
        std::fs::write(dir.join("token"), TOKEN).unwrap();
        let config = Config {
            repositories: repositories.clone(),
            refresh_minutes: 0,
            parallelism: 4,
            pmd_url: pmd.server.url.clone(),
            pmd_token_path: "token".into(),
            ..Default::default()
        };
        std::fs::write(dir.join("config.toml"), config.to_toml()).unwrap();
        let live = dashboard_core::priority::Pmd::from_config(&config, &dir);
        let source: Arc<dyn StatusSource> = if leak {
            Arc::new(Leaking(Arc::new(live.clone())))
        } else {
            Arc::new(live.clone())
        };
        let (handle, view) = open_board(
            cx,
            repositories,
            StatusCache::default(),
            source,
            Options {
                config_dir: Some(dir.clone()),
                cache_dir: Some(dir.clone()),
                priorities: Arc::new(live.clone()),
                pmd: Some(live),
                ..options()
            },
        );
        Self {
            dir,
            all: config.repositories.clone(),
            config,
            handle,
            view,
        }
    }

    /// Runs the GPUI clock and the real threads until `done`, and returns the real time it took.
    fn until(&self, cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) -> f64 {
        let start = Instant::now();
        while !done(cx) {
            assert!(start.elapsed() < WAIT, "stuck waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
            cx.executor().advance_clock(Duration::from_millis(100));
            cx.run_until_parked();
        }
        start.elapsed().as_secs_f64() * 1000.0
    }

    fn idle(&self, cx: &mut TestAppContext) -> bool {
        cx.update(|cx| {
            let view = self.view.read(cx);
            !view.board().is_refreshing() && !view.priorities_pending() && view.pending_refreshes() == 0
        })
    }

    fn refresh(&self, cx: &mut TestAppContext) -> f64 {
        let start = Instant::now();
        cx.update(|cx| self.view.update(cx, |view, cx| view.refresh_all(cx)));
        self.until(cx, "a refresh", |cx| self.idle(cx));
        start.elapsed().as_secs_f64() * 1000.0
    }

    fn render(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    fn click(&self, cx: &mut TestAppContext, button: &'static str) {
        cx.update_window(self.handle, |_, window, cx| {
            window.click(button, cx);
            window.render_frame(cx);
        })
        .unwrap();
    }

    fn cycle(&mut self, cx: &mut TestAppContext, pmd: &Pmd, warmup: bool, n: usize) -> Cycle {
        let refresh_ms = self.refresh(cx);
        pmd.failing.store(true, Ordering::SeqCst);
        let failure_ms = self.refresh(cx);
        let (failed, cards) = cx.update(|cx| {
            let cards = self.view.read(cx).board().cards();
            (cards.iter().filter(|c| c.error.is_some()).count(), cards.len())
        });
        assert_eq!(failed, cards, "every card shows the failure");
        pmd.failing.store(false, Ordering::SeqCst);
        let recovery_ms = self.refresh(cx);
        let recovered = cx.update(|cx| self.view.read(cx).board().cards().iter().all(|c| c.error.is_none()));
        assert!(recovered, "every card recovers");
        // A config replacement, applied by the config poll: one checkout leaves on even cycles and comes back on odd
        // ones (its card is added and refreshed), and parallelism alternates.
        let keep = if n.is_multiple_of(2) { CHECKOUTS - 1 } else { CHECKOUTS };
        self.config.repositories = self.all[..keep].to_vec();
        self.config.parallelism = if n.is_multiple_of(2) { 3 } else { 4 };
        let padding = " ".repeat(n % 7 + 1);
        std::fs::write(
            self.dir.join("config.toml"),
            format!("{}#{padding}\n", self.config.to_toml()),
        )
        .unwrap();
        let start = Instant::now();
        self.until(cx, "the config", |cx| {
            cx.update(|cx| self.view.read(cx).board().cards().len() == keep) && self.idle(cx)
        });
        let config_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.render(cx);
        self.click(cx, "Backlog");
        let before = cx.update(|cx| self.view.read(cx).fleet_status.fetched_at);
        let start = Instant::now();
        self.click(cx, "Fleet");
        self.until(cx, "Fleet", |cx| {
            cx.update(|cx| {
                let view = self.view.read(cx);
                !view.fleet_loading && view.fleet_status.fetched_at.is_some() && view.fleet_status.fetched_at != before
            })
        });
        let fleet_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.click(cx, "Project");
        self.until(cx, "settled", |cx| self.idle(cx));
        Cycle {
            warmup,
            refresh_ms,
            failure_ms,
            recovery_ms,
            config_ms,
            fleet_ms,
            resources: sample(),
        }
    }
}

/// Runs the whole workload and returns its report; the caller evaluates it.
fn run(cx: &mut TestAppContext, leak: bool) -> Report {
    let pmd = Pmd::new();
    let before = sample();
    let mut workload = Workload::open(cx, &pmd, leak);
    let mut cycles = Vec::new();
    let mut served = 0;
    for n in 0..WARMUP + STEADY {
        cycles.push(workload.cycle(cx, &pmd, n < WARMUP, n));
        served += pmd.take();
    }
    // Teardown: close the window and let go of the view, then run the clock: nothing may keep working.
    let weak = workload.view.downgrade();
    cx.update_window(workload.handle, |_, window, _| window.remove_window())
        .unwrap();
    let dir = workload.dir.clone();
    drop(workload);
    let start = Instant::now();
    let mut teardown = Teardown::default();
    loop {
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        let now = sample();
        let settled = match (now.threads, before.threads) {
            (Some(t), Some(b)) => t - b <= BUDGETS.threads_after_teardown,
            _ => true,
        };
        if settled || start.elapsed() > Duration::from_secs(10) {
            break;
        }
    }
    teardown.view_released = cx.update(|_| weak.upgrade().is_none());
    served += pmd.take();
    for _ in 0..240 {
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
    }
    std::thread::sleep(Duration::from_millis(300));
    teardown.requests_after = pmd.take();
    teardown.resources = sample();
    let _ = std::fs::remove_dir_all(dir);
    Report {
        revision: std::env::var("PERF_REVISION").unwrap_or_default(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        fixture: if leak { "leak" } else { "baseline" }.into(),
        profile: if cfg!(debug_assertions) { "debug" } else { "release" }.into(),
        checkouts: CHECKOUTS,
        items_per_project: ITEMS,
        requests: served,
        status_bytes: pmd.status_bytes,
        work_bytes: pmd.work_bytes,
        before,
        cycles,
        teardown,
    }
}

/// The qualification itself. Ignored by default: it runs alone, so the process's resources are its own.
#[gpui_kit::test]
#[ignore = "run alone: cargo test -p dashboard-app qualification -- --ignored --test-threads=1"]
fn qualification_repeated_refresh_stays_within_budget(cx: &mut TestAppContext) {
    let leak = std::env::var("PERF_FIXTURE").as_deref() == Ok("leak");
    let report = run(cx, leak);
    let failures = evaluate(&report, &BUDGETS);
    let md = markdown(&report, &BUDGETS, &failures);
    let out = PathBuf::from(std::env::var("PERF_DIR").unwrap_or_else(|_| "../../target/perf".into()));
    std::fs::create_dir_all(&out).unwrap();
    let stem = format!("refresh-{}-{}", report.platform, report.fixture);
    let raw = json!({"budgets": BUDGETS, "summary": summarize(&report), "failures": failures, "report": report});
    std::fs::write(
        out.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&raw).unwrap(),
    )
    .unwrap();
    std::fs::write(out.join(format!("{stem}.md")), &md).unwrap();
    println!("{md}");
    assert!(failures.is_empty(), "qualification failed:\n{}", failures.join("\n"));
}

#[cfg(test)]
mod evaluation {
    use super::*;

    fn cycle(warmup: bool, ms: f64, memory: i64, threads: i64, handles: i64) -> Cycle {
        Cycle {
            warmup,
            refresh_ms: ms,
            failure_ms: ms,
            recovery_ms: ms,
            config_ms: ms,
            fleet_ms: ms,
            resources: Resources {
                private_bytes: Some(memory),
                threads: Some(threads),
                handles: Some(handles),
            },
        }
    }

    fn report(steady: impl Fn(usize) -> Cycle) -> Report {
        let mut cycles: Vec<Cycle> = (0..WARMUP).map(|_| cycle(true, 100.0, 100 << 20, 20, 200)).collect();
        cycles.extend((0..STEADY).map(steady));
        Report {
            revision: "a".repeat(40),
            platform: "test".into(),
            cycles,
            before: Resources {
                private_bytes: Some(50 << 20),
                threads: Some(18),
                handles: Some(150),
            },
            teardown: Teardown {
                view_released: true,
                resources: Resources {
                    private_bytes: Some(100 << 20),
                    threads: Some(19),
                    handles: Some(160),
                },
                requests_after: 0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_steady_run_within_budget_passes() {
        let r = report(|i| cycle(false, 100.0 + i as f64, (100 << 20) + (i as i64 % 3) * 4096, 21, 205));
        assert_eq!(evaluate(&r, &BUDGETS), Vec::<String>::new());
    }

    #[test]
    fn growing_memory_threads_or_handles_fail() {
        let r = report(|i| {
            cycle(
                false,
                100.0,
                (100 << 20) + i as i64 * (2 << 20),
                20 + i as i64,
                200 + 10 * i as i64,
            )
        });
        let failures = evaluate(&r, &BUDGETS);
        assert_eq!(failures.len(), 3, "{failures:?}");
        assert!(failures[0].starts_with("private memory growth (bytes)"));
        assert!(failures[1].starts_with("threads above warm-up 29 is over 8"));
        assert!(failures[2].starts_with("handles above warm-up 290 is over 64"));
    }

    #[test]
    fn slow_refreshes_fail_on_their_95th_percentile() {
        // Of 30 samples the 95th percentile is the 29th smallest: one slow sample is an outlier, two are not.
        let r = report(|i| cycle(false, if i < 1 { 9000.0 } else { 100.0 }, 100 << 20, 20, 200));
        assert!(
            evaluate(&r, &BUDGETS).iter().all(|f| !f.contains("p95")),
            "1 of 30 slow is an outlier"
        );
        let r = report(|i| cycle(false, if i < 2 { 9000.0 } else { 100.0 }, 100 << 20, 20, 200));
        assert_eq!(evaluate(&r, &BUDGETS).len(), 5, "every kind's p95 is over");
    }

    #[test]
    fn work_after_teardown_or_a_live_view_fails() {
        let mut r = report(|_| cycle(false, 100.0, 100 << 20, 20, 200));
        r.teardown.requests_after = 2;
        r.teardown.view_released = false;
        r.teardown.resources.threads = Some(30);
        let failures = evaluate(&r, &BUDGETS);
        assert!(
            failures.iter().any(|f| f == "threads left after teardown 12 is over 2"),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|f| f == "the board view was still alive after its window closed")
        );
        assert!(
            failures
                .iter()
                .any(|f| f == "2 requests reached pmd after the window closed")
        );
    }

    #[test]
    fn missing_evidence_never_passes() {
        let mut r = report(|_| cycle(false, 100.0, 100 << 20, 20, 200));
        r.revision = String::new();
        r.cycles.truncate(20);
        for c in &mut r.cycles {
            c.resources.handles = None;
        }
        let failures = evaluate(&r, &BUDGETS);
        assert!(failures[0].starts_with("no candidate revision"), "{failures:?}");
        assert!(
            failures
                .iter()
                .any(|f| f.starts_with("incomplete run: 5 warm-up and 15 steady"))
        );
        assert!(
            failures
                .iter()
                .any(|f| f == "handles above warm-up: not measured on test")
        );
    }

    #[test]
    fn the_os_reports_this_process_s_threads_and_memory() {
        let r = sample();
        if cfg!(any(target_os = "linux", windows)) {
            assert!(
                r.threads.unwrap() >= 1 && r.private_bytes.unwrap() > 0 && r.handles.unwrap() > 0,
                "{r:?}"
            );
            let before = r.threads.unwrap();
            let (go, wait) = std::sync::mpsc::channel::<()>();
            let thread = std::thread::spawn(move || wait.recv().ok());
            let during = sample().threads.unwrap();
            go.send(()).unwrap();
            thread.join().unwrap();
            assert!(
                during > before || during >= 2,
                "a new thread shows: {before} then {during}"
            );
        }
    }
}
