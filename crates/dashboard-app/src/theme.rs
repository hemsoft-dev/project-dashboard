//! The HemSoft brand theme, gold on black, shared with CodexBar (`themes/dashboard.json` is CodexBar's theme file under
//! this app's names). Orange and red stay reserved for status.

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeRegistry};

const THEME_FILE: &str = include_str!("../themes/dashboard.json");
pub const DARK: &str = "Project Dashboard Dark";

/// Registers the brand themes and applies the dark one. Falls back to the stock theme if they fail to parse.
pub fn init(cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_FILE) {
        eprintln!("project-dashboard: brand themes failed to load: {err}");
        return;
    }
    let Some(config) = ThemeRegistry::global(cx).themes().get(DARK).cloned() else {
        eprintln!("project-dashboard: theme {DARK} is missing from themes/dashboard.json");
        return;
    };
    Theme::update(cx, |current| current.apply_config(&config));
}
