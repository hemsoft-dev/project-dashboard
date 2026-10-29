//! Window geometry in logical pixels relative to a monitor's usable origin, never UI zoom units.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub monitor: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Placement {
    pub fn valid(&self) -> bool {
        [self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite()) && self.width > 0. && self.height > 0.
    }

    /// Resolve against the saved monitor, or the primary monitor when missing/off-screen.
    /// Monitor sizes are logical pixels supplied by the platform at its current DPI.
    pub fn restore(&self, monitors: &[(String, f32, f32)], primary: usize) -> Option<(usize, Self)> {
        if !self.valid() {
            return None;
        }
        let primary = primary.min(monitors.len().checked_sub(1)?);
        let index = monitors.iter().position(|m| m.0 == self.monitor).unwrap_or(primary);
        let (_, w, h) = &monitors[index];
        let visible = self.x >= 0. && self.y >= 0. && self.x + self.width <= *w && self.y + self.height <= *h;
        let index = if visible { index } else { primary };
        let (id, w, h) = &monitors[index];
        if !w.is_finite() || !h.is_finite() || *w <= 0. || *h <= 0. {
            return None;
        }
        let width = self.width.max(480.).min(*w);
        let height = self.height.max(320.).min(*h);
        Some((
            index,
            Self {
                monitor: id.clone(),
                x: self.x.clamp(0., w - width),
                y: self.y.clamp(0., h - height),
                width,
                height,
                maximized: self.maximized,
            },
        ))
    }
}
