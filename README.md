# Project Dashboard

A fast, native Windows app that shows the status of all your active
repositories at a glance. It covers everything `gh x status` (alias `gs`) shows
for one checkout, across every project, in one window.

Status: planning. See the [issues](https://github.com/hemsoft-dev/project-dashboard/issues).

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

## License

[MIT](LICENSE)
