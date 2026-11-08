// SPDX-License-Identifier: GPL-3.0-only
//! Text drawing/measuring through pangocairo, so emoji, box-drawing and other symbols
//! get font fallback (cairo's "toy" text API shows tofu for them).
//!
//! Drop-in for `cr.show_text` / `cr.text_extents`: the size comes from the context's
//! current font size (`cr.set_font_size`) and drawing starts at the baseline-left
//! of the current point.

use gtk::cairo::Context;
use gtk::glib::translate::ToGlibPtr;
use adw::gtk;
use std::os::raw::{c_char, c_int, c_void};

type Layout = *mut c_void;

#[link(name = "pangocairo-1.0")]
#[link(name = "pango-1.0")]
#[link(name = "gobject-2.0")]
extern "C" {
    fn pango_cairo_create_layout(cr: *mut c_void) -> Layout;
    fn pango_cairo_show_layout(cr: *mut c_void, l: Layout);
    fn pango_layout_set_text(l: Layout, text: *const c_char, len: c_int);
    fn pango_layout_get_pixel_size(l: Layout, w: *mut c_int, h: *mut c_int);
    fn pango_layout_get_baseline(l: Layout) -> c_int;
    fn pango_layout_set_font_description(l: Layout, d: *const c_void);
    fn pango_font_description_new() -> *mut c_void;
    fn pango_font_description_set_family(d: *mut c_void, f: *const c_char);
    fn pango_font_description_set_absolute_size(d: *mut c_void, s: f64);
    fn pango_font_description_free(d: *mut c_void);
    fn g_object_unref(o: *mut c_void);
}

fn size(cr: &Context) -> f64 {
    let m = cr.font_matrix();
    (m.yy().abs()).max(1.0)
}

fn layout(cr: &Context, s: &str) -> Layout {
    unsafe {
        let l = pango_cairo_create_layout(cr.to_glib_none().0 as *mut c_void);
        let d = pango_font_description_new();
        pango_font_description_set_family(d, c"Sans".as_ptr());
        pango_font_description_set_absolute_size(d, size(cr) * 1024.0);
        pango_layout_set_font_description(l, d);
        pango_font_description_free(d);
        pango_layout_set_text(l, s.as_ptr() as *const c_char, s.len() as c_int);
        l
    }
}

/// Width in pixels of `s` at the context's font size.
pub fn width(cr: &Context, s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    unsafe {
        let l = layout(cr, s);
        let (mut w, mut h) = (0, 0);
        pango_layout_get_pixel_size(l, &mut w, &mut h);
        g_object_unref(l);
        w as f64
    }
}

/// Draw `s` with its baseline-left at the current point.
pub fn show(cr: &Context, s: &str) {
    if s.is_empty() {
        return;
    }
    let Some((x, y)) = cr.current_point().ok() else { return };
    unsafe {
        let l = layout(cr, s);
        let base = pango_layout_get_baseline(l) as f64 / 1024.0;
        cr.move_to(x, y - base);
        pango_cairo_show_layout(cr.to_glib_none().0 as *mut c_void, l);
        cr.move_to(x, y);
        g_object_unref(l);
    }
}
