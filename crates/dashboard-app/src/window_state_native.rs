//! Keep Windows workspace coordinates out of GPUI's logical-pixel startup path.
use dashboard_core::placement::NativePlacement;
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
#[derive(Default)]
struct Placement {
    length: u32,
    flags: u32,
    show: u32,
    min: Point,
    max: Point,
    rect: Rect,
}
#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
    device: [u16; 32],
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowPlacement(hwnd: *mut c_void, placement: *mut Placement) -> i32;
    fn SetWindowPlacement(hwnd: *mut c_void, placement: *const Placement) -> i32;
    fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
    fn MonitorFromWindow(hwnd: *mut c_void, flags: u32) -> *mut c_void;
    fn MonitorFromRect(rect: *const Rect, flags: u32) -> *mut c_void;
    fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfo) -> i32;
}
fn hwnd(window: &Window) -> Option<*mut c_void> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut c_void),
        _ => None,
    }
}
fn info(monitor: *mut c_void) -> Option<MonitorInfo> {
    let mut info = MonitorInfo {
        size: std::mem::size_of::<MonitorInfo>() as u32,
        monitor: Rect::default(),
        work: Rect::default(),
        flags: 0,
        device: [0; 32],
    };
    // SAFETY: info is a correctly sized MONITORINFOEXW and monitor is an OS handle.
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info)
}
fn device(info: &MonitorInfo) -> String {
    String::from_utf16_lossy(&info.device[..info.device.iter().position(|c| *c == 0).unwrap_or(32)])
}
pub fn snapshot(window: &Window) -> Option<NativePlacement> {
    let hwnd = hwnd(window)?;
    let mut p = Placement {
        length: std::mem::size_of::<Placement>() as u32,
        ..Default::default()
    };
    // SAFETY: the live window owns hwnd; p is a writable WINDOWPLACEMENT.
    unsafe {
        if GetWindowPlacement(hwnd, &mut p) == 0 {
            return None;
        }
        let monitor = info(MonitorFromWindow(hwnd, 2))?;
        let result = NativePlacement {
            rect: [p.rect.left, p.rect.top, p.rect.right, p.rect.bottom],
            monitor: device(&monitor),
            dpi: GetDpiForWindow(hwnd),
            maximized: p.show == 3 || (p.show == 2 && p.flags & 2 != 0),
        };
        result.valid().then_some(result)
    }
}
pub fn restore(window: &Window, saved: &NativePlacement) {
    if !saved.valid() {
        return;
    }
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let [left, top, right, bottom] = saved.rect;
    let mut rect = Rect {
        left,
        top,
        right,
        bottom,
    };
    // SAFETY: all pointers refer to initialized Win32 structures and a live hwnd.
    unsafe {
        let same = info(MonitorFromRect(&rect, 0)).is_some_and(|m| device(&m) == saved.monitor);
        if !same {
            let Some(primary) = info(MonitorFromRect(&Rect::default(), 1)) else {
                return;
            };
            let w = (right as i64 - left as i64).min((primary.work.right - primary.work.left) as i64) as i32;
            let h = (bottom as i64 - top as i64).min((primary.work.bottom - primary.work.top) as i64) as i32;
            // WINDOWPLACEMENT uses workspace coordinates, including taskbar offset.
            rect = Rect {
                left: primary.monitor.left,
                top: primary.monitor.top,
                right: primary.monitor.left + w,
                bottom: primary.monitor.top + h,
            };
        }
        let p = Placement {
            length: std::mem::size_of::<Placement>() as u32,
            show: if saved.maximized { 3 } else { 1 },
            rect,
            ..Default::default()
        };
        // The first move may synchronously dispatch WM_DPICHANGED. Apply the
        // saved physical rectangle again once the destination DPI is established.
        if SetWindowPlacement(hwnd, &p) == 0 || SetWindowPlacement(hwnd, &p) == 0 {
            eprintln!("project-dashboard: can't restore native window placement");
        }
    }
}
