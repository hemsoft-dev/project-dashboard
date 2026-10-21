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

## License

[MIT](LICENSE)
