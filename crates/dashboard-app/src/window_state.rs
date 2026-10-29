//! Platform window geometry is independent of the app's rem-based zoom.
use crate::board_view::BoardView;
use dashboard_core::{config, placement::Placement, ui_state::UiState};
use gpui_kit::{App, Bounds, Context, Task, Window, WindowBounds, WindowOptions, point, px, size};
use std::{cell::RefCell, rc::Rc, time::Duration};

pub fn restore(options: &mut WindowOptions, cx: &App) {
    let Some(saved) = UiState::load(&config::default_dir()).placement else {
        return;
    };
    let displays = cx.displays();
    let primary = cx
        .primary_display()
        .and_then(|p| displays.iter().position(|d| d.id() == p.id()))
        .unwrap_or(0);
    let monitors: Vec<_> = displays
        .iter()
        .map(|d| {
            let area = d.visible_bounds();
            (
                d.uuid().map(|id| id.to_string()).unwrap_or_default(),
                f32::from(area.size.width),
                f32::from(area.size.height),
            )
        })
        .collect();
    let Some((index, saved)) = saved.restore(&monitors, primary) else {
        return;
    };
    let display = &displays[index];
    let origin = display.visible_bounds().origin;
    let bounds = Bounds::new(
        point(origin.x + px(saved.x), origin.y + px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    options.display_id = Some(display.id());
    options.window_bounds = Some(if saved.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    });
}

fn snapshot(window: &Window, cx: &App) -> Option<Placement> {
    let display = window.display(cx)?;
    let monitor = display.uuid().ok()?.to_string();
    let bounds = match window.window_bounds() {
        WindowBounds::Windowed(b) | WindowBounds::Maximized(b) => b,
        WindowBounds::Fullscreen(_) => return None,
    };
    let origin = display.visible_bounds().origin;
    Some(Placement {
        monitor,
        x: f32::from(bounds.origin.x - origin.x),
        y: f32::from(bounds.origin.y - origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        maximized: window.is_maximized(),
    })
}
fn save(placement: Option<Placement>, native: Option<dashboard_core::placement::NativePlacement>) {
    if let Some(placement) = placement.filter(Placement::valid)
        && let Err(err) = UiState::update(&config::default_dir(), |s| {
            s.placement = Some(placement);
            s.native_placement = native;
        })
    {
        eprintln!("project-dashboard: can't save window placement: {err}");
    }
}

fn observe_ready(window: &mut Window, cx: &mut Context<BoardView>) {
    let pending: Rc<RefCell<Option<Task<()>>>> = Rc::default();
    let debounce = pending.clone();
    cx.observe_window_bounds(window, move |_, window, cx| {
        let placement = snapshot(window, cx);
        let native = native_snapshot(window);
        *debounce.borrow_mut() = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            save(placement, native);
        }));
    })
    .detach();
    window.on_window_should_close(cx, move |window, cx| {
        pending.borrow_mut().take();
        save(snapshot(window, cx), native_snapshot(window));
        true
    });
}

#[cfg(windows)]
#[path = "window_state_native.rs"]
mod native;

#[cfg(windows)]
fn native_snapshot(window: &Window) -> Option<dashboard_core::placement::NativePlacement> {
    native::snapshot(window)
}

#[cfg(not(windows))]
fn native_snapshot(_: &Window) -> Option<dashboard_core::placement::NativePlacement> {
    None
}

pub fn observe(window: &mut Window, cx: &mut Context<BoardView>) {
    // GPUI applies its initial placement at first presentation. Wait for that
    // before restoring native workspace pixels, and only then attach save observers.
    let entity = cx.entity();
    #[cfg(windows)]
    let saved = UiState::load(&config::default_dir()).native_placement;
    window.on_next_frame(move |window, cx| {
        #[cfg(windows)]
        if let Some(saved) = saved {
            native::restore(window, &saved);
        }
        entity.update(cx, |_, cx| observe_ready(window, cx));
    });
}
