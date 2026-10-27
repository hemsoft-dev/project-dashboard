//! Interface zoom (#9): Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 (Command on macOS) and Ctrl+mouse wheel, 50–200% in 10%
//! steps. Zoom scales the theme's base font, which gpui-kit uses as the rem for type, spacing, controls and icons, so
//! everything grows together; the few sizes given in pixels go through [`scaled`]. The level is saved to
//! `ui-state.json` once input settles and shows briefly as a toast. Follows CodexBar's `zoom.rs` (#116).

use std::path::PathBuf;
use std::time::Duration;

use dashboard_core::ui_state::UiState;
use dashboard_core::zoom as levels;
use gpui_kit::component::Theme;
use gpui_kit::{
    App, DispatchPhase, Global, KeyBinding, Pixels, ScrollDelta, ScrollWheelEvent, TouchPhase, Window, actions, px,
};

actions!(project_dashboard, [ZoomIn, ZoomOut, ResetZoom]);

/// The theme's base font size at 100% (`font.size` in themes/dashboard.json).
const BASE_FONT_SIZE: f32 = 16.0;
/// Precise (touchpad) scrolling is accumulated so one notch-sized gesture is one step.
const PIXELS_PER_STEP: f32 = 50.0;
/// Saving waits for input to settle, so a fast wheel writes the file once.
const SAVE_DELAY: Duration = Duration::from_millis(600);
/// How long the "110%" toast stays.
pub const TOAST: Duration = Duration::from_millis(1200);

struct ZoomState {
    level: f64,
    /// Where `ui-state.json` lives; `None` keeps the level in memory (tests and screenshots).
    dir: Option<PathBuf>,
    /// Bumped on every change; a pending save or toast only acts if no newer change followed it.
    generation: u64,
    /// Whether the toast is showing.
    toast: bool,
    pixel_carry: f32,
}

impl Global for ZoomState {}

/// [`init`] unless zoom is already set up, for tests that open several windows.
#[cfg(test)]
pub fn init_if_absent(cx: &mut App, dir: Option<PathBuf>) {
    if cx.try_global::<ZoomState>().is_none() {
        init(cx, dir);
    }
}

/// Forgets the zoom state, as a restart would.
#[cfg(test)]
pub fn reset_for_tests(cx: &mut App) {
    cx.remove_global::<ZoomState>();
}

/// Applies the saved level, registers the shortcuts, and saves future changes to `dir` (none: in memory only).
pub fn init(cx: &mut App, dir: Option<PathBuf>) {
    let level = dir.as_deref().map_or(levels::DEFAULT, |dir| UiState::load(dir).zoom);
    cx.set_global(ZoomState {
        level,
        dir,
        generation: 0,
        toast: false,
        pixel_carry: 0.0,
    });
    apply(level, cx);
    // A change made just before quitting would otherwise be lost with its pending save.
    cx.on_app_quit(|cx| {
        save(cx);
        async {}
    })
    .detach();
    cx.bind_keys([
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary-shift-=", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-0", ResetZoom, None),
    ]);
    cx.on_action(|_: &ZoomIn, cx| step(1, cx));
    cx.on_action(|_: &ZoomOut, cx| step(-1, cx));
    cx.on_action(|_: &ResetZoom, cx| set_level(levels::DEFAULT, cx));
}

pub fn level(cx: &App) -> f64 {
    cx.try_global::<ZoomState>()
        .map_or(levels::DEFAULT, |state| state.level)
}

/// The toast's text while it shows.
pub fn toast(cx: &App) -> Option<String> {
    cx.try_global::<ZoomState>()
        .filter(|state| state.toast)
        .map(|state| levels::label(state.level))
}

/// A size designed at 100%, at the current zoom, for the few sizes gpui-kit takes in pixels.
/// Hides the toast at once, so a screenshot taken right after a zoom change shows only the window.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub fn hide_toast(cx: &mut App) {
    cx.global_mut::<ZoomState>().toast = false;
    cx.refresh_windows();
}

pub fn scaled(px_at_100: f32, cx: &App) -> Pixels {
    px(px_at_100 * level(cx) as f32)
}

pub fn step(steps: i32, cx: &mut App) {
    set_level(levels::stepped(level(cx), steps), cx);
}

pub fn set_level(new_level: f64, cx: &mut App) {
    let new_level = levels::clamp(new_level);
    let generation = {
        let state = cx.global_mut::<ZoomState>();
        state.generation += 1;
        state.toast = true;
        state.level = new_level;
        state.generation
    };
    apply(new_level, cx);
    cx.refresh_windows();
    // After input settles: save once and hide the toast, unless a newer change has taken over.
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        cx.update(|cx| {
            if cx.global::<ZoomState>().generation == generation {
                save(cx);
            }
        });
        cx.background_executor().timer(TOAST - SAVE_DELAY).await;
        cx.update(|cx| {
            let state = cx.global_mut::<ZoomState>();
            if state.generation == generation {
                state.toast = false;
                cx.refresh_windows();
            }
        });
    })
    .detach();
}

fn save(cx: &mut App) {
    let Some(state) = cx.try_global::<ZoomState>() else {
        return;
    };
    let level = state.level;
    if let Some(dir) = &state.dir
        && let Err(err) = UiState::update(dir, |saved| saved.zoom = level)
    {
        eprintln!("project-dashboard: can't save the zoom level: {err}");
    }
}

/// The base font size at the current zoom, for a theme being applied (a theme resets it to 100%).
pub fn font_size(cx: &App) -> Pixels {
    px(BASE_FONT_SIZE * level(cx) as f32)
}

fn apply(level: f64, cx: &mut App) {
    Theme::update(cx, |theme| theme.font_size = px(BASE_FONT_SIZE * level as f32));
}

/// Whole zoom steps for a wheel event, carrying any partial remainder. Positive zooms in. Each line event counts as
/// one notch; pixel (touchpad) scrolling accumulates.
fn wheel_steps(delta: &ScrollDelta, carry: &mut f32) -> i32 {
    match delta {
        // `signum` of 0.0 is 1.0, so a horizontal-only event must not count as a step.
        ScrollDelta::Lines(lines) if lines.y == 0.0 => {}
        ScrollDelta::Lines(lines) => *carry += lines.y.signum() * PIXELS_PER_STEP,
        ScrollDelta::Pixels(pixels) => *carry += f32::from(pixels.y),
    }
    let steps = (*carry / PIXELS_PER_STEP).trunc();
    *carry -= steps * PIXELS_PER_STEP;
    steps as i32
}

/// Steps for one event of a gesture: a new or finished touchpad gesture starts from zero, so separate partial
/// gestures never add up to an unexpected step.
fn gesture_steps(delta: &ScrollDelta, phase: TouchPhase, carry: &mut f32) -> i32 {
    if phase == TouchPhase::Started {
        *carry = 0.0;
    }
    let steps = wheel_steps(delta, carry);
    if phase == TouchPhase::Ended {
        *carry = 0.0;
    }
    steps
}

/// Registers this frame's Ctrl+wheel listener in the capture phase, ahead of the scrolling table, so the wheel zooms
/// instead of scrolling. Call from a paint callback; a wheel without Ctrl passes through untouched.
pub fn capture_wheel(window: &mut Window) {
    window.on_mouse_event(|event: &ScrollWheelEvent, phase, _window, cx| {
        if phase != DispatchPhase::Capture || cx.try_global::<ZoomState>().is_none() {
            return;
        }
        if !event.modifiers.secondary() {
            // Plain scrolling between Ctrl gestures must not bank toward a later zoom step.
            cx.global_mut::<ZoomState>().pixel_carry = 0.0;
            return;
        }
        cx.stop_propagation();
        let steps = gesture_steps(
            &event.delta,
            event.touch_phase,
            &mut cx.global_mut::<ZoomState>().pixel_carry,
        );
        if steps != 0 {
            step(steps, cx);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::point;

    #[test]
    fn a_wheel_notch_is_one_step_and_horizontal_scrolling_none() {
        let lines = |x: f32, y: f32| ScrollDelta::Lines(point(x, y));
        let mut carry = 0.0;
        assert_eq!(wheel_steps(&lines(0.0, 3.0), &mut carry), 1);
        assert_eq!(wheel_steps(&lines(0.0, -1.0), &mut carry), -1);
        assert_eq!(wheel_steps(&lines(2.0, 0.0), &mut carry), 0);
    }

    #[test]
    fn touchpad_scrolling_accumulates_within_a_gesture_only() {
        let delta = |y: f32| ScrollDelta::Pixels(point(px(0.0), px(y)));
        let mut carry = 0.0;
        assert_eq!(gesture_steps(&delta(30.0), TouchPhase::Moved, &mut carry), 0);
        assert_eq!(gesture_steps(&delta(10.0), TouchPhase::Ended, &mut carry), 0);
        assert_eq!(carry, 0.0, "a finished gesture leaves nothing behind");
        assert_eq!(gesture_steps(&delta(30.0), TouchPhase::Started, &mut carry), 0);
        assert_eq!(gesture_steps(&delta(25.0), TouchPhase::Moved, &mut carry), 1);
    }
}
