//! The dashboard window's content: a title bar with the refresh controls, then a table with one row per configured
//! checkout: its overall state, name, open PR and issue counts, when it was last updated (#13), and its last activity
//! (#8).
//!
//! Each row's T3 button, left of its refresh button, opens the T3 Code thread working on that checkout (#15).
//!
//! Dragging the border between two header cells resizes the column to its left (#14); double-clicking it restores the
//! default. The widths are remembered in `ui-state.json`.
//!
//! Expanding a row shows pmd's hosted status and Board/work items. Local health is explicitly unavailable.
//! Items open in the browser.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashboard_core::activity::{age_words, clock_time, moment, parse_time};
use dashboard_core::cache::StatusCache;
use dashboard_core::columns::{Column, ColumnWidths};
use dashboard_core::config::{Config, ConfigError, ConfigFile};
use dashboard_core::pmd::{Hosted, WorkItem};
use dashboard_core::priority::{Pmd, PrioritySource};
use dashboard_core::scheduler::{self, Outcome};
use dashboard_core::source::StatusSource;
use dashboard_core::status::{HealthRow, Issue, PullRequest, RunOutcome, Section, StatusDocument, WorkflowRun};
use dashboard_core::t3::{self, Lookup, ThreadFinder};
use dashboard_core::ui_state::UiState;
use dashboard_core::write::{PmdWriter, WriteBackend};
use dashboard_core::{APP_NAME, Board, Card, Overall, SortOrder};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, TitleBar,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, Window, actions, canvas, div, px,
    rems,
};

use crate::manage::{Manage, ManageEvent};

actions!(project_dashboard, [Refresh]);

/// F5 refreshes every card, as in a browser.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("f5", Refresh, None)]);
    crate::manage::bind_keys(cx);
}

/// How often a running refresh is checked for results.
const POLL: Duration = Duration::from_millis(100);
/// How often config.toml is checked for edits: one read of a small file.
const CONFIG_POLL: Duration = Duration::from_secs(2);

/// The banner text for a config file that can't be used.
pub fn config_problem(err: &ConfigError) -> String {
    // TOML's own message ends with a line break; the advice goes on a line of its own after it.
    let err = err.to_string();
    format!(
        "{}
The dashboard keeps showing the last good list and reads the file again when it is saved.",
        err.trim_end().trim_end_matches('.')
    )
}

/// Where the board's settings and cache live, and how it refreshes.
pub struct Options {
    /// The folder for `status-cache.json`; `None` keeps the cache in memory (tests and screenshots).
    pub cache_dir: Option<PathBuf>,
    pub parallelism: usize,
    /// The automatic refresh interval; `None` turns it off.
    pub auto_refresh: Option<Duration>,
    /// A problem with the config file, shown above the cards.
    pub config_problem: Option<String>,
    /// The config file, named in the empty state.
    pub config_path: Option<PathBuf>,
    /// The folder whose `config.toml` is watched and re-read while the app runs (#21); `None` keeps the list given
    /// at startup (the demo, tests and screenshots).
    pub config_dir: Option<PathBuf>,
    /// A fixed "now" for last-activity ages, for tests and screenshots; `None` reads the clock.
    pub now: Option<DateTime<Utc>>,
    /// The column widths to start with, as `ui-state.json` remembers them.
    pub columns: ColumnWidths,
    /// The folder for `ui-state.json`, where resized columns are saved; `None` keeps them in memory.
    pub ui_state_dir: Option<PathBuf>,
    /// Finds the T3 Code thread for each checkout.
    pub threads: Arc<dyn ThreadFinder>,
    pub priorities: Arc<dyn PrioritySource>,
    /// Live pmd settings, for config reload; `None` uses the supplied fixture source.
    pub pmd: Option<Pmd>,
    /// Where the Manage dialogs write. Live, it follows config.toml's `pmd_write_token_path`.
    pub write: Arc<dyn WriteBackend>,
    /// The demo's fake row for the Manage menu's sample dialog; `None` hides it.
    pub sample: Option<Arc<dyn WriteBackend>>,
}

/// A column border being dragged: which column, and where the drag started.
#[derive(Clone, Copy)]
struct Resize {
    column: Column,
    start_x: Pixels,
    start_width: f32,
}

pub struct BoardView {
    pub(crate) selected_view: dashboard_core::config::View,
    pub(crate) board: Board,
    cache: StatusCache,
    /// Caps every refresh, together, at `parallelism` HTTP reads.
    limiter: Arc<scheduler::Limited>,
    /// The cards on the board, shared with running refreshes so they skip removed checkouts.
    wanted: scheduler::Wanted,
    /// Bumped when the refresh interval changes; a timer from an older schedule stops instead of refreshing.
    auto_schedule: u64,
    config_file: Option<ConfigFile>,
    options: Options,
    /// Expanded rows, by checkout, so expansion survives refreshes and re-sorting.
    expanded: HashSet<PathBuf>,
    sort: SortOrder,
    columns: ColumnWidths,
    resizing: Option<Resize>,
    /// The repository column's width as last laid out, which a narrow window can make less than its setting.
    repository_laid_out: Rc<Cell<Option<Pixels>>>,
    /// Each checkout's T3 thread, once looked up.
    threads: HashMap<PathBuf, Lookup>,
    finding_threads: bool,
    priorities_loaded: bool,
    row_baselines: HashMap<PathBuf, StatusDocument>,
    pub(crate) glows: HashMap<PathBuf, (u64, f32)>,
    glow_serial: u64,
    table_scroll: gpui_kit::ScrollHandle,
    /// Another lookup was asked for while one ran; it starts when that one lands, so its result is never stale.
    threads_wanted: bool,
    finding_priorities: bool,
    priorities_wanted: bool,
    priority_generation: u64,
    last_refresh: Option<Instant>,
    #[cfg(test)]
    pending_refreshes: usize,
    focus: FocusHandle,
    /// The Manage menu and its dialogs (#33).
    manage: Entity<Manage>,
    /// The live write settings last applied, to notice an edited `pmd_write_token_path` or `pmd_url`.
    writer: Option<PmdWriter>,
    _manage_events: Subscription,
}

impl BoardView {
    pub fn new(
        checkouts: &[PathBuf],
        cache: StatusCache,
        source: Arc<dyn StatusSource>,
        options: Options,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // One limit for every refresh, so overlapping refreshes never run more than `parallelism` HTTP reads.
        let wanted = scheduler::Wanted::default();
        let limiter = Arc::new(scheduler::Limited::new(source, options.parallelism));
        let columns = options.columns;
        let config_file = options.config_dir.as_deref().map(ConfigFile::new);
        let write = Arc::clone(&options.write);
        let sample = options.sample.clone();
        let config_path = options.config_path.clone();
        let manage = cx.new(|cx| Manage::new(write, sample, config_path, cx));
        // A write that landed shows in the table now, not at the next automatic refresh.
        let manage_events = cx.subscribe(&manage, |view, _, event, cx| match event {
            ManageEvent::Wrote => view.refresh_all(cx),
        });
        let writer = match (&options.pmd, &options.config_dir) {
            (Some(_), Some(dir)) => Config::load(dir)
                .ok()
                .map(|config| PmdWriter::from_config(&config, dir)),
            _ => None,
        };
        let mut view = Self {
            selected_view: options
                .config_dir
                .as_deref()
                .and_then(|d| Config::load(d).ok())
                .map(|c| c.view)
                .unwrap_or_default(),
            board: Board::new(checkouts, &cache),
            cache,
            limiter,
            wanted,
            auto_schedule: 0,
            config_file,
            options,
            expanded: HashSet::new(),
            sort: SortOrder::NewestFirst,
            columns,
            resizing: None,
            repository_laid_out: Rc::new(Cell::new(None)),
            threads: HashMap::new(),
            finding_threads: false,
            priorities_loaded: false,
            row_baselines: HashMap::new(),
            glows: HashMap::new(),
            glow_serial: 0,
            table_scroll: gpui_kit::ScrollHandle::new(),
            threads_wanted: false,
            finding_priorities: false,
            priorities_wanted: false,
            priority_generation: 0,
            last_refresh: None,
            #[cfg(test)]
            pending_refreshes: 0,
            focus,
            manage,
            writer,
            _manage_events: manage_events,
        };
        view.find_threads(cx);
        view.refresh_priorities(cx);
        // Keeps "Updated … ago" and the last-activity ages current between refreshes.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(30)).await;
                let Ok(()) = this.update(cx, |view, cx| {
                    if view.last_refresh.is_some() || view.board.cards().iter().any(|card| card.document.is_some()) {
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();
        if let Some(pmd) = &view.options.pmd {
            view.board.sync_pmd_identities(&pmd.url, &mut view.cache);
        }
        view.sync_wanted();
        view.schedule_auto_refresh(cx);
        // A separate short timer also works with refresh_minutes = 0. Only due, idle cards run.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |view, cx| {
                        let due = view
                            .board
                            .cards()
                            .iter()
                            .filter(|card| !card.refreshing && card.recovery.due(Instant::now()))
                            .map(|card| card.checkout.clone())
                            .collect::<Vec<_>>();
                        if !due.is_empty() {
                            view.refresh_priorities(cx);
                            view.start(due, false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // Edits to config.toml show up within a couple of seconds, without a refresh.
        if view.config_file.is_some() {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(CONFIG_POLL).await;
                    let Ok(()) = this.update(cx, |view, cx| view.check_config(cx)) else {
                        break;
                    };
                }
            })
            .detach();
        }
        view
    }

    #[cfg(test)]
    pub fn board(&self) -> &Board {
        &self.board
    }

    #[cfg(test)]
    pub fn manage(&self) -> &Entity<Manage> {
        &self.manage
    }

    #[cfg(test)]
    pub fn pending_refreshes(&self) -> usize {
        self.pending_refreshes
    }

    #[cfg(test)]
    pub fn cached(&self, checkout: &Path) -> Option<&StatusDocument> {
        self.cache.get(checkout)
    }

    #[cfg(test)]
    pub fn priorities_pending(&self) -> bool {
        self.finding_priorities
    }

    // Only the offscreen screenshot fixtures inject priorities directly.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub fn apply_priorities(&mut self, priorities: dashboard_core::priority::Priorities, cx: &mut Context<Self>) {
        self.set_priorities(priorities, cx);
        cx.notify();
    }

    pub fn is_expanded(&self, checkout: &Path) -> bool {
        self.expanded.contains(checkout)
    }

    #[cfg(test)]
    pub fn sort(&self) -> SortOrder {
        self.sort
    }

    #[cfg(test)]
    pub fn columns(&self) -> ColumnWidths {
        self.columns
    }

    /// Starts dragging `column`'s right border, or restores its default width on a double-click.
    fn start_resize(&mut self, column: Column, event: &MouseDownEvent, window: &Window, cx: &mut Context<Self>) {
        if event.click_count >= 2 {
            self.resizing = None;
            self.columns.reset(column);
            self.save_columns();
            cx.notify();
            return;
        }
        let mut start_width = self.columns.get(column);
        // On a narrow window the repository column is drawn narrower than its setting. Start from what is on screen,
        // so the border follows the pointer from the first pixel.
        if column == Column::Repository
            && let Some(laid_out) = self.repository_laid_out.get()
        {
            start_width = start_width.min(laid_out / window.rem_size());
        }
        self.resizing = Some(Resize {
            column,
            start_x: event.position.x,
            start_width,
        });
    }

    /// Follows the pointer while a border is dragged; widths are in rems, so the drag is measured at the current zoom.
    fn drag_resize(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing else {
            return;
        };
        // The button came up outside the window: the drag is over.
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_resize(cx);
            return;
        }
        let delta = (event.position.x - resize.start_x) / window.rem_size();
        self.columns.set(resize.column, resize.start_width + delta);
        cx.notify();
    }

    /// Saves the widths and repaints, so the handle drops its dragging color.
    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.save_columns();
            cx.notify();
        }
    }

    fn save_columns(&self) {
        let columns = self.columns;
        if let Some(dir) = &self.options.ui_state_dir
            && let Err(err) = UiState::update(dir, |state| state.columns = columns)
        {
            eprintln!("project-dashboard: can't save the column widths: {err}");
        }
    }

    /// The handle on `column`'s right border: it spans the gap to the next column, so it never covers a header's own
    /// click target.
    fn resize_handle(&self, column: Column, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (line, active) = (theme.border, theme.primary);
        let dragging = self.resizing.is_some_and(|resize| resize.column == column);
        div()
            .id(("resize", column as usize))
            .test_support()
            .aria_label(format!("Resize {}", column_title(column)))
            .absolute()
            .top_0()
            .bottom_0()
            .right(-GAP)
            .w(GAP)
            .flex()
            .justify_center()
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.start_resize(column, event, window, cx);
                }),
            )
            .child(
                div()
                    .id(("resize-line", column as usize))
                    .w(px(1.))
                    .h_full()
                    .bg(if dragging { active } else { line })
                    .hover(move |style| style.bg(active)),
            )
    }

    /// A header cell for a resizable column, with the handle on its right border.
    fn header_cell(&self, column: Column, cell: impl IntoElement, cx: &mut Context<Self>) -> impl IntoElement {
        let laid_out = (column == Column::Repository).then(|| Rc::clone(&self.repository_laid_out));
        column_cell(column, &self.columns)
            .relative()
            .child(cell)
            .child(self.resize_handle(column, cx))
            // Records the width the layout gave the column, for a drag that starts on a narrow window.
            .when_some(laid_out, |this, laid_out| {
                this.child(
                    canvas(
                        move |bounds, _, _| laid_out.set(Some(bounds.size.width)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
    }

    /// Moves to the next order: newest updated first, oldest first, then config order.
    pub fn cycle_sort(&mut self, cx: &mut Context<Self>) {
        self.sort = self.sort.next();
        cx.notify();
    }

    pub fn cycle_priority_sort(&mut self, cx: &mut Context<Self>) {
        self.sort = self.sort.next_priority();
        cx.notify();
    }

    /// Priority never occupies a status worker or delays the UI. Coalesce repeated requests and ignore an old config's result.
    pub fn refresh_priorities(&mut self, cx: &mut Context<Self>) {
        if self.finding_priorities {
            self.priorities_wanted = true;
            return;
        }
        self.finding_priorities = true;
        let generation = self.priority_generation;
        let source = Arc::clone(&self.options.priorities);
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(source.fetch_result());
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let priorities = match receive.try_recv() {
                    Ok(priorities) => priorities,
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => Err(dashboard_core::source::FetchError::Unavailable),
                };
                let _ = this.update(cx, |view, cx| {
                    view.finding_priorities = false;
                    if view.priority_generation == generation {
                        match priorities {
                            Ok(priorities) => view.set_priorities(priorities, cx),
                            // Keep displaying last good priorities, but recovery is a new baseline for highlighting.
                            Err(_) => view.priorities_loaded = false,
                        }
                    }
                    if std::mem::take(&mut view.priorities_wanted) {
                        view.refresh_priorities(cx);
                    }
                    cx.notify();
                });
                break;
            }
        })
        .detach();
    }

    /// The checkouts in the order the table shows them.
    #[cfg(test)]
    pub fn row_order(&self) -> Vec<PathBuf> {
        let cards = self.board.cards();
        self.board
            .order(self.sort)
            .into_iter()
            .map(|ix| cards[ix].checkout.clone())
            .collect()
    }

    fn now(&self) -> DateTime<Utc> {
        self.options.now.unwrap_or_else(Utc::now)
    }

    pub fn toggle_expanded(&mut self, checkout: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(checkout) {
            self.expanded.insert(checkout.to_owned());
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn config_problem(&self) -> Option<&str> {
        self.options.config_problem.as_deref()
    }

    /// Shares the board's current cards with running refreshes.
    fn sync_wanted(&self) {
        self.wanted.set(
            self.board
                .cards()
                .iter()
                .map(|card| (card.checkout.clone(), card.generation)),
        );
    }

    /// Starts the automatic-refresh timer for the current interval. An earlier timer sees the schedule moved on and
    /// stops, so a changed `refresh_minutes` applies at once, from now, and 0 turns it off.
    fn schedule_auto_refresh(&mut self, cx: &mut Context<Self>) {
        self.auto_schedule += 1;
        let schedule = self.auto_schedule;
        let Some(interval) = self.options.auto_refresh else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let Ok(current) = this.update(cx, |view, cx| {
                    if view.auto_schedule != schedule {
                        return false;
                    }
                    // An edit saved since the last poll can change the interval: read it first, and refresh only if
                    // this timer is still the current schedule afterwards.
                    view.reload_config(cx);
                    let current = view.auto_schedule == schedule;
                    if current {
                        view.refresh_idle(cx);
                    }
                    current
                }) else {
                    break;
                };
                if !current {
                    break;
                }
            }
        })
        .detach();
    }

    /// Applies config.toml if it changed since it was last read.
    pub fn check_config(&mut self, cx: &mut Context<Self>) {
        self.check_identities(cx);
        if let Some(result) = self.config_file.as_mut().and_then(ConfigFile::changed) {
            self.apply_config(result, cx);
        }
    }

    fn check_identities(&mut self, cx: &mut Context<Self>) {
        if let Some(pmd) = &self.options.pmd {
            let changed = self.board.sync_pmd_identities(&pmd.url, &mut self.cache);
            if !changed.is_empty() {
                self.sync_wanted();
                self.start(changed, false, cx);
            }
        }
    }

    /// Follows the config file: the repository list, parallelism and the refresh interval. A file that can't be read
    /// keeps the last good list and says why in the banner until it is fixed.
    fn apply_config(&mut self, result: Result<Config, ConfigError>, cx: &mut Context<Self>) {
        let config = match result {
            Ok(config) => config,
            Err(err) => {
                self.options.config_problem = Some(config_problem(&err));
                cx.notify();
                return;
            }
        };
        self.selected_view = config.view;
        self.options.config_problem = None;
        if self.options.pmd.is_some() {
            let dir = self
                .options
                .config_dir
                .clone()
                .expect("live pmd has a config directory");
            let dir = dir.as_path();
            let pmd = Pmd::from_config(&config, dir);
            if self.options.pmd.as_ref() != Some(&pmd) {
                self.priority_generation += 1;
                self.options.priorities = Arc::new(pmd.clone());
                self.limiter.set_source(Arc::new(pmd.clone()));
                self.board.replace_pmd_source(&pmd.url, &mut self.cache);
                self.options.pmd = Some(pmd);
                self.sync_wanted();
                self.start(self.board.checkouts(), false, cx);
                self.refresh_priorities(cx);
            }
            let writer = PmdWriter::from_config(&config, dir);
            if self.writer.as_ref() != Some(&writer) {
                let config_path = self.options.config_path.clone();
                let backend: Arc<dyn WriteBackend> = Arc::new(writer.clone());
                self.manage
                    .update(cx, |manage, cx| manage.set_backend(backend, config_path, cx));
                self.writer = Some(writer);
            }
        }
        let before = self.board.cards().len();
        let added = self.board.set_checkouts(&config.repositories, &self.cache);
        self.sync_wanted();
        let present: HashSet<PathBuf> = self.board.checkouts().into_iter().collect();
        self.expanded.retain(|checkout| present.contains(checkout));
        self.threads.retain(|checkout, _| present.contains(checkout));
        self.row_baselines.retain(|checkout, _| present.contains(checkout));
        self.glows.retain(|checkout, _| present.contains(checkout));
        // One limiter for all refreshes, old and new, so a lower limit holds as soon as running fetches finish.
        if config.parallelism != self.options.parallelism {
            self.options.parallelism = config.parallelism;
            self.limiter.set_limit(config.parallelism);
        }
        if config.refresh_interval() != self.options.auto_refresh {
            self.options.auto_refresh = config.refresh_interval();
            self.schedule_auto_refresh(cx);
        }
        let changed = !added.is_empty() || present.len() != before;
        // Removed repositories leave the status cache now, not only after the next refresh.
        if changed && let Some(dir) = &self.options.cache_dir {
            self.cache.retain(&self.board.checkouts());
            if let Err(err) = self.cache.save(dir) {
                eprintln!("project-dashboard: can't save the status cache: {err}");
            }
        }
        if !added.is_empty() {
            self.start(added, false, cx);
        }
        // Any change to the list gets a fresh lookup; if one is running, it runs again after that one lands.
        if changed {
            self.find_threads(cx);
        }
        cx.notify();
    }

    /// Refreshes every card that isn't already refreshing, from pmd's cached state. The config file is
    /// read first, so the refresh covers the current list.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.reload_config(cx);
        self.refresh_idle(cx);
    }

    /// Applies config.toml now if it changed, for a refresh that's about to start.
    fn reload_config(&mut self, cx: &mut Context<Self>) {
        if let Some(result) = self.config_file.as_mut().and_then(ConfigFile::reload) {
            self.apply_config(result, cx);
        }
    }

    /// Refreshes every card that isn't already refreshing.
    fn refresh_idle(&mut self, cx: &mut Context<Self>) {
        self.check_identities(cx);
        self.refresh_priorities(cx);
        let idle: Vec<PathBuf> = self
            .board
            .cards()
            .iter()
            .filter(|card| !card.refreshing)
            .map(|card| card.checkout.clone())
            .collect();
        self.start(idle, false, cx);
    }

    /// Refreshes one card from pmd's cached state.
    pub fn refresh_one(&mut self, checkout: &Path, cx: &mut Context<Self>) {
        self.reload_config(cx);
        self.check_identities(cx);
        self.refresh_priorities(cx);
        if self.board.card(checkout).is_some_and(|card| !card.refreshing) {
            self.start(vec![checkout.to_owned()], true, cx);
        }
    }

    fn start(&mut self, checkouts: Vec<PathBuf>, bypass_cache: bool, cx: &mut Context<Self>) {
        if checkouts.is_empty() {
            return;
        }
        #[cfg(test)]
        {
            self.pending_refreshes += 1;
        }
        let generations = self.board.start_refresh(&checkouts);
        // Work for a checkout removed (or removed and added back) since now is skipped, before it queues for a slot
        // and again once it has one, so it never fetches or holds a slot.
        let source: Arc<dyn StatusSource> = Arc::new(scheduler::OnlyWanted {
            limiter: Arc::clone(&self.limiter),
            wanted: self.wanted.clone(),
            generations: generations.clone(),
        });
        let receiver = scheduler::refresh(source, checkouts, self.options.parallelism, bypass_cache);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let (outcomes, done) = drain(&receiver);
                let alive = this.update(cx, |view, cx| {
                    view.check_identities(cx);
                    for outcome in outcomes {
                        // A card removed and added back while this ran is a new card; this result isn't for it.
                        let Some(&generation) = generations.get(&outcome.checkout) else {
                            continue;
                        };
                        let checkout = outcome.checkout.clone();
                        if view.board.apply_started(outcome, generation, &mut view.cache) {
                            view.observe_row(&checkout, cx);
                            cx.notify();
                        }
                    }
                    if done {
                        #[cfg(test)]
                        {
                            view.pending_refreshes -= 1;
                        }
                        view.finished(cx);
                    }
                });
                if done || alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Shows one refresh result on its card, for the screenshot captures (which only run on Linux).
    #[cfg(test)]
    pub fn apply(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        let checkout = outcome.checkout.clone();
        if self.board.apply(outcome, &mut self.cache) {
            self.observe_row(&checkout, cx);
            cx.notify();
        }
    }

    fn set_priorities(&mut self, priorities: dashboard_core::priority::Priorities, cx: &mut Context<Self>) {
        let before: Vec<_> = self
            .board
            .cards()
            .iter()
            .map(|c| (c.checkout.clone(), self.board.priority(c)))
            .collect();
        self.board.set_priorities(priorities);
        if self.priorities_loaded {
            for (checkout, old) in before {
                if self
                    .board
                    .card(&checkout)
                    .is_some_and(|c| self.board.priority(c) != old)
                {
                    self.glow(checkout, cx);
                }
            }
        }
        self.priorities_loaded = true;
    }

    fn observe_row(&mut self, checkout: &Path, cx: &mut Context<Self>) {
        let Some(card) = self.board.card(checkout) else {
            return;
        };
        if card.error.is_some() || card.document.is_none() {
            // Keep last good data on screen. An outage discards only the highlight baseline, so recovery is quiet.
            self.row_baselines.remove(checkout);
            return;
        }
        let document = card.document.as_ref().expect("checked above");
        let changed = dashboard_core::changes::changed(self.row_baselines.get(checkout), document);
        self.row_baselines.insert(checkout.to_owned(), document.clone());
        if changed {
            self.glow(checkout.to_owned(), cx);
        }
    }

    fn glow(&mut self, checkout: PathBuf, cx: &mut Context<Self>) {
        self.glow_serial += 1;
        let serial = self.glow_serial;
        self.glows.insert(checkout.clone(), (serial, 1.));
        cx.spawn(async move |this, cx| {
            for tick in 1..=50 {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let active = this
                    .update(cx, |view, cx| {
                        if view.glows.get(&checkout).is_none_or(|(id, _)| *id != serial) {
                            return false;
                        }
                        if tick == 50 {
                            view.glows.remove(&checkout);
                        } else {
                            view.glows.insert(checkout.clone(), (serial, 1. - tick as f32 / 50.));
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Looks up every checkout's T3 thread off the UI thread. Asked again while a lookup runs, it runs once more
    /// after that one lands.
    pub fn find_threads(&mut self, cx: &mut Context<Self>) {
        if self.board.is_empty() {
            return;
        }
        if self.finding_threads {
            self.threads_wanted = true;
            return;
        }
        self.finding_threads = true;
        let finder = Arc::clone(&self.options.threads);
        let checkouts = self.board.checkouts();
        cx.spawn(async move |this, cx| {
            let lookup = checkouts.clone();
            let found = cx
                .background_executor()
                .spawn(async move { finder.find(&lookup) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.finding_threads = false;
                // Checkouts removed while the lookup ran don't come back with its answer.
                let present: HashSet<PathBuf> = view.board.checkouts().into_iter().collect();
                let old_threads = view.threads.clone();
                view.threads = checkouts
                    .into_iter()
                    .zip(found)
                    .filter(|(checkout, _)| present.contains(checkout))
                    .collect();
                let changed: Vec<_> = view
                    .threads
                    .iter()
                    .filter(|(path, link)| dashboard_core::changes::session_changed(old_threads.get(*path), link))
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in changed {
                    view.glow(path, cx);
                }
                if std::mem::take(&mut view.threads_wanted) {
                    view.find_threads(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub fn thread(&self, checkout: &Path) -> Option<&Lookup> {
        self.threads.get(checkout)
    }

    fn finished(&mut self, cx: &mut Context<Self>) {
        self.last_refresh = Some(Instant::now());
        // Threads come and go between refreshes; look again whenever the statuses are fresh.
        self.find_threads(cx);
        if let Some(dir) = &self.options.cache_dir {
            self.cache.retain(&self.board.checkouts());
            if let Err(err) = self.cache.save(dir) {
                eprintln!("project-dashboard: can't save the status cache: {err}");
            }
        }
        cx.notify();
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let refreshing = self.board.is_refreshing();
        let transient = self.board.cards().iter().any(|card| card.recovery.retry_at.is_some());
        let last_success = self
            .board
            .cards()
            .iter()
            .filter_map(|card| card.recovery.last_success)
            .max();
        let connecting = !self.board.is_empty()
            && last_success.is_none()
            && self
                .board
                .cards()
                .iter()
                .all(|card| card.error.as_ref().is_none_or(|e| e.is_transient()));
        let status = if connecting {
            "Connecting to pmd…".to_owned()
        } else if transient {
            last_success.map_or_else(
                || "Connecting to pmd…".to_owned(),
                |at| format!("pmd offline · last success {}", at.format("%X")),
            )
        } else {
            match (refreshing, self.last_refresh) {
                (true, _) => "Refreshing…".to_owned(),
                (false, Some(at)) => format!("Updated {}", ago(at.elapsed())),
                (false, None) if self.board.cards().iter().any(|card| card.from_cache) => "Cached".to_owned(),
                (false, None) => String::new(),
            }
        };
        TitleBar::new()
            // The title bar is a fixed 34px; it grows with the zoom so its text isn't clipped.
            .h(crate::zoom::scaled(34., cx).max(px(34.)))
            .child(
                // On a narrow, zoomed window the status gives way first, then the title; icon and button stay.
                // gpui-kit's title bar never shrinks below its content's natural width, so this row is positioned over the
                // bar's area instead of sizing it: the bar keeps the window's width and the row's text truncates within.
                h_flex()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .min_w_0()
                    .pr_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .child(Icon::new(IconName::LayoutDashboard).text_color(cx.theme().primary)),
                    )
                    .child(
                        div()
                            .id("app-title")
                            .test_support()
                            .min_w_0()
                            .flex_shrink(1.)
                            .truncate()
                            .font_semibold()
                            .child(APP_NAME),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .child(self.manage.update(cx, |manage, cx| manage.render_button(cx))),
                    )
                    .child(
                        div()
                            .id("refresh-status")
                            .aria_label(status.clone())
                            .test_support()
                            .min_w_0()
                            .flex_shrink(10.)
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    )
                    .child(
                        // Blocks the drag area beneath, like the Manage button: on Windows a left press on the
                        // caption moves the window instead of clicking.
                        div().flex_none().occlude().child(
                            Button::new("refresh-all")
                                .ghost()
                                .small()
                                .icon(IconName::RefreshCw)
                                .tooltip("Refresh all (F5)")
                                .loading(refreshing)
                                .on_click(cx.listener(|view, _, _, cx| view.refresh_all(cx))),
                        ),
                    ),
            )
    }

    fn render_banner(&self, icon: IconName, text: String, cx: &App) -> impl IntoElement {
        h_flex()
            .id("banner")
            .test_support()
            .mx_4()
            .mt_4()
            .p_3()
            .gap_2()
            .items_start()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().warning)
            .bg(cx.theme().warning.opacity(0.08))
            .child(Icon::new(icon).text_color(cx.theme().warning))
            .child(div().flex_1().min_w_0().text_sm().child(text))
    }

    fn render_empty(&self, cx: &App) -> impl IntoElement {
        let empty = dashboard_core::EmptyState::NO_REPOSITORIES;
        let hint = match &self.options.config_path {
            Some(path) => format!("{} ({})", empty.hint, path.display()),
            None => empty.hint.to_owned(),
        };
        v_flex()
            .id("empty-state")
            .test_support()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::FolderOpen)
                    .size_8()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(div().text_lg().font_semibold().child(empty.title))
            // Kept off the window's edges and centered when it wraps, as on a narrow window.
            .child(
                div()
                    .max_w(rems(40.))
                    .px_8()
                    .text_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> gpui_kit::Stateful<gpui_kit::Div> {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let (arrow, sort_label) = match self.sort {
            SortOrder::Config | SortOrder::PriorityFirst | SortOrder::PriorityLast => ("", "Updated, not sorted"),
            SortOrder::NewestFirst => ("↓", "Updated, newest first"),
            SortOrder::OldestFirst => ("↑", "Updated, oldest first"),
        };
        let sorted = matches!(self.sort, SortOrder::NewestFirst | SortOrder::OldestFirst);
        let (priority_arrow, priority_label) = match self.sort {
            SortOrder::PriorityFirst => ("↑", "Priority, highest first"),
            SortOrder::PriorityLast => ("↓", "Priority, lowest first"),
            _ => ("", "Priority, not sorted"),
        };
        h_flex()
            .id("table-header")
            .px_2()
            .py_1()
            .gap(GAP)
            .items_center()
            .text_xs()
            .font_semibold()
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(CHEVRON).flex_none())
            .child(div().w(STATE).flex_none())
            .child(
                self.header_cell(
                    Column::Repository,
                    div()
                        .id("header-repository")
                        .test_support()
                        .truncate()
                        .child(column_title(Column::Repository)),
                    cx,
                ),
            )
            .child(
                self.header_cell(
                    Column::Priority,
                    h_flex()
                        .id("sort-priority")
                        .test_support()
                        .aria_label(priority_label)
                        .size_full()
                        .gap_1()
                        .cursor_pointer()
                        .when(!priority_arrow.is_empty(), |this| this.text_color(foreground))
                        .hover(move |style| style.text_color(foreground))
                        .on_click(cx.listener(|view, _, _, cx| view.cycle_priority_sort(cx)))
                        .child(column_title(Column::Priority))
                        .child(priority_arrow),
                    cx,
                ),
            )
            .child(self.header_cell(
                Column::PullRequests,
                div().truncate().text_right().child(column_title(Column::PullRequests)),
                cx,
            ))
            .child(self.header_cell(
                Column::Issues,
                div().truncate().text_right().child(column_title(Column::Issues)),
                cx,
            ))
            .child(self.header_cell(Column::Merged24h, div().truncate().text_right().child("Merged 24h"), cx))
            .child(self.header_cell(Column::Merged7d, div().truncate().text_right().child("Merged 7d"), cx))
            .child(
                self.header_cell(
                    Column::Updated,
                    h_flex()
                        .id("sort-updated")
                        .test_support()
                        .aria_label(sort_label)
                        .size_full()
                        .gap_1()
                        .cursor_pointer()
                        .when(sorted, |this| this.text_color(foreground))
                        .hover(move |style| style.text_color(foreground))
                        .on_click(cx.listener(|view, _, _, cx| view.cycle_sort(cx)))
                        .child(column_title(Column::Updated))
                        .child(arrow),
                    cx,
                ),
            )
            .child(div().flex_1().min_w(ACTIVITY_MIN).truncate().child("Last activity"))
    }

    fn render_row(&self, ix: usize, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let expanded = self.is_expanded(&card.checkout);
        let overall = card.overall();
        let checkout = card.checkout.clone();
        let toggle = card.checkout.clone();
        let toggle_chevron = card.checkout.clone();
        let now = self.now();
        let count = |section: &str, len: fn(&StatusDocument) -> usize| -> String {
            card.document.as_ref().map_or_else(
                || "–".to_owned(),
                |document| section_count(document, section, len(document)),
            )
        };
        let open_counts = card
            .document
            .as_ref()
            .and_then(|doc| doc.hosted.as_ref())
            .map(|hosted| &hosted.open_counts);
        let prs = open_counts.and_then(|counts| counts.open_prs).map_or_else(
            || count("pullRequests.open", |document| document.pull_requests.open.len()),
            |n| n.to_string(),
        );
        let issues = open_counts
            .and_then(|counts| counts.open_issues)
            .map_or_else(|| count("issues", |document| document.issues.len()), |n| n.to_string());
        let error = card.display_error().filter(|error| !error.is_setup());
        // pmd regenerates a status only when the repository changes, and confirms it on every sweep: show both, each
        // with its local time on hover.
        let freshness = card
            .document
            .as_ref()
            .and_then(|doc| doc.hosted.as_ref())
            .map(|hosted| freshness(hosted, now));
        // Found once per row: both the Updated and Last activity columns show it.
        let last_activity = card.last_activity();
        // The age, with the exact local time on hover; a dash until there is something to date.
        let updated: AnyElement = match &last_activity {
            Some(activity) => {
                let age = activity.age(now);
                let time: SharedString = activity.local_time().into();
                div()
                    .id(("updated", ix))
                    .test_support()
                    .aria_label(format!("Updated {age}"))
                    .truncate()
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(time.clone()).build(window, cx)
                    })
                    .child(age)
                    .into_any_element()
            }
            None => div()
                .id(("updated", ix))
                .test_support()
                .aria_label("Updated: unknown")
                .text_color(theme.muted_foreground)
                .child("–")
                .into_any_element(),
        };
        let activity: AnyElement = match (last_activity, error) {
            // A failed refresh says so in the activity column; the counts keep the last good status.
            (_, Some(error)) => div()
                .id(("row-error", ix))
                .test_support()
                .aria_label(error.to_string())
                .truncate()
                .text_color(theme.danger)
                .child(error.to_string())
                .into_any_element(),
            (Some(activity), None) => {
                let url = activity.url.clone();
                h_flex()
                    .id(("activity", ix))
                    .test_support()
                    .aria_label(format!("Last activity: {}, {}", activity.label, activity.title))
                    .tooltip({
                        let text = format!("{}: {}", activity.label, activity.title);
                        move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                    })
                    .min_w_0()
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    })
                    // Workflow names can be longer than their titles. Let the label use its natural width up to
                    // a third of this cell. At its minimum, keep a label prefix instead of two bare ellipses.
                    .child(
                        div()
                            .id(("activity-summary", ix))
                            .test_support()
                            .min_w_0()
                            .when(!activity.title.is_empty(), |this| {
                                this.min_w(ACTIVITY_MIN).max_w(gpui_kit::relative(1. / 3.))
                            })
                            .truncate()
                            .child(activity.label.clone()),
                    )
                    .child(
                        div()
                            .id(("activity-title", ix))
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            // The gap shrinks with the description, so the minimum label always fits the cell.
                            .child(div().pl_2().truncate().child(activity.title.clone())),
                    )
                    .into_any_element()
            }
            // A setup problem (missing or rejected pmd token) is explained once in the banner; the row just says it has nothing.
            (None, None) if card.document.is_none() && card.error.is_some() => div()
                .id(("activity-placeholder", ix))
                .test_support()
                .aria_label("Unavailable")
                .truncate()
                .text_color(theme.muted_foreground)
                .child("Unavailable")
                .into_any_element(),
            (None, None) if card.document.is_none() => div()
                .id(("activity-placeholder", ix))
                .test_support()
                .aria_label("Loading")
                .truncate()
                .text_color(theme.muted_foreground)
                .child("Loading…")
                .into_any_element(),
            (None, None) => div()
                .truncate()
                .text_color(theme.muted_foreground)
                .child("No recent activity")
                .into_any_element(),
        };
        let row = h_flex()
            .id(("row", ix))
            .test_support()
            .px_2()
            .py_1p5()
            .gap(GAP)
            .items_center()
            .text_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme.list_hover))
            .when(expanded && !self.glows.contains_key(&card.checkout), |this| {
                this.bg(theme.list_active)
            })
            .on_click(cx.listener(move |view, _, _, cx| view.toggle_expanded(&toggle, cx)))
            .child(
                div().w(CHEVRON).flex_none().child(
                    Button::new(("expand", ix))
                        .ghost()
                        .xsmall()
                        .icon(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .tooltip(if expanded { "Collapse" } else { "Show details" })
                        .on_click(cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            view.toggle_expanded(&toggle_chevron, cx)
                        })),
                ),
            )
            .child(
                div()
                    .w(STATE)
                    .flex_none()
                    .child(state_dot(ix, overall, card.signals(), cx)),
            )
            .child(
                column_cell(Column::Repository, &self.columns)
                    .id(("name", ix))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .id(("repo-full", ix))
                                    .truncate()
                                    .font_semibold()
                                    .tooltip({
                                        let name = card.name().to_owned();
                                        move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx)
                                        }
                                    })
                                    .child(card.name()),
                            )
                            .when_some(freshness, |this, (text, times)| {
                                let times: SharedString = times.into();
                                this.child(
                                    div()
                                        .id(("pmd-age", ix))
                                        .test_support()
                                        .aria_label(text.clone())
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .tooltip(move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(times.clone()).build(window, cx)
                                        })
                                        .child(text),
                                )
                            }),
                    )
                    // Stays while a refresh runs: until a fresh result lands, the row still shows the cached copy.
                    .when(
                        card.from_cache
                            && (card.display_error().is_some()
                                || !card.error.as_ref().is_some_and(|e| e.is_transient())),
                        |this| {
                            this.child(
                                div()
                                    .id(("cached", ix))
                                    .test_support()
                                    .flex_none()
                                    .child(Tag::secondary().xsmall().outline().child("Cached")),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .id(("priority", ix))
                    .test_support()
                    .aria_label(
                        self.board
                            .priority(card)
                            .map_or_else(|| "Priority: unknown".to_owned(), |p| format!("Priority P{p}")),
                    )
                    .w(width(Column::Priority, &self.columns))
                    .flex_none()
                    .truncate()
                    .child(
                        self.board
                            .priority(card)
                            .map_or_else(|| "—".to_owned(), |p| format!("P{p}")),
                    ),
            )
            .child(
                div()
                    .id(("prs", ix))
                    .test_support()
                    .aria_label(format!("{prs} open pull requests"))
                    .w(width(Column::PullRequests, &self.columns))
                    .flex_none()
                    .truncate()
                    .text_right()
                    .child(prs),
            )
            .child(
                div()
                    .id(("issues", ix))
                    .test_support()
                    .aria_label(format!("{issues} open issues"))
                    .w(width(Column::Issues, &self.columns))
                    .flex_none()
                    .truncate()
                    .text_right()
                    .child(issues),
            )
            .children([Column::Merged24h, Column::Merged7d].map(|column| {
                let counts = card
                    .document
                    .as_ref()
                    .and_then(|doc| doc.hosted.as_ref())
                    .map(|hosted| &hosted.merge_counts);
                let count = counts.and_then(|counts| match column {
                    Column::Merged24h => counts.merged_24h,
                    _ => counts.merged_7d,
                });
                let text = count.map_or_else(|| "—".to_owned(), |n| n.to_string());
                column_cell(column, &self.columns)
                    .id((
                        match column {
                            Column::Merged24h => "merged24h",
                            _ => "merged7d",
                        },
                        ix,
                    ))
                    .test_support()
                    .aria_label(format!("{}: {}", column_title(column), text))
                    .text_right()
                    .child(text)
            }))
            .child(column_cell(Column::Updated, &self.columns).truncate().child(updated))
            // Takes the width the other columns leave, and shrinks first on a narrow window, so the refresh button
            // stays in view.
            .child(
                div()
                    .id(("activity-cell", ix))
                    .test_support()
                    .flex_1()
                    .min_w(ACTIVITY_MIN)
                    .child(activity),
            );
        let row = h_flex()
            .items_center()
            .child(
                div()
                    .id(("row-scroll", ix))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.table_scroll)
                    .child(row.min_w(table_min(&self.columns))),
            )
            .child(
                h_flex()
                    .w(ACTIONS)
                    .px_2()
                    .gap_1()
                    .flex_none()
                    .justify_end()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.thread_button(ix, &card.checkout))
                    .child(
                        Button::new(("refresh", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCw)
                            .tooltip("Reload this repository from pmd")
                            .loading(card.refreshing)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.refresh_one(&checkout, cx)
                            })),
                    ),
            );
        let theme = cx.theme();
        v_flex()
            .id(("entry", ix))
            .test_support()
            .when(self.glows.contains_key(&card.checkout), |this| {
                let progress = self.glows[&card.checkout].1;
                let opacity = if cx.reduce_motion() { 0.16 } else { 0.22 * progress };
                this.bg(theme.primary.opacity(opacity))
            })
            .border_b_1()
            .border_color(theme.border)
            .child(row)
            .when(expanded, |this| {
                this.child(
                    div()
                        .id(("details-scroll", ix))
                        .test_support()
                        .min_w_0()
                        .overflow_x_scroll()
                        .restrict_scroll_to_axis()
                        .child(self.render_details(ix, card, cx)),
                )
            })
    }

    /// Opens the checkout's T3 Code thread; disabled, with the reason as its tooltip, when there is none to open.
    fn thread_button(&self, ix: usize, checkout: &Path) -> impl IntoElement + use<> {
        let lookup = self.threads.get(checkout);
        let tooltip = lookup.map_or_else(|| "Looking for this checkout's T3 Code thread…".to_owned(), t3::tooltip);
        let url = lookup
            .and_then(|lookup| lookup.as_ref().ok())
            .map(|link| link.url.clone());
        Button::new(("t3", ix))
            .ghost()
            .xsmall()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .child(
                div().min_w_0().truncate().child(
                    lookup
                        .and_then(|l| l.as_ref().ok())
                        .map_or("T3 session", |l| l.title.as_str())
                        .to_owned(),
                ),
            )
            .tooltip(url.as_ref().map_or(tooltip.clone(), |url| format!("{tooltip}\n{url}")))
            .disabled(url.is_none())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                if let Some(url) = &url {
                    cx.open_url(url);
                }
            })
    }

    /// The expanded part of a row: the error, the health rows, unavailable sections and the four tables.
    fn render_details(&self, ix: usize, card: &Card, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let mut body = v_flex()
            .id(("details", ix))
            .test_support()
            // Fixed detail cells need room for checks, reviews, ages and a readable title. Scroll their own
            // viewport independently of the pinned session lane and the resizable Project columns.
            .min_w(rems(48.))
            .pl(rems(3.25))
            .pr_4()
            .pt_1()
            .pb_3()
            .gap_1();
        if let Some(error) = card.display_error().filter(|error| !error.is_setup()) {
            body = body.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(Icon::new(IconName::CircleX).small())
                    .child(div().flex_1().min_w_0().child(error.to_string())),
            );
        }
        match &card.document {
            Some(document) => {
                if let Some(error) = document.hosted.as_ref().and_then(|hosted| hosted.work_error.as_ref()) {
                    body = body.child(
                        div()
                            .id(("pmd-work-error", ix))
                            .test_support()
                            .text_sm()
                            .text_color(theme.warning)
                            .child(
                                if document.hosted.as_ref().is_some_and(|hosted| hosted.board.is_some()) {
                                    format!("Board/work refresh failed: {error} Showing last good Board/work below.")
                                } else {
                                    format!("Board/work unavailable: {error}")
                                },
                            ),
                    );
                }
                body = body.children(
                    document
                        .health_rows()
                        .iter()
                        .filter(|_| document.hosted.is_none())
                        .map(|row| health_row(row, cx)),
                );
                body = body.children(document.unavailable_sections().map(|section| {
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(Icon::new(IconName::TriangleAlert).small())
                        .child(format!(
                            "{} unavailable: {}",
                            section_label(&section.name),
                            section.reason
                        ))
                }));
                body = body.child(tables(ix, document, cx));
                if let Some(hosted) = &document.hosted {
                    body = body.child(about_pmd(ix, hosted, self.now(), cx));
                }
            }
            None if card.error.is_none() => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading pmd status…"),
                );
            }
            None => {}
        }
        body
    }
}

// Table column widths, in rems so they follow the text size. The resizable columns' widths are in `ColumnWidths`.
const CHEVRON: gpui_kit::Rems = rems(1.75);
const STATE: gpui_kit::Rems = rems(0.75);
/// Fixed session lane, outside the data columns’ horizontal scroll area.
const ACTIONS: gpui_kit::Rems = rems(11.);
/// The space between columns.
const GAP: gpui_kit::Rems = rems(0.5);
/// Extra space before the update time, so it doesn't read as part of the right-aligned issue count before it.
const UPDATED_LEAD: gpui_kit::Rems = rems(0.75);
/// Last activity takes whatever width is left, down to this: on the narrowest window it is little more than an
/// ellipsis, so the names and buttons keep their room.
const ACTIVITY_MIN: gpui_kit::Rems = rems(1.5);

fn width(column: Column, widths: &ColumnWidths) -> gpui_kit::Rems {
    rems(widths.get(column))
}

/// A resizable column's cell. The repository column gives way on a narrow window, down to its minimum, after last
/// activity has; the others keep the width they were given.
fn column_cell(column: Column, widths: &ColumnWidths) -> gpui_kit::Div {
    let cell = div().min_w_0();
    match column {
        Column::Repository => cell
            .flex_basis(width(column, widths))
            .flex_grow(0.)
            .flex_shrink(1.)
            .min_w(rems(column.min_width())),
        Column::Updated => cell.ml(UPDATED_LEAD).w(width(column, widths)).flex_none(),
        _ => cell.w(width(column, widths)).flex_none(),
    }
}

fn column_title(column: Column) -> &'static str {
    match column {
        Column::Repository => "Repository",
        Column::Priority => "Priority",
        Column::PullRequests => "PRs",
        Column::Issues => "Issues",
        Column::Merged24h => "Merged 24h",
        Column::Merged7d => "Merged 7d",
        Column::Updated => "Updated",
    }
}

/// Data columns keep their saved widths and scroll together, independently of the fixed session lane.
fn table_min(widths: &ColumnWidths) -> gpui_kit::Rems {
    let resizable = widths.total() - widths.get(Column::Repository) + Column::Repository.min_width();
    rems(1.75 + 0.75 + resizable + 0.75 + 1.5 + 10. * 0.5 + 2. * 0.5)
}

impl Render for BoardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = v_flex().id("board").flex_1().min_h_0();
        content = content.child(
            h_flex().px_3().py_2().gap_2().children(
                [
                    ("Project", dashboard_core::config::View::Project),
                    ("Backlog", dashboard_core::config::View::Backlog),
                ]
                .into_iter()
                .map(|(name, choice)| {
                    div().occlude().child(
                        Button::new(name)
                            .small()
                            .label(name)
                            .selected(self.selected_view == choice)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if let Some(dir) = &view.options.config_dir
                                    && let Err(error) = Config::save_view(dir, choice)
                                {
                                    view.options.config_problem = Some(error.to_string());
                                }
                                view.selected_view = choice;
                                cx.notify();
                            })),
                    )
                }),
            ),
        );
        if let Some(problem) = &self.options.config_problem {
            content = content.child(self.render_banner(IconName::TriangleAlert, problem.clone(), cx));
        }
        if let Some(problem) = self.board.setup_problem() {
            content = content.child(self.render_banner(IconName::CircleAlert, problem.to_string(), cx));
        }
        if self.selected_view == dashboard_core::config::View::Backlog {
            content = content.child(self.render_backlog(cx));
        } else if self.board.is_empty() {
            content = content.child(self.render_empty(cx));
        } else {
            let cards = self.board.cards().to_vec();
            let rows: Vec<AnyElement> = self
                .board
                .order(self.sort)
                .into_iter()
                .map(|ix| self.render_row(ix, &cards[ix], cx).into_any_element())
                .collect();
            content = content.child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .child(
                                div()
                                    .id("table")
                                    .test_support()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_x_scroll()
                                    .track_scroll(&self.table_scroll)
                                    .child(self.render_header(cx).min_w(table_min(&self.columns))),
                            )
                            .child(div().w(ACTIONS).flex_none().px_2().text_xs().child("T3 session")),
                    )
                    .child(
                        v_flex()
                            .id("rows")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .restrict_scroll_to_axis()
                            .children(rows),
                    ),
            );
        }
        v_flex()
            .id("dashboard")
            .key_context("Dashboard")
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &Refresh, _, cx| view.refresh_all(cx)))
            // Alt+M opens the Manage menu from anywhere; the button handles it first when it has focus.
            .on_action(cx.listener(|view, _: &crate::manage::OpenMenu, window, cx| {
                view.manage.update(cx, |manage, cx| manage.open_menu(window, cx))
            }))
            // A column border drag follows the pointer anywhere in the window.
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| view.drag_resize(event, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|view, _, _, cx| view.end_resize(cx)))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .relative()
            .child(self.render_title_bar(cx))
            .child(content)
            // Ctrl+wheel zooms; registered each frame in the capture phase, ahead of the table's scrolling.
            .child(
                canvas(|_, _, _| {}, |_, _, window, _| crate::zoom::capture_wheel(window))
                    .absolute()
                    .size_0(),
            )
            .when_some(crate::zoom::toast(cx), |this, label| this.child(zoom_toast(label, cx)))
            // The Manage menu or dialog, over everything else. It sits inside this element, so F5 still reaches the
            // refresh action above while it is open.
            .child(self.manage.clone())
    }
}

/// The zoom level, briefly, after it changes.
fn zoom_toast(label: String, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .absolute()
        // At the bottom, clear of the table's header and first rows.
        .bottom(rems(1.5))
        .left_0()
        .right_0()
        .justify_center()
        .child(
            div()
                .id("zoom-toast")
                .test_support()
                .aria_label(label.clone())
                .px_3()
                .py_1()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_sm()
                .font_semibold()
                .child(label),
        )
}

/// Everything the receiver holds right now, and whether the refresh is complete.
fn drain(receiver: &Receiver<Outcome>) -> (Vec<Outcome>, bool) {
    let mut outcomes = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(outcome) => outcomes.push(outcome),
            Err(TryRecvError::Empty) => return (outcomes, false),
            Err(TryRecvError::Disconnected) => return (outcomes, true),
        }
    }
}

fn ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

/// The row's pmd ages, "checked 12m ago · changed 5h 23m ago", and their local times for the tooltip.
fn freshness(hosted: &Hosted, now: DateTime<Utc>) -> (String, String) {
    let changed = hosted.changed();
    let checked = hosted.checked();
    let text = match (checked, changed) {
        (Some(checked), Some(changed)) => format!(
            "checked {} · changed {}",
            age_words(checked, now),
            age_words(changed, now)
        ),
        (None, Some(changed)) => format!("changed {}", age_words(changed, now)),
        _ => "pmd status age unknown".to_owned(),
    };
    let times = [
        checked.map(|at| format!("pmd last checked {}", clock_time(at, now))),
        changed.map(|at| format!("status last changed {}", clock_time(at, now))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let times = if times.is_empty() { text.clone() } else { times };
    (text, times)
}

/// Where the details came from, at the bottom: when pmd last checked and changed this status, when its Board was
/// built, and the local health pmd doesn't supply.
fn about_pmd(ix: usize, hosted: &Hosted, now: DateTime<Utc>, cx: &App) -> impl IntoElement + use<> {
    let when = |at: Option<DateTime<Utc>>| at.map_or_else(|| "unknown".to_owned(), |at| moment(at, now));
    let mut lines = vec![format!("Status changed {}", when(hosted.changed()))];
    if let Some(checked) = hosted.checked() {
        lines.push(format!("pmd last checked {}", moment(checked, now)));
    }
    if let Some(board) = &hosted.board {
        lines.push(format!("Board as of {}", when(parse_time(&board.generated_at))));
    }
    v_flex()
        .id(("pmd-freshness", ix))
        .test_support()
        .aria_label(lines.join(" · "))
        .pt_3()
        .gap_0p5()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(lines.join(" · "))
        .child(
            div()
                .id(("local-health", ix))
                .test_support()
                .child("Local branch, worktree and stash health: not supplied by pmd"),
        )
}

/// pmd's work statuses in plain words. "Ready" means open and not started, so it says that.
fn plain_status(status: &str) -> &str {
    match status {
        "ready" => "Not started",
        other => dashboard_core::work::status_label(other),
    }
}

/// The color a work status stands out in, if any: red for blocked, amber for waiting on Franz.
fn status_color(status: &str, cx: &App) -> Option<Hsla> {
    match status {
        "blocked" => Some(cx.theme().danger),
        "waiting_on_franz" => Some(cx.theme().yellow),
        _ => None,
    }
}

/// The row's overall state as a colored dot, with its name for tests and screen readers, and every signal behind it
/// on hover.
fn state_dot(ix: usize, overall: Overall, signals: Option<dashboard_core::Signals>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (color, label) = match overall {
        Overall::Loading => (theme.muted_foreground, "Loading"),
        Overall::Clean => (theme.success, "OK"),
        Overall::Attention => (theme.warning, "Attention"),
        Overall::Failed => (theme.danger, "Refresh failed"),
        Overall::Unavailable => (theme.muted_foreground, "Board unavailable"),
        Overall::Blocked => (theme.danger, "Blocked"),
        Overall::WaitingOnFranz => (theme.yellow, "Waiting on Franz"),
        Overall::ChecksFailing => (theme.warning, "Checks failing"),
    };
    let hosted = signals.is_some();
    let details = signals.map(|signals| signals.describe()).unwrap_or_default();
    let tooltip: SharedString = match overall {
        Overall::Clean if hosted => "OK: nothing blocked, waiting on Franz or failing".into(),
        _ if details.is_empty() => label.into(),
        Overall::Failed => format!("Refresh failed · {}", details.join(" · ")).into(),
        _ => details.join(" · ").into(),
    };
    div()
        .id(SharedString::from(format!("state-{ix}-{overall:?}")))
        .test_support()
        // The color alone doesn't reach screen readers.
        .aria_label(format!("State: {label}"))
        .size(rems(0.625))
        .rounded_full()
        .bg(color)
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
}

/// Keep the count of loaded rows even when enrichment failed. An empty unavailable list is unknown ("?"),
/// and a section turned off for this run reads "–".
pub(crate) fn section_count(document: &StatusDocument, section: &str, len: usize) -> String {
    match document.section(section) {
        Some(section) if section.is_unavailable() && len == 0 => "?".to_owned(),
        Some(section) if section.is_disabled() => "–".to_owned(),
        _ => len.to_string(),
    }
}

fn section_label(name: &str) -> &str {
    match name {
        "issues" => "Open issues",
        "pullRequests.open" => "Open pull requests",
        "pullRequests.merged" => "Merged pull requests",
        "workflowRuns" => "Workflow runs",
        other => other,
    }
}

fn health_row(row: &HealthRow, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (marker, color): (&str, Hsla) = match (row.unavailable, row.clean) {
        (true, _) => ("!", theme.warning),
        (false, true) => ("✓", theme.success),
        (false, false) => ("•", theme.warning),
    };
    h_flex()
        .id(SharedString::from(format!("health-{}", row.label)))
        .test_support()
        .aria_label(format!("{}: {}", row.label, row.text))
        .gap_2()
        .text_sm()
        .child(div().w(rems(0.875)).text_color(color).child(marker))
        .child(
            div()
                .w(rems(5.25))
                .text_color(theme.muted_foreground)
                .child(row.label.clone()),
        )
        .child(div().flex_1().truncate().child(row.text.clone()))
}

/// One table: a heading with its count, then clickable rows or "None". An unavailable section keeps any loaded rows
/// alongside gh-x's diagnostic. Only an empty unavailable list is unknown ("?"). A section turned off reads "off".
/// `tone` colors a heading that must stand out, such as Blocked.
fn table(
    title: &'static str,
    count: usize,
    section: Option<&Section>,
    rows: Vec<AnyElement>,
    tone: Option<Hsla>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let unavailable = section.filter(|section| section.is_unavailable());
    let disabled = section.is_some_and(Section::is_disabled);
    let heading = match (unavailable, disabled) {
        (Some(_), _) if count == 0 => format!("{title} (?)"),
        (Some(_), _) => format!("{title} ({count})"),
        (None, true) => format!("{title} (off)"),
        (None, false) => format!("{title} ({count})"),
    };
    let note = |id: String, text: String, color| {
        vec![
            div()
                .id(SharedString::from(id))
                .test_support()
                .aria_label(text.clone())
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element(),
        ]
    };
    let body: Vec<AnyElement> = match (unavailable, disabled) {
        (Some(section), _) => {
            let mut body = note(
                format!("unavailable-{title}"),
                format!("Unavailable: {}", section.reason),
                theme.warning,
            );
            body.extend(rows);
            body
        }
        (None, true) => note(
            format!("disabled-{title}"),
            "Turned off for this run".to_owned(),
            theme.muted_foreground,
        ),
        (None, false) if rows.is_empty() => vec![
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("None")
                .into_any_element(),
        ],
        (None, false) => rows,
    };
    v_flex()
        .gap_0p5()
        .child(
            div()
                .id(SharedString::from(format!("heading-{title}")))
                .test_support()
                .aria_label(heading.clone())
                .pb_0p5()
                .mb_0p5()
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .font_semibold()
                .text_color(tone.unwrap_or(theme.foreground))
                .child(heading),
        )
        .children(body)
}

/// A row that opens `url` in the default browser.
fn link_row(id: SharedString, url: &str, cells: Vec<AnyElement>, cx: &App) -> AnyElement {
    let url = url.to_owned();
    let hover = cx.theme().list_hover;
    h_flex()
        .id(id)
        .test_support()
        .gap_3()
        .px_1()
        .text_sm()
        .rounded_sm()
        .when(!url.is_empty(), |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(move |_, _, cx| cx.open_url(&url))
        })
        .children(cells)
        .into_any_element()
}

fn cell(text: impl Into<SharedString>, width: f32) -> AnyElement {
    div()
        .w(rems(width / 16.))
        .flex_none()
        .truncate()
        .child(text.into())
        .into_any_element()
}

fn grow(text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .child(text.into())
        .into_any_element()
}

fn muted(text: impl Into<SharedString>, width: f32, cx: &App) -> AnyElement {
    div()
        .w(rems(width / 16.))
        .flex_none()
        .truncate()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// gh-x's checks verdict in words, colored by meaning. Unknown values stay visible as they are.
fn checks_cell(checks: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (text, color) = match checks {
        "pass" => ("CI passing".to_owned(), theme.success),
        "fail" => ("CI failing".to_owned(), theme.danger),
        "pending" => ("CI running".to_owned(), theme.yellow),
        "-" | "" => ("No CI".to_owned(), theme.muted_foreground),
        other => (format!("CI {other}"), theme.muted_foreground),
    };
    div()
        .w(rems(84. / 16.))
        .flex_none()
        .truncate()
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// gh-x's review verdict in words.
fn review_words(pr: &PullRequest) -> String {
    match pr.review.as_str() {
        "approved" if pr.approvals > 1 => format!("Approved ×{}", pr.approvals),
        "approved" => "Approved".to_owned(),
        "changes" => "Changes requested".to_owned(),
        "review" => "Needs review".to_owned(),
        "-" | "" => "No review yet".to_owned(),
        other => other.to_owned(),
    }
}

/// gh-x's AI review verdict in words.
fn ai_words(ai_review: &str) -> String {
    match ai_review {
        "pass" => "AI: clean".to_owned(),
        "fail" => "AI: findings".to_owned(),
        "pending" => "AI: running".to_owned(),
        "-" | "" => String::new(),
        other => format!("AI: {other}"),
    }
}

/// A tag for an issue or PR row: Draft, or pmd's work status in plain words, colored when it must stand out.
fn tag_cell(text: String, color: Option<Hsla>, cx: &App) -> AnyElement {
    div()
        .w(rems(120. / 16.))
        .flex_none()
        .truncate()
        .text_color(color.unwrap_or(cx.theme().muted_foreground))
        .child(text)
        .into_any_element()
}

fn work_tag(hosted: Option<&Hosted>, kind: &str, number: u64, cx: &App) -> (String, Option<Hsla>) {
    hosted
        .and_then(|hosted| hosted.item_for(kind, number))
        .map_or((String::new(), None), |item| {
            (plain_status(&item.status).to_owned(), status_color(&item.status, cx))
        })
}

fn pr_rows(kind: &str, ix: usize, prs: &[PullRequest], hosted: Option<&Hosted>, cx: &App) -> Vec<AnyElement> {
    prs.iter()
        .map(|pr| {
            let (tag, color) = if pr.state == "draft" {
                ("Draft".to_owned(), None)
            } else {
                work_tag(hosted, "pr", pr.number, cx)
            };
            link_row(
                format!("{kind}-{ix}-{}", pr.number).into(),
                &pr.url,
                vec![
                    cell(format!("#{}", pr.number), 52.),
                    grow(pr.title.clone()),
                    tag_cell(tag, color, cx),
                    checks_cell(&pr.checks, cx),
                    muted(review_words(pr), 130., cx),
                    muted(ai_words(&pr.ai_review), 84., cx),
                    muted(pr.updated.clone(), 40., cx),
                ],
                cx,
            )
        })
        .collect()
}

fn issue_rows(ix: usize, issues: &[Issue], hosted: Option<&Hosted>, cx: &App) -> Vec<AnyElement> {
    issues
        .iter()
        .map(|issue| {
            let (tag, color) = work_tag(hosted, "issue", issue.number, cx);
            let linked = match issue.pull_requests.as_str() {
                "-" | "" => String::new(),
                prs => format!("PR {prs}"),
            };
            link_row(
                format!("issue-{ix}-{}", issue.number).into(),
                &issue.url,
                vec![
                    cell(format!("#{}", issue.number), 52.),
                    grow(issue.title.clone()),
                    tag_cell(tag, color, cx),
                    muted(linked, 60., cx),
                    muted(issue.labels.clone(), 120., cx),
                    muted(issue.updated.clone(), 40., cx),
                ],
                cx,
            )
        })
        .collect()
}

/// pmd work items: "Issue #31" or "PR #3", and the title.
fn work_rows(ix: usize, items: &[&WorkItem], cx: &App) -> Vec<AnyElement> {
    items
        .iter()
        .map(|item| {
            let kind = match item.kind.as_str() {
                "pr" => "PR",
                "issue" => "Issue",
                other => other,
            };
            link_row(
                SharedString::from(format!("work-{ix}-{}", item.id)),
                &item.url,
                vec![cell(format!("{kind} #{}", item.number), 84.), grow(item.title.clone())],
                cx,
            )
        })
        .collect()
}

fn run_rows(ix: usize, runs: &[WorkflowRun], cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    runs.iter()
        .map(|run| {
            // gh-x's own marker, colored by its meaning.
            let color = match run.outcome() {
                RunOutcome::Success => theme.success,
                RunOutcome::Failure => theme.danger,
                RunOutcome::Running | RunOutcome::Queued | RunOutcome::Attention => theme.warning,
                RunOutcome::Skipped | RunOutcome::Unknown => theme.muted_foreground,
            };
            let mark = run.status.clone();
            link_row(
                format!("run-{ix}-{}", run.id).into(),
                &run.url,
                vec![
                    div()
                        .w(rems(0.875))
                        .flex_none()
                        .text_color(color)
                        .child(mark)
                        .into_any_element(),
                    muted(run.workflow.clone(), 90., cx),
                    grow(run.title.clone()),
                    muted(run.branch.clone(), 150., cx),
                    muted(run.age.clone(), 40., cx),
                ],
                cx,
            )
        })
        .collect()
}

/// The details' sections. With pmd data: Blocked and Waiting on Franz first, when there are any; then open PRs and
/// issues, each tagged with its work status; then In progress, and any work not started that the lists above don't
/// show. Recently merged and CI runs come last.
fn tables(ix: usize, document: &StatusDocument, cx: &App) -> impl IntoElement + use<> {
    let prs = &document.pull_requests;
    let hosted = document.hosted.as_ref();
    let mut tables = v_flex().id(("tables", ix)).test_support().gap_3();
    let work = |title: &'static str, items: Vec<&WorkItem>, tone: Option<Hsla>| {
        table(title, items.len(), None, work_rows(ix, &items, cx), tone, cx)
    };
    if let Some(hosted) = hosted {
        for (status, title) in [("blocked", "Blocked"), ("waiting_on_franz", "Waiting on Franz")] {
            let items = hosted.items(status);
            if !items.is_empty() {
                tables = tables.child(work(title, items, status_color(status, cx)));
            }
        }
    }
    tables = tables
        .child(table(
            "Open PRs",
            prs.open.len(),
            document.section("pullRequests.open"),
            pr_rows("pr", ix, &prs.open, hosted, cx),
            None,
            cx,
        ))
        .child(table(
            "Open issues",
            document.issues.len(),
            document.section("issues"),
            issue_rows(ix, &document.issues, hosted, cx),
            None,
            cx,
        ));
    if let Some(hosted) = hosted.filter(|hosted| hosted.board.is_some()) {
        tables = tables.child(work("In progress", hosted.items("in_progress"), None));
        let listed = |item: &&WorkItem| match item.kind.as_str() {
            "pr" => prs.open.iter().any(|pr| pr.number == item.number),
            "issue" => document.issues.iter().any(|issue| issue.number == item.number),
            _ => false,
        };
        let other: Vec<_> = hosted.items("ready").into_iter().filter(|item| !listed(item)).collect();
        if !other.is_empty() {
            tables = tables.child(work("Other work not started", other, None));
        }
    }
    tables
        .child(table(
            "Recently merged",
            prs.merged.len(),
            document.section("pullRequests.merged"),
            pr_rows("merged", ix, &prs.merged, hosted, cx),
            None,
            cx,
        ))
        .child(table(
            "CI runs",
            document.workflow_runs.len(),
            document.section("workflowRuns"),
            run_rows(ix, &document.workflow_runs, cx),
            None,
            cx,
        ))
}
