# AGENTS.md — Project Dashboard

A fast native window that shows `gh x status` (Franz's `gs`) for many local checkouts at once. The stack and
conventions follow [CodexBar](https://github.com/hemsoft-dev/codexbar); when in doubt, do what CodexBar does.

## Stack

- Rust, stable toolchain (`rust-toolchain.toml`, edition 2024).
- [GPUI](https://www.gpui.rs/) through [gpui-kit](https://gpui-kit.com/) `0.8.0`, the version and lock CodexBar
  builds on. Move both apps together when upgrading.
- The HemSoft brand theme, gold on black (`crates/dashboard-app/themes/dashboard.json`, CodexBar's theme file).

## The one rule: gh-x is the source of truth

All status data comes from `gh x status --json`
([gh-x#230](https://github.com/hemsoft-dev/gh-x/issues/230)), run in each configured checkout. The app parses and
renders that document. It never calls the GitHub API or git for status itself, and it never re-derives a verdict
(clean, attention, cleanup candidate). If the dashboard needs something gh-x doesn't report, add it to gh-x first.
That way the app and `gs` can never disagree.

## Workspace

```text
crates/
  dashboard-core/   # Data model, no UI: the gh x status --json schema (status.rs), the config file (config.rs),
                    # the status cache (cache.rs), running gh (source.rs), the parallel refresh (scheduler.rs)
                    # and the board (lib.rs). Fixtures: tests/fixtures/status-*.json.
  dashboard-app/    # GPUI views and the `project-dashboard` binary. `--demo` shows the fixtures.
```

Keep logic that can be tested without a window in `dashboard-core`.

## Commands

```sh
cargo run -p dashboard-app                                    # open the dashboard
cargo fmt --all --check                                       # format
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked                               # unit tests and headless UI tests
CAPTURE_DIR=captures cargo test -p dashboard-app captures -- --ignored   # PNG screenshots (Linux only)
```

On Linux, building needs GPUI's headers:
`sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libxcb1-dev libx11-xcb-dev`.

## Quality gates

Every change passes these before merge; CI runs them on `ubuntu-latest` and `windows-latest`:

1. **Format**: `cargo fmt --all --check` clean.
2. **Lint**: `cargo clippy --workspace --all-targets --locked -- -D warnings` clean.
3. **Tests**: `cargo test --workspace --locked` all green.

UI changes include screenshots in the pull request. Render them with the headless capture test on Linux, so no
window opens on anyone's desktop.

## Tests

- The status fixtures follow gh-x's `--json` schema version 1. When gh-x changes the schema, update the fixtures
  from real `gh x status --json` output and the structs in `status.rs` together.
- UI tests use `#[gpui_kit::test]` and in-process test windows (`src/ui_tests.rs`); they never open a desktop window
  or send OS input.
- Name tests after the behavior they check (`empty_board_renders_its_empty_state`).
- Cover failure paths as well as success paths.

## Git

- Conventional commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `ci:`.
- One logical change per commit. Pull requests close their issue with `Closes #n`.
