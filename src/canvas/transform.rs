// SPDX-License-Identifier: GPL-3.0-only
//! Canvas <-> screen coordinate mapping. Pure math, no GTK, unit-tested.
//!
//!   screen = canvas * zoom + offset
//!   canvas = (screen - offset) / zoom
//!
//! Equivalent to Helvum's `translate(-adj).scale(zoom)` gsk::Transform, with
//! `offset = -adjustment`, but kept as plain f64 so it can be tested headless.

pub const ZOOM_MIN: f64 = 0.1;
pub const ZOOM_MAX: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: f64,
    pub offset: (f64, f64),
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, offset: (0.0, 0.0) }
    }
}

impl View {
    #[allow(dead_code)] // used by Phase 2 hit-testing of slots in screen space
    pub fn to_screen(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (x * self.zoom + self.offset.0, y * self.zoom + self.offset.1)
    }

    pub fn to_canvas(&self, (x, y): (f64, f64)) -> (f64, f64) {
        ((x - self.offset.0) / self.zoom, (y - self.offset.1) / self.zoom)
    }

    /// Multiply zoom by `factor`, keeping the canvas point under `anchor` (screen space) fixed.
    pub fn zoom_at(&mut self, anchor: (f64, f64), factor: f64) {
        let c = self.to_canvas(anchor);
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.offset = (anchor.0 - c.0 * self.zoom, anchor.1 - c.1 * self.zoom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn roundtrip_and_anchor() {
        let mut v = View { zoom: 1.7, offset: (33.0, -12.0) };
        let p = (123.4, -56.7);
        assert!(close(v.to_canvas(v.to_screen(p)), p));

        let anchor = (400.0, 300.0);
        let before = v.to_canvas(anchor);
        v.zoom_at(anchor, 1.25);
        assert!(close(v.to_canvas(anchor), before), "anchor must stay fixed");

        v.zoom_at(anchor, 1000.0);
        assert_eq!(v.zoom, ZOOM_MAX);
        assert!(close(v.to_canvas(anchor), before), "anchor fixed even when clamped");
    }
}
