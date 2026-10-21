# Project Dashboard

A fast, native Windows app that shows the status of all your active
repositories at a glance. It covers everything `gh x status` (alias `gs`) shows
for one checkout, across every project, in one window.

Status: early development. See the [issues](https://github.com/hemsoft-dev/project-dashboard/issues).

## Why

Franz keeps a terminal open next to T3 Code and runs `gs` constantly to see
where each project stands. This app does that continuously, for all repos,
with the look and speed of [CodexBar](https://github.com/hemsoft-dev/codexbar).

## Design decisions (2026-10-21)

- **Standalone app**, not a section of hs-buddy. It replaces
  [hs-buddy#850](https://github.com/hemsoft-dev/hs-buddy/issues/850).
- **Same stack as CodexBar:** Rust, [GPUI](https://www.gpui.rs/) and
  [gpui-kit](https://gpui-kit.com/). Native, lightweight, no Electron.
- **gh-x stays the source of truth.** The app runs `gh x status --json` in each
  repository and renders the result. It never reimplements the status rules, so
  the app and `gs` always agree. JSON output is tracked in
  [gh-x](https://github.com/hemsoft-dev/gh-x/issues).
- **Many repos, one window:** a configurable list of local checkouts, refreshed
  in parallel. Cached results render first, and fresh data replaces them as it
  arrives.

## Build and run

Requirements:

- [Rust](https://www.rust-lang.org/tools/install). `rust-toolchain.toml` pins stable; on Windows that is the MSVC
  toolchain, with the Visual Studio Build Tools' "Desktop development with C++" workload.
- On Linux, the headers GPUI links against. On Debian or Ubuntu:
  `sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libxcb1-dev libx11-xcb-dev`.

```sh
git clone https://github.com/hemsoft-dev/project-dashboard.git
cd project-dashboard
cargo run -p dashboard-app            # opens the Project Dashboard window
cargo run --release -p dashboard-app  # optimized build
```

Checks, the same ones CI runs on Linux and Windows:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

See [AGENTS.md](AGENTS.md) for the stack, the workspace layout and the conventions.

## Usage

Project Dashboard needs the [GitHub CLI](https://cli.github.com) signed in (`gh auth login`) and the
[gh-x](https://github.com/hemsoft-dev/gh-x) extension with `gh x status --json`:

```sh
gh extension install hemsoft-dev/gh-x   # or: gh extension upgrade gh-x
```

The window shows one card per repository: its name, a **Clean**, **Attention**, **Failed** or **Loading** badge,
the counts of open PRs, open issues, recently merged PRs and workflow runs, and the health rows `gs` prints (default
branch, branches, worktrees, stashes, and the current branch when it isn't the default), each with gh-x's own clean
verdict. A card is Clean when every row is clean and every GitHub section loaded.

- **Expand** a card (the chevron) to see its open pull requests, open issues, recently merged pull requests and
  workflow runs. Click a row to open it in your browser.
- **Refresh all** with the title-bar button or **F5**. It uses gh-x's own cache when that is still fresh.
- **Refresh one** card with its own button. It runs `gh x status --json --refresh`, bypassing gh-x's cache.
- **Automatic refresh** runs every `refresh_minutes` (default 5).
- At startup every card shows its last result from the local cache at once, marked **Cached**, and each card updates as
  its fresh result arrives. Up to `parallelism` (default 4) `gh x status` processes run at a time.
- A repository whose status fails shows the error on its own card; the others keep working. A GitHub section gh-x
  couldn't load shows its reason on the card, and its count reads `?`. A missing `gh` or gh-x shows one setup message
  at the top.

`cargo run -p dashboard-app -- --demo` shows sample data from the checked-in fixtures, without the config file,
the cache or gh.

### Config file

`config.toml`, in `%APPDATA%\ProjectDashboard` on Windows and `~/.config/project-dashboard` on Linux (or the folder
in `PROJECT_DASHBOARD_CONFIG_DIR`):

```toml
# Local checkouts, in display order.
repositories = [
  "C:/Users/franz/github/hemsoft/gh-x",
  "C:/Users/franz/github/hemsoft/codexbar",
]
# Minutes between automatic refreshes; 0 turns them off.
refresh_minutes = 5
# How many gh x status processes run at once.
parallelism = 4
# The folder scanned on first run.
root = "C:/Users/franz/github"
```

On first run, when the file doesn't exist yet, the app creates it with every git checkout one level under `~/github`
(or your home folder if there's no `~/github`). Edit the list and restart the app to change it. The last results are
cached next to it in `status-cache.json`; deleting that file is always safe.

## License

[MIT](LICENSE)
