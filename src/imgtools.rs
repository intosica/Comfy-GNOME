// SPDX-License-Identifier: GPL-3.0-only
//! Small picture tools opened from the lightbox: before/after compare, crop, paint.
//! Each is a modal window around one cairo `DrawingArea`; crop and paint hand a PNG back
//! (`done`) for the caller to drop into the workflow, and can save it to disk themselves.

use adw::{gdk, gio, glib, gtk, prelude::*};
use gtk::cairo::{Context, Format, ImageSurface};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Decode any picture bytes into a cairo surface.
pub fn surface(bytes: &[u8]) -> Option<ImageSurface> {
    let tex = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)).ok()?;
    let (w, h) = (tex.width(), tex.height());
    let mut data = vec![0u8; (w * h * 4) as usize];
    tex.download(&mut data, w as usize * 4); // default format = B8G8R8A8 premultiplied = cairo ARgb32 (LE)
    ImageSurface::create_for_data(data, Format::ARgb32, w, h, w * 4).ok()
}

fn png(s: &mut ImageSurface) -> Vec<u8> {
    let (w, h, st) = (s.width(), s.height(), s.stride() as usize);
    let data = s.data().map(|d| d.to_vec()).unwrap_or_default();
    gdk::MemoryTexture::new(w, h, gdk::MemoryFormat::B8g8r8a8Premultiplied, &glib::Bytes::from_owned(data), st).save_to_png_bytes().to_vec()
}

/// Largest centred fit of `iw`×`ih` in `aw`×`ah`: (scale, x offset, y offset).
fn fit(iw: f64, ih: f64, aw: f64, ah: f64) -> (f64, f64, f64) {
    let k = (aw / iw).min(ah / ih).max(1e-6);
    (k, (aw - iw * k) / 2.0, (ah - ih * k) / 2.0)
}

fn window(parent: Option<&gtk::Window>, title: &str) -> (adw::Window, adw::HeaderBar, gtk::DrawingArea, adw::ToolbarView) {
    let win = adw::Window::builder().title(title).modal(true).default_width(1100).default_height(800).build();
    if let Some(p) = parent {
        win.set_transient_for(Some(p));
        win.set_default_size((p.width() as f64 * 0.9) as i32, (p.height() as f64 * 0.9) as i32);
    }
    let head = adw::HeaderBar::new();
    let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
    let tv = adw::ToolbarView::builder().content(&area).build();
    tv.add_top_bar(&head);
    win.set_content(Some(&tv));
    let esc = gtk::EventControllerKey::new();
    let w = win.clone();
    esc.connect_key_pressed(move |_, k, _, _| {
        if k == gdk::Key::Escape {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(esc);
    (win, head, area, tv)
}

fn save_as(win: &adw::Window, name: &str, bytes: Vec<u8>) {
    let dlg = gtk::FileDialog::builder().title("Save picture").initial_name(name).build();
    dlg.save(Some(win), gio::Cancellable::NONE, move |r| {
        if let Some(p) = r.ok().and_then(|f| f.path()) {
            let _ = std::fs::write(p, &bytes);
        }
    });
}

/// Before/after slider: A on the left of the divider, B on the right. Drag to move it.
pub fn compare(parent: Option<&gtk::Window>, (an, a): (&str, &[u8]), (bn, b): (&str, &[u8])) -> Result<(), String> {
    let (sa, sb) = (surface(a).ok_or("cannot decode A")?, surface(b).ok_or("cannot decode B")?);
    let dims = (sa.width() as f64, sa.height() as f64);
    let (win, _head, area, _) = window(parent, &format!("Compare — {an} | {bn}"));
    let split = Rc::new(Cell::new(0.5f64));
    let (names, s) = ((an.to_owned(), bn.to_owned()), split.clone());
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let (iw, ih) = (sa.width() as f64, sa.height() as f64);
        let (k, ox, oy) = fit(iw, ih, w, h);
        cr.set_source_rgb(0.08, 0.08, 0.08);
        cr.paint().ok();
        let x = ox + iw * k * s.get();
        for (img, left) in [(&sa, true), (&sb, false)] {
            cr.save().ok();
            if left { cr.rectangle(0.0, 0.0, x, h) } else { cr.rectangle(x, 0.0, w - x, h) }
            cr.clip();
            cr.translate(ox, oy);
            // B is stretched onto A's frame so the two line up whatever their sizes.
            cr.scale(k * iw / img.width() as f64, k * ih / img.height() as f64);
            cr.set_source_surface(img, 0.0, 0.0).ok();
            cr.paint().ok();
            cr.restore().ok();
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.move_to(x, oy);
        cr.line_to(x, oy + ih * k);
        cr.stroke().ok();
        cr.arc(x, h / 2.0, 14.0, 0.0, std::f64::consts::TAU);
        cr.fill().ok();
        cr.set_source_rgb(0.1, 0.1, 0.1);
        cr.select_font_face("sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
        cr.set_font_size(18.0);
        cr.move_to(x - 7.0, h / 2.0 + 6.0);
        cr.show_text("↔").ok();
        for (t, px) in [(&names.0, ox + 10.0), (&names.1, x + 10.0)] {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            cr.rectangle(px - 4.0, oy + 6.0, 9.0 * t.chars().count().min(28) as f64 + 8.0, 24.0);
            cr.fill().ok();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_font_size(14.0);
            cr.move_to(px, oy + 23.0);
            cr.show_text(&t.chars().take(28).collect::<String>()).ok();
        }
    });
    let drag = gtk::GestureDrag::new();
    let origin = Rc::new(Cell::new(0.0));
    let move_to = {
        let (area, split, (iw, ih)) = (area.clone(), split.clone(), dims);
        move |x: f64| {
            let (w, h) = (area.width() as f64, area.height() as f64);
            let (k, ox, _) = fit(iw, ih, w, h);
            split.set(((x - ox) / (iw * k)).clamp(0.0, 1.0));
            area.queue_draw();
        }
    };
    let (m1, o1) = (move_to.clone(), origin.clone());
    drag.connect_drag_begin(move |_, x, _| {
        o1.set(x);
        m1(x);
    });
    drag.connect_drag_update(move |_, dx, _| move_to(origin.get() + dx));
    area.add_controller(drag);
    win.present();
    Ok(())
}

/// Drag a rectangle over the picture, then "Use in workflow" / "Save As…" with just that part.
pub fn crop(parent: Option<&gtk::Window>, name: &str, bytes: &[u8], done: impl Fn(String, Vec<u8>) + 'static) -> Result<(), String> {
    let src = Rc::new(RefCell::new(surface(bytes).ok_or("cannot decode the picture")?));
    let (iw, ih) = (src.borrow().width() as f64, src.borrow().height() as f64);
    let (win, head, area, _) = window(parent, &format!("Crop — {name}"));
    // Selection in image pixels: x0, y0, x1, y1 (None until the first drag).
    let sel: Rc<Cell<Option<(f64, f64, f64, f64)>>> = Rc::default();
    let (s, sf) = (sel.clone(), src.clone());
    area.set_draw_func(move |_, cr, w, h| {
        let (k, ox, oy) = fit(iw, ih, w as f64, h as f64);
        cr.set_source_rgb(0.08, 0.08, 0.08);
        cr.paint().ok();
        cr.translate(ox, oy);
        cr.scale(k, k);
        cr.set_source_surface(&*sf.borrow(), 0.0, 0.0).ok();
        cr.paint().ok();
        if let Some((x0, y0, x1, y1)) = s.get() {
            let (l, t, r, b) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
            cr.rectangle(0.0, 0.0, iw, ih);
            cr.rectangle(l, t, r - l, b - t);
            cr.fill().ok();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(2.0 / k);
            cr.rectangle(l, t, r - l, b - t);
            cr.stroke().ok();
        }
    });
    let info = gtk::Label::builder().css_classes(["dim-label", "numeric"]).label("Drag over the picture to choose the area").build();
    head.pack_start(&info);
    let drag = gtk::GestureDrag::new();
    let to_img = {
        let area = area.clone();
        move |x: f64, y: f64| {
            let (k, ox, oy) = fit(iw, ih, area.width() as f64, area.height() as f64);
            (((x - ox) / k).clamp(0.0, iw), ((y - oy) / k).clamp(0.0, ih))
        }
    };
    let start = Rc::new(Cell::new((0.0, 0.0)));
    {
        let (start, sel, to_img) = (start.clone(), sel.clone(), to_img.clone());
        drag.connect_drag_begin(move |_, x, y| {
            start.set(to_img(x, y));
            sel.set(None);
        });
    }
    {
        let (start, sel, area, info) = (start.clone(), sel.clone(), area.clone(), info.clone());
        drag.connect_drag_update(move |g, dx, dy| {
            let (sx, sy) = g.start_point().unwrap_or_default();
            let (x1, y1) = to_img(sx + dx, sy + dy);
            let (x0, y0) = start.get();
            sel.set(Some((x0, y0, x1, y1)));
            info.set_text(&format!("{} × {} px", (x1 - x0).abs().round(), (y1 - y0).abs().round()));
            area.queue_draw();
        });
    }
    area.add_controller(drag);

    let cropped = {
        let (src, sel) = (src.clone(), sel.clone());
        let name = name.to_owned();
        move || -> Option<(String, Vec<u8>)> {
            let (x0, y0, x1, y1) = sel.get()?;
            let (l, t) = (x0.min(x1).round(), y0.min(y1).round());
            let (w, h) = ((x0 - x1).abs().round() as i32, (y0 - y1).abs().round() as i32);
            if w < 1 || h < 1 {
                return None;
            }
            let mut out = ImageSurface::create(Format::ARgb32, w, h).ok()?;
            let cr = Context::new(&out).ok()?;
            cr.set_source_surface(&*src.borrow(), -l, -t).ok()?;
            cr.paint().ok()?;
            drop(cr);
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
            Some((format!("{stem}_crop.png"), png(&mut out)))
        }
    };
    let save = gtk::Button::with_label("Save As…");
    let apply = gtk::Button::builder().label("Use in Workflow").css_classes(["suggested-action"]).build();
    {
        let (w2, f2) = (win.clone(), cropped.clone());
        save.connect_clicked(move |_| {
            if let Some((n, b)) = f2() {
                save_as(&w2, &n, b);
            }
        });
        let win = win.clone();
        apply.connect_clicked(move |_| {
            if let Some((n, b)) = cropped() {
                done(n, b);
                win.close();
            }
        });
    }
    head.pack_end(&apply);
    head.pack_end(&save);
    win.present();
    Ok(())
}

struct Stroke {
    rgba: gdk::RGBA,
    width: f64,
    pts: Vec<(f64, f64)>,
}

fn draw_stroke(cr: &Context, s: &Stroke, from: usize) {
    cr.set_source_rgba(s.rgba.red() as f64, s.rgba.green() as f64, s.rgba.blue() as f64, s.rgba.alpha() as f64);
    cr.set_line_width(s.width);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    let mut it = s.pts.iter().skip(from.saturating_sub(1));
    if let Some(p) = it.next() {
        cr.move_to(p.0, p.1);
        if s.pts.len() == 1 {
            cr.line_to(p.0 + 0.01, p.1);
        }
    }
    for p in it {
        cr.line_to(p.0, p.1);
    }
    cr.stroke().ok();
}

/// Freehand painting over the picture. Undo = drop the last stroke.
pub fn paint(parent: Option<&gtk::Window>, name: &str, bytes: &[u8], done: impl Fn(String, Vec<u8>) + 'static) -> Result<(), String> {
    let base = surface(bytes).ok_or("cannot decode the picture")?;
    let (iw, ih) = (base.width() as f64, base.height() as f64);
    // `art` = base + all strokes; strokes stay in a list so undo can rebuild it.
    let art = Rc::new(RefCell::new(base.clone()));
    let strokes: Rc<RefCell<Vec<Stroke>>> = Rc::default();
    let (win, head, area, _) = window(parent, &format!("Paint — {name}"));
    let color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    color.set_rgba(&gdk::RGBA::new(0.9, 0.1, 0.1, 1.0));
    let size = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 200.0, 1.0);
    size.set_value((iw.max(ih) / 80.0).round().max(4.0));
    size.set_size_request(160, -1);
    size.set_tooltip_text(Some("Brush size (image pixels)"));
    let undo = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Undo stroke").build();
    head.pack_start(&color);
    head.pack_start(&size);
    head.pack_start(&undo);

    let a = art.clone();
    area.set_draw_func(move |_, cr, w, h| {
        let (k, ox, oy) = fit(iw, ih, w as f64, h as f64);
        cr.set_source_rgb(0.08, 0.08, 0.08);
        cr.paint().ok();
        cr.translate(ox, oy);
        cr.scale(k, k);
        cr.set_source_surface(&*a.borrow(), 0.0, 0.0).ok();
        cr.paint().ok();
    });
    let to_img = {
        let area = area.clone();
        move |x: f64, y: f64| {
            let (k, ox, oy) = fit(iw, ih, area.width() as f64, area.height() as f64);
            ((x - ox) / k, (y - oy) / k)
        }
    };
    let drag = gtk::GestureDrag::new();
    {
        let (strokes, art, area, color, size, to_img) = (strokes.clone(), art.clone(), area.clone(), color.clone(), size.clone(), to_img.clone());
        drag.connect_drag_begin(move |_, x, y| {
            let s = Stroke { rgba: color.rgba(), width: size.value(), pts: vec![to_img(x, y)] };
            if let Ok(cr) = Context::new(&*art.borrow()) {
                draw_stroke(&cr, &s, 0);
            }
            strokes.borrow_mut().push(s);
            area.queue_draw();
        });
    }
    {
        let (strokes, art, area) = (strokes.clone(), art.clone(), area.clone());
        drag.connect_drag_update(move |g, dx, dy| {
            let (sx, sy) = g.start_point().unwrap_or_default();
            let mut st = strokes.borrow_mut();
            if let Some(s) = st.last_mut() {
                s.pts.push(to_img(sx + dx, sy + dy));
                if let Ok(cr) = Context::new(&*art.borrow()) {
                    draw_stroke(&cr, s, s.pts.len() - 1);
                }
            }
            area.queue_draw();
        });
    }
    area.add_controller(drag);
    {
        let (strokes, art, area, base) = (strokes.clone(), art.clone(), area.clone(), base.clone());
        undo.connect_clicked(move |_| {
            strokes.borrow_mut().pop();
            let fresh = base.clone();
            if let Ok(cr) = Context::new(&fresh) {
                for s in strokes.borrow().iter() {
                    draw_stroke(&cr, s, 0);
                }
            }
            *art.borrow_mut() = fresh;
            area.queue_draw();
        });
    }

    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_owned();
    let result = {
        let art = art.clone();
        move || (format!("{stem}_paint.png"), png(&mut art.borrow_mut()))
    };
    let save = gtk::Button::with_label("Save As…");
    let apply = gtk::Button::builder().label("Use in Workflow").css_classes(["suggested-action"]).build();
    {
        let (w2, f2) = (win.clone(), result.clone());
        save.connect_clicked(move |_| {
            let (n, b) = f2();
            save_as(&w2, &n, b);
        });
        let win = win.clone();
        apply.connect_clicked(move |_| {
            let (n, b) = result();
            done(n, b);
            win.close();
        });
    }
    head.pack_end(&apply);
    head.pack_end(&save);
    win.present();
    Ok(())
}
