//! Zoom steps, bounds and the remembered level.

use std::path::PathBuf;

use dashboard_core::zoom::{self, UiState};

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
    UiState { zoom: 1.3 }.save(&scratch.0).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.3);
    // A later save replaces the file (on Windows too, where rename must overwrite an existing destination).
    UiState { zoom: 0.8 }.save(&scratch.0).unwrap();
    UiState { zoom: 1.5 }.save(&scratch.0).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.5);
    assert!(!scratch.0.join("ui-state.json.tmp").exists());
    // config.toml is untouched: the state lives in its own file.
    assert!(!scratch.0.join("config.toml").exists());
}

#[test]
fn a_corrupt_or_out_of_range_state_file_still_starts_the_app() {
    let scratch = Scratch::new("bad");
    std::fs::create_dir_all(&scratch.0).unwrap();
    std::fs::write(scratch.0.join(zoom::STATE_FILE), "{ nope").unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.0);
    std::fs::write(scratch.0.join(zoom::STATE_FILE), r#"{"zoom": 9}"#).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 2.0);
    std::fs::write(scratch.0.join(zoom::STATE_FILE), r#"{"other": true}"#).unwrap();
    assert_eq!(UiState::load(&scratch.0).zoom, 1.0);
}
