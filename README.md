# Project Dashboard

<img src="assets/icon-256.png" alt="Project Dashboard icon: four gold dashboard tiles on a black rounded square" width="96">

A fast, native Windows app that shows the status of all your active
repositories at a glance. It reads hosted status and Board/work data from Project Manager, across every configured project in one window.

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
- **Project Manager is the data source.** Franz's October 25, 2026 direction replaces local gh-x calls with the
  [pmd read API](https://github.com/hemsoft-dev/project-manager#read-api). The app reads hosted schema-v1 status,
  priority, Board and work items. It renders hosted rows/verdicts and never runs gh-x or falls back to it.
  Local checkout health is unavailable because pmd does not supply it.
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

Project Dashboard needs a reachable [Project Manager read API](https://github.com/hemsoft-dev/project-manager#read-api)
and a read-scoped bearer token in `pmd_token_path`. Configure `pmd_url` for that service.
Each checkout must have one GitHub origin URL whose full `owner/name` matches a tracked pmd project. SSH origins
and linked worktrees are supported. Conditional/include-based Git identity configuration is explicitly unavailable;
folder names never stand in for repository identity. No local GitHub CLI installation is required.

The window shows a table with one row per repository, in config order: a colored dot for its overall state
(**Local health unavailable**, **Failed** or **Loading**, plus any supplied hosted verdict), its `owner/name`, its **Priority**, the counts of open PRs and open issues, when it
was last **updated**, such as `14m ago`, and its **last activity**: the most recently updated open or recently merged
PR, open issue or workflow run in gh-x's report, such as `PR #12` with its title. The update time is that item's;
hover it for the exact local time. Pmd currently supplies no overall/local-health verdict, so hosted status shows Local health unavailable.

- **Expand** a row (click it or its chevron) to see explicitly unavailable local health, the pmd status generation age, Board columns, the complete paginated open-work list, its open pull requests, open issues, recently merged
  pull requests and workflow runs. Several rows can be open at once, and they stay open across refreshes. Click an
  item, or a row's last activity, to open it in your browser.
- **Sort** by update time with the **Updated** column header: newest first, then oldest first, then config order
  again. Repositories with nothing to date stay at the bottom either way.
- **Priority** shows pmd's tier, such as `P0`, immediately before PRs. Click its header for highest priority first,
  then lowest first, then config order. Equal priorities keep config order; unknowns stay last in either direction.
  A repository without a known tier shows `—`. Failed reads retain the last good priorities from the same pmd
  service; a successful response replaces them, including an unknown tier. Its request runs separately, with a
  two-second timeout, independently of status workers. Priority refreshes at startup and with every refresh.
- **Resize** a column by dragging the border to the right of its header (Repository, Priority, PRs, Issues, Updated); last
  activity takes the rest. Double-click a border to restore that column's default width. Widths are remembered in
  `ui-state.json`, next to `config.toml`, and scale with the zoom. On a window too narrow for them, the repository
  column gives way first, then the table scrolls sideways.
- **Refresh all** with the title-bar button or **F5**. It reads pmd's cached state; the read API has no force-sync endpoint.
- **Open the T3 Code thread** working on a repository with the robot button left of its refresh button. It opens
  the thread in the local T3 Code server's web UI (`http://127.0.0.1:3773/<environment>/<thread>` on a default
  install) in your browser; a browser that hasn't paired with that server shows T3's pairing page first. The thread
  is the newest open top-level thread of the T3 project whose folder is the checkout, a running one first. T3 offers
  no other way to ask, so at startup and after every refresh the dashboard reads the state database in T3's
  `userdata` folder, read-only. It looks in `t3_home` from `config.toml`, then T3's own `T3CODE_HOME`, then `~/.t3`
  and `~/.t3-server`, preferring the one a server is running from. When there is nothing to open, the button is
  disabled and its tooltip says why: T3 isn't installed or running, has no project for the checkout, or has no open
  thread. The T3 desktop app has no way to open a thread from outside, so the browser it is.
- **Refresh one** repository with its row's button. It reloads the same pmd read endpoints.
- **Automatic refresh** runs every `refresh_minutes` (default 5).
- **Config changes apply while it runs.** Save `config.toml` and, within a couple of seconds, added repositories
  appear and load, removed ones disappear, and the rest keep their status. A new `parallelism` applies to the next
  refresh and a new `refresh_minutes` from the next scheduled one. **Refresh all** reads the file first too. If an
  edit leaves the file invalid, the table keeps the last good list and a banner says what's wrong until you fix it.
  `t3_home` is read at startup only.
  Changes to `pmd_token_path` replace the credential source and supersede in-flight results while keeping last good
  status, Board/work data and priorities. Failed requests retain that data with its original status age and the error.
  Changes to `pmd_url` or origin identity invalidate the old service or repository data.
- **Zoom** the whole window with **Ctrl** **+** (or **Ctrl** **=**) and **Ctrl** **-**, or **Ctrl** + mouse wheel, in
  10% steps from 50% to 200%; **Ctrl** **0** resets to 100%. Text, spacing, icons and columns scale together, and the
  new level shows briefly at the bottom. It's remembered in `ui-state.json`, next to `config.toml`, so the next start
  opens at the same size.
- At startup every row shows its last result from the local cache at once, marked **Cached**, and each row updates as
  its fresh result arrives. Up to `parallelism` (default 4) pmd status reads run at a time. Legacy local gh-x caches are discarded.
- A repository whose status fails shows the error in its row, in place of the last activity, and keeps its last good
  counts; the others keep working. A GitHub section gh-x couldn't load shows its reason in the expanded row, and its
  count reads `?` only when it has no rows. When a primary list loaded but supplemental enrichment failed,
  its count and available rows remain visible alongside the diagnostic. A section gh-x reports as turned off
  (the merged list with `--merged 0`) reads `–` and *off*.
  Workflow runs show gh-x's own markers (`✓`, `X`, `*`, `○`, `!`, `—`, `·`). Missing/invalid read tokens and HTTP 401 show one setup message at the top. Other errors distinguish timeouts, outages, untracked/ambiguous identities, identity mismatches, no_status, malformed data and unsupported schema.

Board/work data is shown within each expanded repository row, scoped through the API's project filter.
The four columns retain pmd's titles, rows and `truncated` flags. The open-work list follows API cursors,
preserves item IDs and GitHub links, and reports incomplete pagination explicitly. Reads stop at 20 pages or a
15-second total deadline; each HTTP request has a two-second timeout and a 4 MiB response limit. Redirects and system
proxies are disabled. Board/work errors keep hosted status available and retain prior Board/work data when possible,
with the old Board generation timestamp and an error. Status refresh errors preserve the last good status and age.

`cargo run -p dashboard-app -- --demo` shows sample data from the checked-in fixtures, without the config file,
the cache or pmd.

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
# How many pmd status reads run at once.
parallelism = 4
# Base URL of the tailnet-only project-manager API.
pmd_url = "http://100.97.164.73:8790"
# Read-scoped bearer token; relative paths resolve beside config.toml.
pmd_token_path = "pmd-token"
# The folder scanned on first run.
root = "C:/Users/franz/github"
```

On first run, when the file doesn't exist yet, the app creates it with every git checkout one level under `~/github`
(or your home folder if there's no `~/github`). Edit the list to change it; the running app picks up the change when
the file is saved. The last results are
cached next to it in `status-cache.json`, and the zoom level in `ui-state.json`; deleting either file is always safe.

Existing configs use the pmd defaults when these keys are absent. Store the read token as plain text in the configured
file, with access restricted to your user. On Windows the default is `%APPDATA%\ProjectDashboard\pmd-token`.
Only the file path belongs in config.toml. The app never writes the token to its logs, config, or cache.
See [project-manager's token documentation](https://github.com/hemsoft-dev/project-manager#tokens).

## License

[MIT](LICENSE)
