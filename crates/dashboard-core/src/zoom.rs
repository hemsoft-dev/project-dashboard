//! Interface zoom (#9): the level's bounds and 10% steps. The level is a factor (1.0 is 100%), remembered in
//! [`crate::ui_state`].

pub const MIN: f64 = 0.5;
pub const MAX: f64 = 2.0;
pub const STEP: f64 = 0.1;
pub const DEFAULT: f64 = 1.0;

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
