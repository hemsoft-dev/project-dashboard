# AGENTS.md — Project Dashboard

A fast native window that shows hosted Project Manager status for many configured checkouts at once. The stack and
conventions follow [CodexBar](https://github.com/hemsoft-dev/codexbar); when in doubt, do what CodexBar does.

## Stack

- Rust, stable toolchain (`rust-toolchain.toml`, edition 2024).
- [GPUI](https://www.gpui.rs/) through [gpui-kit](https://gpui-kit.com/) `0.8.0`, the version and lock CodexBar
  builds on. Move both apps together when upgrading.
- The HemSoft brand theme, gold on black (`crates/dashboard-app/themes/dashboard.json`, CodexBar's theme file).

## The one rule: pmd is the data source

Franz's explicit October 25, 2026 direction replaces local gh-x reads with the
[project-manager read API](https://github.com/hemsoft-dev/project-manager#read-api).
Production startup, automatic refresh and user refresh read hosted status, priorities and Board/work items from pmd.
There is no gh-x subprocess adapter or fallback, for reads or writes. The app starts no child process at all, so it
never depends on the PATH it inherits (#38, `dashboard-core/tests/no_child_process.rs`). Match projects by full GitHub
`owner/name` from checkout origin, never by basename. Read origin configuration only for identity, never local git
status.

Render API rows and verdicts as supplied. Pmd does not supply local branch, worktree, stash or working-tree health;
show it as unavailable. Pmd supplies no overall verdict either: a hosted row's dot reads the work statuses and gh-x
checks pmd does supply (`Signals` in `lib.rs`), blocked first, never the status's age. Times show in local time and
ages in words (`activity::age_words`), never UTC or seconds. Keep the last good API data on refresh failure, with its original age and the error.
A user refresh reloads pmd's cache; the read API has no force-sync endpoint. Never log or commit a bearer token.
Unknown priorities render as `—`; priority requests remain independent of status workers.

Writes are allowed, through pmd's [write API](https://github.com/hemsoft-dev/project-manager#write-api) only (#33).
The write client and the write token live in `dashboard-core/src/write.rs`; one row's edit and the shared
"Changed elsewhere" flow in `edit.rs`; the Manage menu and the modal frame in `dashboard-app/src/manage.rs`, and each
dialog in its own module beside it (`manage/work_items.rs`, `manage/fleet.rs` and `manage/projects.rs`, with their
data and request bodies in `dashboard-core/src/work.rs`, `fleet.rs` and `projects.rs`). Project priority lives in pmd's
projects table (option C of project-manager#50): no PORTFOLIO.md override or reset in the UI.
Writes use their own token from `pmd_write_token_path`, stored owner-only and never in `config.toml`,
`ui-state.json`, the status cache, logs, errors, the UI or fixtures. Unset, writes are off. Every write carries
`If-Match` or `If-None-Match` and is never retried automatically. A dialog reads with the read token, so without write
access it opens read-only.

## Workspace

```text
crates/
  dashboard-core/   # Data model, no UI: the gh x status --json schema (status.rs), the config file (config.rs),
                    # the status cache (cache.rs), the pmd adapter (pmd.rs), the source boundary (source.rs), the parallel refresh (scheduler.rs),
                    # the write client and token (write.rs), one row's edit (edit.rs) and the board (lib.rs). Fixtures: tests/fixtures/status-*.json.
  dashboard-app/    # GPUI views and the `project-dashboard` binary. `--demo` shows the fixtures. manage.rs: the Manage menu.
                    # src/mark.rs draws the icon; build.rs embeds it and the version block on Windows.
scripts/Build-WindowsRelease.ps1   # Release build, executable checks and zip (.github/workflows/release.yml).
```

Releases: bump `[workspace.package] version`, merge, push the tag `v<version>`. The release workflow refuses a tag
that doesn't match the version.

Keep logic that can be tested without a window in `dashboard-core`.

## Commands

```sh
cargo run -p dashboard-app                                    # open the dashboard
cargo fmt --all --check                                       # format
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked                               # unit tests and headless UI tests
cargo deny --locked check advisories                          # needs cargo install cargo-deny --version 0.20.2 --locked
CAPTURE_DIR=$PWD/captures cargo test -p dashboard-app captures -- --ignored   # PNGs (Linux or macOS; absolute path)
```

On Linux, building needs GPUI's headers:
`sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libxcb1-dev libx11-xcb-dev`.

## Quality gates

Every change passes these before merge; CI runs them on `ubuntu-latest` and `windows-latest`:

1. **Format**: `cargo fmt --all --check` clean.
2. **Lint**: `cargo clippy --workspace --all-targets --locked -- -D warnings` clean.
3. **Tests**: `cargo test --workspace --locked` all green.
4. **Advisories**: `cargo deny --locked check advisories` clean. CI's `advisories` job runs it once, on `ubuntu-latest`,
   because it covers every target. Patch a vulnerability, never accept it. `deny.toml` accepts only unmaintained crates
   that arrive through GPUI, each by ID with its reason. The job reads the lockfile and uploads no source.

5. **Standalone core**: on both Ubuntu and Windows, CI runs
   cargo check -p dashboard-core --locked and cargo test -p dashboard-core --locked with separate clean target directories.

The [main ruleset](https://github.com/hemsoft-dev/project-dashboard/rules/26477626) requires a pull request and
these exact GitHub Actions check names: advisories, core (ubuntu-latest), core (windows-latest),
rust (ubuntu-latest), and rust (windows-latest). The required source is GitHub Actions app 15368.
The branch must be up to date with main; all code-review conversations must be resolved.
There are no bypass actors, including administrators and apps. Force pushes and deletion of main are blocked.
The existing policy still requires a clean current-head AI review; the GitHub formal approval count is 0.

Check-name changes must be coordinated with the ruleset or PRs remain blocked on the old names. Strict freshness
can require another existing CI run after main advances. No release job is added to the required checks:
release.yml is path-filtered for PRs and publishes only on a tag. Release tags are outside this branch ruleset.

These rules trust the PR's workflow under the required names; app-source pinning does not authenticate workflow
contents. Administrators can edit or disable rules through settings. Neither ability is a configured bypass.
See [issue 68 qualification](docs/main-gates.md) for reproducible reads and the explicit unexecuted checks.

UI changes include screenshots in the pull request. Render them with the headless capture test on Linux or macOS, so
no window opens on anyone's desktop.

## Tests

- The status fixtures follow gh-x's `--json` schema version 1
  ([reference](https://github.com/hemsoft-dev/gh-x/blob/main/docs/status-json.md)). `real-*.json` are actual gh-x
  output, each pinned to the terminal view it was captured with; keep at least one. When gh-x changes the schema,
  update the fixtures from real `gh x status --json` output and the structs in `status.rs` together.
- UI tests use `#[gpui_kit::test]` and in-process test windows (`src/ui_tests.rs`); they never open a desktop window
  or send OS input.
- Name tests after the behavior they check (`empty_board_renders_its_empty_state`).
- Cover failure paths as well as success paths.

## Git

- Conventional commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `ci:`.
- One logical change per commit. Pull requests close their issue with `Closes #n`.
