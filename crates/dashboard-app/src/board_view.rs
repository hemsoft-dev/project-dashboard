//! The dashboard window's content: a title bar with the refresh controls, then one card per configured checkout.
//!
//! Each card shows what `gh x status` shows for that checkout: the health header with gh-x's clean verdicts, the
//! counts, and (expanded) the open PR, open issue, merged PR and workflow run tables. Rows open in the browser.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use dashboard_core::cache::StatusCache;
use dashboard_core::scheduler::{self, Outcome};
use dashboard_core::source::StatusSource;
use dashboard_core::status::{HealthRow, Issue, PullRequest, StatusDocument, WorkflowRun};
use dashboard_core::{APP_NAME, Board, Card, Overall};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FocusHandle, Hsla, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, actions, div, px,
};

actions!(project_dashboard, [Refresh]);

/// F5 refreshes every card, as in a browser.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("f5", Refresh, None)]);
}

/// How often a running refresh is checked for results.
const POLL: Duration = Duration::from_millis(100);

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
}

pub struct BoardView {
    board: Board,
    cache: StatusCache,
    source: Arc<dyn StatusSource>,
    options: Options,
    expanded: HashSet<PathBuf>,
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
        let view = Self {
            board: Board::new(checkouts, &cache),
            cache,
            source,
            options,
            expanded: HashSet::new(),
            last_refresh: None,
            focus,
        };
        if let Some(interval) = view.options.auto_refresh {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(interval).await;
                    let Ok(()) = this.update(cx, |view, cx| view.refresh_all(cx)) else {
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

    pub fn is_expanded(&self, checkout: &Path) -> bool {
        self.expanded.contains(checkout)
    }

    pub fn toggle_expanded(&mut self, checkout: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(checkout) {
            self.expanded.insert(checkout.to_owned());
        }
        cx.notify();
    }

    /// Refreshes every card that isn't already refreshing, from gh-x's own cache when it is fresh.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
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
        if self.board.card(checkout).is_some_and(|card| !card.refreshing) {
            self.start(vec![checkout.to_owned()], true, cx);
        }
    }

    fn start(&mut self, checkouts: Vec<PathBuf>, bypass_cache: bool, cx: &mut Context<Self>) {
        if checkouts.is_empty() {
            return;
        }
        self.board.start_refresh(&checkouts);
        let receiver = scheduler::refresh(
            Arc::clone(&self.source),
            checkouts,
            self.options.parallelism,
            bypass_cache,
        );
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let (outcomes, done) = drain(&receiver);
                let alive = this.update(cx, |view, cx| {
                    for outcome in outcomes {
                        view.apply(outcome, cx);
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

    /// Shows one refresh result on its card as soon as it arrives.
    pub fn apply(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        if self.board.apply(outcome, &mut self.cache) {
            cx.notify();
        }
    }

    fn finished(&mut self, cx: &mut Context<Self>) {
        self.last_refresh = Some(Instant::now());
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
        TitleBar::new().child(
            h_flex()
                .w_full()
                .pr_2()
                .gap_2()
                .items_center()
                .child(Icon::new(IconName::LayoutDashboard).text_color(cx.theme().primary))
                .child(div().font_semibold().child(APP_NAME))
                .child(div().flex_1())
                .child(
                    div()
                        .id("refresh-status")
                        .test_support()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
                .child(
                    Button::new("refresh-all")
                        .ghost()
                        .small()
                        .icon(IconName::RefreshCw)
                        .tooltip("Refresh all (F5)")
                        .loading(refreshing)
                        .on_click(cx.listener(|view, _, _, cx| view.refresh_all(cx))),
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
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(hint))
    }

    fn render_card(&self, ix: usize, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let expanded = self.is_expanded(&card.checkout);
        let overall = card.overall();
        let checkout = card.checkout.clone();
        let toggle_checkout = card.checkout.clone();
        let header = h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new(("expand", ix))
                    .ghost()
                    .xsmall()
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .tooltip(if expanded { "Collapse" } else { "Show the tables" })
                    .on_click(cx.listener(move |view, _, _, cx| view.toggle_expanded(&toggle_checkout, cx))),
            )
            .child(div().font_semibold().truncate().child(card.name()))
            .child(overall_tag(overall))
            .when(card.from_cache && !card.refreshing, |this| {
                this.child(Tag::secondary().small().outline().child("Cached"))
            })
            .child(div().flex_1())
            .when_some(card.document.as_ref(), |this, document| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(counts_line(document)),
                )
            })
            .child(
                Button::new(("refresh", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::RotateCw)
                    .tooltip("Refresh this repository (gh x status --refresh)")
                    .loading(card.refreshing)
                    .on_click(cx.listener(move |view, _, _, cx| view.refresh_one(&checkout, cx))),
            );

        let mut body = v_flex().gap_1();
        if let Some(error) = &card.error
            && !error.is_setup()
        {
            body = body.child(
                h_flex()
                    .id(("card-error", ix))
                    .test_support()
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
                if expanded {
                    body = body.child(tables(ix, document, cx));
                }
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

        let theme = cx.theme();
        v_flex()
            .id(("card", ix))
            .test_support()
            .p_3()
            .gap_2()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(match overall {
                Overall::Failed => theme.danger,
                Overall::Attention => theme.border,
                _ => theme.border,
            })
            .bg(theme.group_box)
            .child(header)
            .child(body)
    }
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
            let cards: Vec<AnyElement> = self
                .board
                .cards()
                .to_vec()
                .iter()
                .enumerate()
                .map(|(ix, card)| self.render_card(ix, card, cx).into_any_element())
                .collect();
            content = content.child(
                v_flex()
                    .id("cards")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_3()
                    .children(cards),
            );
        }
        v_flex()
            .id("dashboard")
            .key_context("Dashboard")
            .track_focus(&self.focus)
            .on_action(cx.listener(|view, _: &Refresh, _, cx| view.refresh_all(cx)))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(content)
    }
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

fn overall_tag(overall: Overall) -> impl IntoElement {
    let (tag, label) = match overall {
        Overall::Loading => (Tag::secondary(), "Loading"),
        Overall::Clean => (Tag::success(), "Clean"),
        Overall::Attention => (Tag::warning(), "Attention"),
        Overall::Failed => (Tag::danger(), "Failed"),
    };
    tag.small().outline().child(label)
}

pub(crate) fn counts_line(document: &StatusDocument) -> String {
    // A section gh-x couldn't load counts as "?", not 0.
    let count = |section: &str, len: usize| {
        if document
            .unavailable_sections()
            .any(|unavailable| unavailable.name == section)
        {
            "?".to_owned()
        } else {
            len.to_string()
        }
    };
    format!(
        "{} PRs · {} issues · {} merged · {} runs",
        count("pullRequests.open", document.pull_requests.open.len()),
        count("issues", document.issues.len()),
        count("pullRequests.merged", document.pull_requests.merged.len()),
        count("workflowRuns", document.workflow_runs.len())
    )
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
        .child(div().w(px(14.)).text_color(color).child(marker))
        .child(
            div()
                .w(px(84.))
                .text_color(theme.muted_foreground)
                .child(row.label.clone()),
        )
        .child(div().flex_1().truncate().child(row.text.clone()))
}

/// One table: a heading with its count, then clickable rows or "None".
fn table(title: &'static str, count: usize, rows: Vec<AnyElement>, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    v_flex()
        .gap_0p5()
        .child(
            div()
                .pt_2()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(format!("{title} ({count})")),
        )
        .when(rows.is_empty(), |this| {
            this.child(div().text_sm().text_color(theme.muted_foreground).child("None"))
        })
        .children(rows)
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
        .w(px(width))
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
        .w(px(width))
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
            let (mark, color) = match run.status.as_str() {
                "success" => ("✓", theme.success),
                "failure" | "cancelled" | "timed_out" | "startup_failure" => ("✗", theme.danger),
                _ => ("●", theme.warning),
            };
            link_row(
                format!("run-{ix}-{}", run.id).into(),
                &run.url,
                vec![
                    div()
                        .w(px(14.))
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
            pr_rows("pr", ix, &prs.open, cx),
            cx,
        ))
        .child(table(
            "Open issues",
            document.issues.len(),
            issue_rows(ix, &document.issues, cx),
            cx,
        ))
        .child(table(
            "Recently merged",
            prs.merged.len(),
            pr_rows("merged", ix, &prs.merged, cx),
            cx,
        ))
        .child(table(
            "Workflow runs",
            document.workflow_runs.len(),
            run_rows(ix, &document.workflow_runs, cx),
            cx,
        ))
}
