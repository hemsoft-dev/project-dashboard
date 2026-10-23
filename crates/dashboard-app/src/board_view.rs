//! The dashboard window's content: a title bar with the refresh controls, then a table with one row per configured
//! checkout: its overall state, name, open PR and issue counts, when it was last updated (#13), and its last activity
//! (#8).
//!
//! Each row's T3 button, left of its refresh button, opens the T3 Code thread working on that checkout (#15).
//!
//! Dragging the border between two header cells resizes the column to its left (#14); double-clicking it restores the
//! default. The widths are remembered in `ui-state.json`.
//!
//! Expanding a row shows what `gh x status` shows for that checkout: the health rows with gh-x's clean verdicts and
//! the open PR, open issue, merged PR and workflow run tables. Items open in the browser.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashboard_core::cache::StatusCache;
use dashboard_core::columns::{Column, ColumnWidths};
use dashboard_core::config::{Config, ConfigError, ConfigFile};
use dashboard_core::priority::{Pmd, Priorities, PrioritySource};
use dashboard_core::scheduler::{self, Outcome};
use dashboard_core::source::StatusSource;
use dashboard_core::status::{HealthRow, Issue, PullRequest, RunOutcome, Section, StatusDocument, WorkflowRun};
use dashboard_core::t3::{self, Lookup, ThreadFinder};
use dashboard_core::ui_state::UiState;
use dashboard_core::{APP_NAME, Board, Card, Overall, SortOrder};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, TitleBar, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FocusHandle, Hsla, InteractiveElement as _, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, TestSupportExt as _, Window, actions, canvas, div, px, rems,
};

actions!(project_dashboard, [Refresh]);

/// F5 refreshes every card, as in a browser.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("f5", Refresh, None)]);
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
}

/// A column border being dragged: which column, and where the drag started.
#[derive(Clone, Copy)]
struct Resize {
    column: Column,
    start_x: Pixels,
    start_width: f32,
}

pub struct BoardView {
    board: Board,
    cache: StatusCache,
    /// Caps every refresh, together, at `parallelism` gh processes.
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
    /// Another lookup was asked for while one ran; it starts when that one lands, so its result is never stale.
    threads_wanted: bool,
    finding_priorities: bool,
    priorities_wanted: bool,
    priority_generation: u64,
    last_refresh: Option<Instant>,
    focus: FocusHandle,
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
        // One limit for every refresh, so overlapping refreshes never run more than `parallelism` gh processes.
        let wanted = scheduler::Wanted::default();
        let limiter = Arc::new(scheduler::Limited::new(source, options.parallelism));
        let columns = options.columns;
        let config_file = options.config_dir.as_deref().map(ConfigFile::new);
        let mut view = Self {
            board: Board::new(checkouts, &cache),
            cache,
            limiter,
            wanted,
            auto_schedule: 0,
            config_file,
            options,
            expanded: HashSet::new(),
            sort: SortOrder::default(),
            columns,
            resizing: None,
            repository_laid_out: Rc::new(Cell::new(None)),
            threads: HashMap::new(),
            finding_threads: false,
            threads_wanted: false,
            finding_priorities: false,
            priorities_wanted: false,
            priority_generation: 0,
            last_refresh: None,
            focus,
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
        view.sync_wanted();
        view.schedule_auto_refresh(cx);
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
    pub fn priorities_pending(&self) -> bool {
        self.finding_priorities
    }

    #[cfg(test)]
    pub fn apply_priorities(&mut self, priorities: Priorities, cx: &mut Context<Self>) {
        self.board.set_priorities(priorities);
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

    /// pmd never occupies a gh-x worker or delays the UI. Coalesce repeated requests and ignore an old config's result.
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
            let _ = send.send(source.fetch());
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let priorities = match receive.try_recv() {
                    Ok(priorities) => priorities,
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => Priorities::default(),
                };
                let _ = this.update(cx, |view, cx| {
                    view.finding_priorities = false;
                    if view.priority_generation == generation {
                        view.board.set_priorities(priorities);
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
        if let Some(result) = self.config_file.as_mut().and_then(ConfigFile::changed) {
            self.apply_config(result, cx);
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
        self.options.config_problem = None;
        if self.options.pmd.is_some() {
            let dir = self
                .options
                .config_dir
                .as_deref()
                .expect("live pmd has a config directory");
            let pmd = Pmd::from_config(&config, dir);
            if self.options.pmd.as_ref() != Some(&pmd) {
                self.priority_generation += 1;
                self.options.priorities = Arc::new(pmd.clone());
                self.options.pmd = Some(pmd);
                self.board.set_priorities(Priorities::default());
                self.refresh_priorities(cx);
            }
        }
        let before = self.board.cards().len();
        let added = self.board.set_checkouts(&config.repositories, &self.cache);
        self.sync_wanted();
        let present: HashSet<PathBuf> = self.board.checkouts().into_iter().collect();
        self.expanded.retain(|checkout| present.contains(checkout));
        self.threads.retain(|checkout, _| present.contains(checkout));
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

    /// Refreshes every card that isn't already refreshing, from gh-x's own cache when it is fresh. The config file is
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

    /// Refreshes one card with `--refresh`, bypassing gh-x's cache.
    pub fn refresh_one(&mut self, checkout: &Path, cx: &mut Context<Self>) {
        self.refresh_priorities(cx);
        if self.board.card(checkout).is_some_and(|card| !card.refreshing) {
            self.start(vec![checkout.to_owned()], true, cx);
        }
    }

    fn start(&mut self, checkouts: Vec<PathBuf>, bypass_cache: bool, cx: &mut Context<Self>) {
        if checkouts.is_empty() {
            return;
        }
        let generations = self.board.start_refresh(&checkouts);
        // Work for a checkout removed (or removed and added back) since now is skipped, before it queues for a slot
        // and again once it has one, so it never runs gh or holds a slot.
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
                    for outcome in outcomes {
                        // A card removed and added back while this ran is a new card; this result isn't for it.
                        let Some(&generation) = generations.get(&outcome.checkout) else {
                            continue;
                        };
                        if view.board.apply_started(outcome, generation, &mut view.cache) {
                            cx.notify();
                        }
                    }
                    if done {
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
    #[cfg(all(test, target_os = "linux"))]
    pub fn apply(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        if self.board.apply(outcome, &mut self.cache) {
            cx.notify();
        }
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
                view.threads = checkouts
                    .into_iter()
                    .zip(found)
                    .filter(|(checkout, _)| present.contains(checkout))
                    .collect();
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
        let status = match (refreshing, self.last_refresh) {
            (true, _) => "Refreshing…".to_owned(),
            (false, Some(at)) => format!("Updated {}", ago(at.elapsed())),
            (false, None) if self.board.cards().iter().any(|card| card.from_cache) => "Cached".to_owned(),
            (false, None) => String::new(),
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
                            .id("refresh-status")
                            .test_support()
                            .min_w_0()
                            .flex_shrink(10.)
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    )
                    .child(
                        div().flex_none().child(
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

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(div().w(ACTIONS).flex_none())
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
        let prs = count("pullRequests.open", |document| document.pull_requests.open.len());
        let issues = count("issues", |document| document.issues.len());
        let error = card.error.as_ref().filter(|error| !error.is_setup());
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
                    .min_w_0()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    })
                    // The title only gets space the label leaves; on a narrow window the label truncates too.
                    .child(
                        div()
                            .id(("activity-summary", ix))
                            .test_support()
                            .min_w_0()
                            .truncate()
                            .child(activity.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(activity.title.clone()),
                    )
                    .into_any_element()
            }
            // A setup problem (no gh or gh-x) is explained once in the banner; the row just says it has nothing.
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
            .when(expanded, |this| this.bg(theme.list_active))
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
            .child(div().w(STATE).flex_none().child(state_dot(ix, overall, cx)))
            .child(
                column_cell(Column::Repository, &self.columns)
                    .id(("name", ix))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().min_w_0().truncate().font_semibold().child(card.name()))
                    // Stays while a refresh runs: until a fresh result lands, the row still shows the cached copy.
                    .when(card.from_cache, |this| {
                        this.child(
                            div()
                                .id(("cached", ix))
                                .test_support()
                                .flex_none()
                                .child(Tag::secondary().xsmall().outline().child("Cached")),
                        )
                    }),
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
            .child(column_cell(Column::Updated, &self.columns).child(updated))
            // Takes the width the other columns leave, and shrinks first on a narrow window, so the refresh button
            // stays in view.
            .child(div().flex_1().min_w(ACTIVITY_MIN).child(activity))
            .child(
                h_flex()
                    .w(ACTIONS)
                    .flex_none()
                    .justify_end()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.thread_button(ix, &card.checkout))
                    .child(
                        Button::new(("refresh", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCw)
                            .tooltip("Refresh this repository (gh x status --refresh)")
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
            .border_b_1()
            .border_color(theme.border)
            .child(row)
            .when(expanded, |this| this.child(self.render_details(ix, card, cx)))
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
            .icon(IconName::Bot)
            .tooltip(tooltip)
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
            .pl(rems(3.25))
            .pr_4()
            .pt_1()
            .pb_3()
            .gap_1();
        if let Some(error) = card.error.as_ref().filter(|error| !error.is_setup()) {
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
                body = body.children(document.health_rows().iter().map(|row| health_row(row, cx)));
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
            }
            None if card.error.is_none() => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading gh x status…"),
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
/// The T3 and refresh buttons at the end of each row.
const ACTIONS: gpui_kit::Rems = rems(3.5);
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
        Column::Updated => "Updated",
    }
}

/// The table's least width: chevron, state and the two row buttons; the repository column's minimum and the other resizable
/// columns' widths; the lead before Updated; last activity's minimum; eight gaps and 0.5 rem padding on each side.
/// Narrow windows and wider saved columns scroll sideways with the header and rows together.
fn table_min(widths: &ColumnWidths) -> gpui_kit::Rems {
    let resizable = widths.total() - widths.get(Column::Repository) + Column::Repository.min_width();
    rems(1.75 + 0.75 + 3.5 + resizable + 0.75 + 1.5 + 8. * 0.5 + 2. * 0.5)
}

impl Render for BoardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = v_flex().id("board").flex_1().min_h_0();
        if let Some(problem) = &self.options.config_problem {
            content = content.child(self.render_banner(IconName::TriangleAlert, problem.clone(), cx));
        }
        if let Some(problem) = self.board.setup_problem() {
            content = content.child(self.render_banner(IconName::CircleAlert, problem.to_string(), cx));
        }
        if self.board.is_empty() {
            content = content.child(self.render_empty(cx));
        } else {
            let cards = self.board.cards().to_vec();
            let rows: Vec<AnyElement> = self
                .board
                .order(self.sort)
                .into_iter()
                .map(|ix| self.render_row(ix, &cards[ix], cx).into_any_element())
                .collect();
            // The table never narrows below its columns' minimums. When zoom makes those wider than the window, it
            // scrolls sideways, header and all, so every row's controls stay reachable.
            content = content.child(
                div()
                    .id("table")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .child(
                        v_flex()
                            .size_full()
                            .min_w(table_min(&self.columns))
                            .child(self.render_header(cx))
                            .child(
                                v_flex()
                                    .id("rows")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .children(rows),
                            ),
                    ),
            );
        }
        v_flex()
            .id("dashboard")
            .key_context("Dashboard")
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &Refresh, _, cx| view.refresh_all(cx)))
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

/// The row's overall state as a colored dot, with its name for tests and the tooltip.
fn state_dot(ix: usize, overall: Overall, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (color, label) = match overall {
        Overall::Loading => (theme.muted_foreground, "Loading"),
        Overall::Clean => (theme.success, "Clean"),
        Overall::Attention => (theme.warning, "Attention"),
        Overall::Failed => (theme.danger, "Failed"),
    };
    div()
        .id(SharedString::from(format!("state-{ix}-{label}")))
        .test_support()
        // The color alone doesn't reach screen readers.
        .aria_label(format!("State: {label}"))
        .size(rems(0.625))
        .rounded_full()
        .bg(color)
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx))
}

/// A section's count: "?" when gh-x couldn't load it, "–" when turned off, never 0 for either.
pub(crate) fn section_count(document: &StatusDocument, section: &str, len: usize) -> String {
    match document.section(section) {
        Some(section) if section.is_unavailable() => "?".to_owned(),
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

/// One table: a heading with its count, then clickable rows or "None". When gh-x couldn't load the section, the
/// count reads "?" and gh-x's reason stands in for the rows: an empty list there means unknown, not none. A section
/// the invocation turned off reads "off".
fn table(
    title: &'static str,
    count: usize,
    section: Option<&Section>,
    rows: Vec<AnyElement>,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let unavailable = section.filter(|section| section.is_unavailable());
    let disabled = section.is_some_and(Section::is_disabled);
    let heading = match (unavailable, disabled) {
        (Some(_), _) => format!("{title} (?)"),
        (None, true) => format!("{title} (off)"),
        (None, false) => format!("{title} ({count})"),
    };
    let note = |id: String, text: String, color| {
        vec![
            div()
                .id(SharedString::from(id))
                .test_support()
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element(),
        ]
    };
    let body: Vec<AnyElement> = match (unavailable, disabled) {
        (Some(section), _) => note(
            format!("unavailable-{title}"),
            format!("Unavailable: {}", section.reason),
            theme.warning,
        ),
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
                .pt_2()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
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

fn pr_rows(kind: &str, ix: usize, prs: &[PullRequest], cx: &App) -> Vec<AnyElement> {
    prs.iter()
        .map(|pr| {
            link_row(
                format!("{kind}-{ix}-{}", pr.number).into(),
                &pr.url,
                vec![
                    cell(format!("#{}", pr.number), 52.),
                    grow(pr.title.clone()),
                    muted(pr.checks.clone(), 72., cx),
                    muted(pr.review.clone(), 120., cx),
                    muted(pr.ai_review.clone(), 84., cx),
                    muted(pr.updated.clone(), 40., cx),
                ],
                cx,
            )
        })
        .collect()
}

fn issue_rows(ix: usize, issues: &[Issue], cx: &App) -> Vec<AnyElement> {
    issues
        .iter()
        .map(|issue| {
            link_row(
                format!("issue-{ix}-{}", issue.number).into(),
                &issue.url,
                vec![
                    cell(format!("#{}", issue.number), 52.),
                    grow(issue.title.clone()),
                    muted(issue.pull_requests.clone(), 60., cx),
                    muted(issue.labels.clone(), 120., cx),
                    muted(issue.updated.clone(), 40., cx),
                ],
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

fn tables(ix: usize, document: &StatusDocument, cx: &App) -> impl IntoElement + use<> {
    let prs = &document.pull_requests;
    v_flex()
        .id(("tables", ix))
        .test_support()
        .gap_1()
        .child(table(
            "Open pull requests",
            prs.open.len(),
            document.section("pullRequests.open"),
            pr_rows("pr", ix, &prs.open, cx),
            cx,
        ))
        .child(table(
            "Open issues",
            document.issues.len(),
            document.section("issues"),
            issue_rows(ix, &document.issues, cx),
            cx,
        ))
        .child(table(
            "Recently merged",
            prs.merged.len(),
            document.section("pullRequests.merged"),
            pr_rows("merged", ix, &prs.merged, cx),
            cx,
        ))
        .child(table(
            "Workflow runs",
            document.workflow_runs.len(),
            document.section("workflowRuns"),
            run_rows(ix, &document.workflow_runs, cx),
            cx,
        ))
}
