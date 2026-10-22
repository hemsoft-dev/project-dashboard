# Project Dashboard

<img src="assets/icon-256.png" alt="Project Dashboard icon: four gold dashboard tiles on a black rounded square" width="96">

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

## Install on Windows

1. Open the [latest release](https://github.com/hemsoft-dev/project-dashboard/releases) (early versions are marked
   *Pre-release*) and download `project-dashboard-<version>-windows-x64.zip`. The `.sha256` file next to it holds the
   zip's SHA-256 checksum, which `Get-FileHash <zip>` in PowerShell should match.
2. Unzip it to a folder of your own, for example `%LOCALAPPDATA%\Programs\ProjectDashboard`.
3. Run `project-dashboard.exe`. It opens a window, with no console behind it. To find it from Start, right-click the exe,
   choose *Show more options › Pin to Start* (or *Create shortcut*), or pin the running app to the taskbar.

Windows SmartScreen may warn about an unrecognized app because the executable isn't code-signed yet. Choose
*More info › Run anyway*.

Your settings and cache live in `%APPDATA%\ProjectDashboard`, outside the install folder, so updating never touches
them.

### Update

Close Project Dashboard, download the newer release's zip, and unzip it over the old folder, replacing
`project-dashboard.exe`. Pinned shortcuts keep working because the path doesn't change. Each release's notes list what
changed.

### Release a new version (maintainers)

Set `version` under `[workspace.package]` in `Cargo.toml`, merge, then push a matching tag from the merged `main`.
This reads the version from `Cargo.toml`, so the tag always matches:

```sh
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
git tag "v$version" && git push origin "v$version"
```

The [Release workflow](.github/workflows/release.yml) builds `project-dashboard.exe` on `windows-latest`, checks it
(GUI subsystem, embedded icon and version info, starts and opens its window), zips it with this README and the license,
and publishes the zip and its `.sha256` on a GitHub release. Versions below 1.0 and versions with a hyphen
(`1.2.0-beta.1`) are published as pre-releases. The app's icon is drawn in code (`crates/dashboard-app/src/mark.rs`);
`cargo run -p dashboard-app --example icon -- icon.png 256` writes it as a PNG.

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
[gh-x](https://github.com/hemsoft-dev/gh-x) extension, v0.22.0 or later, for `gh x status --json`
([schema version 1](https://github.com/hemsoft-dev/gh-x/blob/main/docs/status-json.md)):

```sh
gh extension install hemsoft-dev/gh-x   # or: gh extension upgrade gh-x
```

The window shows a table with one row per repository, in config order: a colored dot for its overall state
(**Clean**, **Attention**, **Failed** or **Loading**), its `owner/name`, the counts of open PRs and open issues, when it
was last **updated**, such as `14m ago`, and its **last activity**: the most recently updated open or recently merged
PR, open issue or workflow run in gh-x's report, such as `PR #12` with its title. The update time is that item's;
hover it for the exact local time. A repository is Clean when every health row is clean by gh-x's verdict and
every GitHub section loaded.

- **Expand** a row (click it or its chevron) to see the health rows `gs` prints (default branch, branches, worktrees,
  stashes, and the current branch when it isn't the default), its open pull requests, open issues, recently merged
  pull requests and workflow runs. Several rows can be open at once, and they stay open across refreshes. Click an
  item, or a row's last activity, to open it in your browser.
- **Sort** by update time with the **Updated** column header: newest first, then oldest first, then config order
  again. Repositories with nothing to date stay at the bottom either way.
- **Resize** a column by dragging the border to the right of its header (Repository, PRs, Issues, Updated); last
  activity takes the rest. Double-click a border to restore that column's default width. Widths are remembered in
  `ui-state.json`, next to `config.toml`, and scale with the zoom. On a window too narrow for them, the repository
  column gives way first, then the table scrolls sideways.
- **Refresh all** with the title-bar button or **F5**. It uses gh-x's own cache when that is still fresh.
- **Open the T3 Code thread** working on a repository with the robot button left of its refresh button. It opens
  the thread in the local T3 Code server's web UI (`http://127.0.0.1:3773/<environment>/<thread>` on a default
  install) in your browser; a browser that hasn't paired with that server shows T3's pairing page first. The thread
  is the newest open top-level thread of the T3 project whose folder is the checkout, a running one first. T3 offers
  no other way to ask, so at startup and after every refresh the dashboard reads the state database in T3's
  `userdata` folder, read-only. It looks in `t3_home` from `config.toml`, then T3's own `T3CODE_HOME`, then `~/.t3`
  and `~/.t3-server`, preferring the one a server is running from. When there is nothing to open, the button is
  disabled and its tooltip says why: T3 isn't installed or running, has no project for the checkout, or has no open
  thread. The T3 desktop app has no way to open a thread from outside, so the browser it is.
- **Refresh one** repository with its row's button. It runs `gh x status --json --refresh`, bypassing gh-x's cache.
- **Automatic refresh** runs every `refresh_minutes` (default 5).
- **Config changes apply while it runs.** Save `config.toml` and, within a couple of seconds, added repositories
  appear and load, removed ones disappear, and the rest keep their status. A new `parallelism` applies to the next
  refresh and a new `refresh_minutes` from the next scheduled one. **Refresh all** reads the file first too. If an
  edit leaves the file invalid, the table keeps the last good list and a banner says what's wrong until you fix it.
  `t3_home` is read at startup only.
- **Zoom** the whole window with **Ctrl** **+** (or **Ctrl** **=**) and **Ctrl** **-**, or **Ctrl** + mouse wheel, in
  10% steps from 50% to 200%; **Ctrl** **0** resets to 100%. Text, spacing, icons and columns scale together, and the
  new level shows briefly at the bottom. It's remembered in `ui-state.json`, next to `config.toml`, so the next start
  opens at the same size.
- At startup every row shows its last result from the local cache at once, marked **Cached**, and each row updates as
  its fresh result arrives. Up to `parallelism` (default 4) `gh x status` processes run at a time.
- A repository whose status fails shows the error in its row, in place of the last activity, and keeps its last good
  counts; the others keep working. A GitHub section gh-x couldn't load shows its reason in the expanded row, and its
  count reads `?`; a section gh-x reports as turned off (the merged list with `--merged 0`) reads `–` and *off*.
  Workflow runs show gh-x's own markers (`✓`, `X`, `*`, `○`, `!`, `—`, `·`). A missing `gh` or gh-x shows one setup
  message at the top.

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
cached next to it in `status-cache.json`, and the zoom level in `ui-state.json`; deleting either file is always safe.

## License

[MIT](LICENSE)
