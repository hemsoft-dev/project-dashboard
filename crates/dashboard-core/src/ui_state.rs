//! What the app remembers between runs that isn't configuration: the zoom level (#9) and the table's column widths
//! (#14). It lives in `ui-state.json` next to `config.toml` rather than in the config file itself, so saving it never
//! rewrites the user's hand-edited config.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::columns::ColumnWidths;
use crate::zoom;

pub const STATE_FILE: &str = "ui-state.json";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub zoom: f64,
    pub columns: ColumnWidths,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            zoom: zoom::DEFAULT,
            columns: ColumnWidths::default(),
        }
    }
}

impl UiState {
    /// Reads `dir/ui-state.json`. The file only holds preferences, so it never stops the app from starting: a missing
    /// or unreadable file is the default state, and each value that is missing, malformed or out of range falls back
    /// to its default or bound on its own.
    pub fn load(dir: &Path) -> Self {
        let value: Option<Value> = std::fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        let value = value.as_ref();
        Self {
            zoom: value
                .and_then(|value| value.get("zoom"))
                .and_then(Value::as_f64)
                .map_or(zoom::DEFAULT, zoom::clamp),
            columns: ColumnWidths::from_value(value.and_then(|value| value.get("columns"))),
        }
    }

    /// Writes the file through a temporary one, so a crash mid-write leaves the old state. `std::fs::rename` replaces
    /// an existing `ui-state.json` on Windows as well as Unix: Rust calls `MoveFileExW` with
    /// `MOVEFILE_REPLACE_EXISTING` there (<https://doc.rust-lang.org/std/fs/fn.rename.html>), and
    /// `saving_over_an_existing_state_file_replaces_it` checks it on every CI platform.
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

    /// Changes one part of the saved state and keeps the rest as the file has it, so saving the zoom never undoes a
    /// column width and the other way round.
    pub fn update(dir: &Path, change: impl FnOnce(&mut Self)) -> std::io::Result<()> {
        let mut state = Self::load(dir);
        change(&mut state);
        state.save(dir)
    }
}
