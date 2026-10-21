//! Interface zoom (#9): the level's bounds and 10% steps, and the small UI-state file that remembers it.
//!
//! The level is a factor (1.0 is 100%). It lives in `ui-state.json` next to `config.toml` rather than in the config
//! file itself, so saving it never rewrites the user's hand-edited config.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MIN: f64 = 0.5;
pub const MAX: f64 = 2.0;
pub const STEP: f64 = 0.1;
pub const DEFAULT: f64 = 1.0;

pub const STATE_FILE: &str = "ui-state.json";

/// `level` kept within 50–200% and on a whole percent, so repeated steps never drift (0.1 isn't exact in binary).
pub fn clamp(level: f64) -> f64 {
    if !level.is_finite() {
        return DEFAULT;
    }
    (level.clamp(MIN, MAX) * 100.0).round() / 100.0
}

/// The level `steps` 10% steps from `level`, kept in range. Positive zooms in.
pub fn stepped(level: f64, steps: i32) -> f64 {
    clamp(level + f64::from(steps) * STEP)
}

/// "110%".
pub fn label(level: f64) -> String {
    format!("{}%", (clamp(level) * 100.0).round() as i64)
}

/// What the app remembers between runs that isn't configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiState {
    pub zoom: f64,
}

impl Default for UiState {
    fn default() -> Self {
        Self { zoom: DEFAULT }
    }
}

impl UiState {
    /// Reads `dir/ui-state.json`. A missing or unreadable file is the default state, and an out-of-range zoom is
    /// clamped: the file only holds preferences, so it never stops the app from starting.
    pub fn load(dir: &Path) -> Self {
        let mut state: Self = std::fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        state.zoom = clamp(state.zoom);
        state
    }

    /// Writes the file through a temporary one, so a crash mid-write leaves the old state.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(STATE_FILE);
        let temporary = path.with_extension("json.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&temporary, &path)
    }
}
