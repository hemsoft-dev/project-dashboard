//! Zoom steps and bounds, and the UI-state file that remembers the level and the column widths.

use std::path::PathBuf;

use dashboard_core::columns::{Column, ColumnWidths, MAX_WIDTH};
use dashboard_core::ui_state::{STATE_FILE, UiState};
use dashboard_core::zoom;

#[test]
fn steps_are_ten_percent_and_stay_between_50_and_200_percent() {
    assert_eq!(zoom::stepped(1.0, 1), 1.1);
    assert_eq!(zoom::stepped(1.0, 3), 1.3);
    assert_eq!(zoom::stepped(1.0, -2), 0.8);
    assert_eq!(zoom::stepped(1.95, 1), 2.0);
    assert_eq!(zoom::stepped(2.0, 1), 2.0);
    assert_eq!(zoom::stepped(0.5, -1), 0.5);
    assert_eq!(zoom::stepped(1.0, 100), zoom::MAX);
    assert_eq!(zoom::stepped(1.0, -100), zoom::MIN);
}

#[test]
fn ten_steps_up_and_down_return_exactly_to_100_percent() {
    let mut level = 1.0;
    for _ in 0..10 {
        level = zoom::stepped(level, 1);
    }
    assert_eq!(level, 2.0);
    for _ in 0..10 {
        level = zoom::stepped(level, -1);
    }
    assert_eq!(level, 1.0);
}

#[test]
fn odd_levels_are_clamped_and_labelled() {
    assert_eq!(zoom::clamp(f64::NAN), 1.0);
    assert_eq!(zoom::clamp(f64::INFINITY), 1.0);
    assert_eq!(zoom::clamp(0.1), 0.5);
    assert_eq!(zoom::clamp(1.234), 1.23);
    assert_eq!(zoom::label(1.1), "110%");
    assert_eq!(zoom::label(0.8), "80%");
    assert_eq!(zoom::label(1.0 + 0.1 + 0.1 + 0.1), "130%");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("project-dashboard-zoom-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_level_survives_a_save_and_load() {
    let scratch = Scratch::new("roundtrip");
    assert_eq!(UiState::load(&scratch.0), UiState::default());
    let at = |zoom| UiState {
        zoom,
        ..UiState::default()
    };
    at(1.3).save(&scratch.0).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.3);
    // A later save replaces the file (on Windows too, where rename must overwrite an existing destination).
    at(0.8).save(&scratch.0).unwrap();
    at(1.5).save(&scratch.0).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.5);
    assert!(!scratch.0.join("ui-state.json.tmp").exists());
    // config.toml is untouched: the state lives in its own file.
    assert!(!scratch.0.join("config.toml").exists());
}

#[test]
fn a_corrupt_or_out_of_range_state_file_still_starts_the_app() {
    let scratch = Scratch::new("bad");
    std::fs::create_dir_all(&scratch.0).unwrap();
    std::fs::write(scratch.0.join(STATE_FILE), "{ nope").unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.0);
    std::fs::write(scratch.0.join(STATE_FILE), r#"{"zoom": 9}"#).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 2.0);
    std::fs::write(scratch.0.join(STATE_FILE), r#"{"other": true}"#).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.0);
}

#[test]
fn column_widths_default_and_stay_within_their_bounds() {
    let mut widths = ColumnWidths::default();
    assert_eq!(widths.get(Column::Repository), 18.0);
    assert_eq!(widths.get(Column::Updated), 4.5);
    widths.set(Column::Repository, 2.0);
    assert_eq!(widths.get(Column::Repository), Column::Repository.min_width());
    widths.set(Column::Issues, 500.0);
    assert_eq!(widths.get(Column::Issues), MAX_WIDTH);
    widths.set(Column::PullRequests, f32::NAN);
    assert_eq!(widths.get(Column::PullRequests), Column::PullRequests.default_width());
    widths.set(Column::Updated, 5.123);
    assert_eq!(widths.get(Column::Updated), 5.12);
    widths.reset(Column::Repository);
    widths.reset(Column::Issues);
    widths.reset(Column::Updated);
    assert_eq!(widths, ColumnWidths::default());
    for column in Column::ALL {
        assert!(column.min_width() <= column.default_width(), "{column:?}");
    }
}

#[test]
fn column_widths_survive_a_save_and_load_beside_the_zoom() {
    let scratch = Scratch::new("columns");
    UiState::update(&scratch.0, |state| state.columns.set(Column::Repository, 12.5)).unwrap();
    UiState::update(&scratch.0, |state| state.zoom = 1.2).unwrap();
    UiState::update(&scratch.0, |state| state.columns.set(Column::Updated, 6.0)).unwrap();
    let state = UiState::load(&scratch.0);
    // Each save kept what the others wrote.
    assert_eq!(state.zoom, 1.2);
    assert_eq!(state.columns.get(Column::Repository), 12.5);
    assert_eq!(state.columns.get(Column::Updated), 6.0);
    assert_eq!(state.columns.get(Column::Issues), Column::Issues.default_width());
    let text = std::fs::read_to_string(scratch.0.join(STATE_FILE)).unwrap();
    assert!(text.contains(r#""repository": 12.5"#), "{text}");
}

#[test]
fn a_malformed_or_out_of_range_column_width_falls_back_on_its_own() {
    let scratch = Scratch::new("bad-columns");
    std::fs::create_dir_all(&scratch.0).unwrap();
    std::fs::write(
        scratch.0.join(STATE_FILE),
        r#"{"zoom": 1.1, "columns": {"repository": "wide", "pullRequests": -4, "issues": 1e9, "updated": 7}}"#,
    )
    .unwrap();
    let state = UiState::load(&scratch.0);
    assert_eq!(state.zoom, 1.1);
    assert_eq!(
        state.columns.get(Column::Repository),
        Column::Repository.default_width()
    );
    assert_eq!(
        state.columns.get(Column::PullRequests),
        Column::PullRequests.min_width()
    );
    assert_eq!(state.columns.get(Column::Issues), MAX_WIDTH);
    assert_eq!(state.columns.get(Column::Updated), 7.0);
    // Beyond what an f32 can hold: still the maximum, not the default.
    std::fs::write(
        scratch.0.join(STATE_FILE),
        r#"{"columns": {"issues": 1e39, "updated": -1e39}}"#,
    )
    .unwrap();
    let state = UiState::load(&scratch.0);
    assert_eq!(state.columns.get(Column::Issues), MAX_WIDTH);
    assert_eq!(state.columns.get(Column::Updated), Column::Updated.min_width());
    // Columns that aren't an object, and a file that isn't JSON at all, are the defaults.
    std::fs::write(scratch.0.join(STATE_FILE), r#"{"zoom": 1.1, "columns": [1, 2]}"#).unwrap();
    assert_eq!(UiState::load(&scratch.0).columns, ColumnWidths::default());
    std::fs::write(scratch.0.join(STATE_FILE), "{ nope").unwrap();
    assert_eq!(UiState::load(&scratch.0), UiState::default());
}
