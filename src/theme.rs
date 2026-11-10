// SPDX-License-Identifier: GPL-3.0-only
//! Colour palettes in the ComfyUI frontend's own JSON shape (`id`, `name`, `light_theme`,
//! `colors.{node_slot,litegraph_base,comfy_base}`), so a palette exported from the web UI
//! can be dropped into `~/.config/comfy-gnome/themes/*.json` and shows up in Settings.
//! Built-ins live in `data/palettes/` (extracted from frontend 1.45). Adding a theme source
//! later (colour-site import, ...) only has to produce this JSON.

use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

const BUILTIN: [(&str, &str); 6] = [
    ("dark", include_str!("../data/palettes/dark.json")),
    ("light", include_str!("../data/palettes/light.json")),
    ("solarized", include_str!("../data/palettes/solarized.json")),
    ("arc", include_str!("../data/palettes/arc.json")),
    ("nord", include_str!("../data/palettes/nord.json")),
    ("github", include_str!("../data/palettes/github.json")),
];

#[derive(Default)]
pub struct Palette {
    pub id: String,
    pub name: String,
    pub light: bool,
    slot: HashMap<String, u32>,
    lg: HashMap<String, u32>,
    comfy: HashMap<String, u32>,
}

thread_local! {
    static CURRENT: RefCell<Rc<Palette>> = RefCell::new(Rc::new(parse(BUILTIN[0].1).unwrap_or_default()));
    /// `Comfy.CustomColorPalettes` from the backend: `("custom_<key>", palette json)`, the web's ids.
    static SERVER: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    static CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

fn color(s: &str) -> Option<u32> {
    if s == "lightgray" {
        return Some(0xd3d3d3);
    }
    crate::canvas::parse_color(s)
}

fn group(v: &Value, k: &str) -> HashMap<String, u32> {
    v["colors"][k].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), color(v.as_str()?)?))).collect()
}

pub fn parse(json: &str) -> Option<Palette> {
    let v: Value = serde_json::from_str(json).ok()?;
    Some(Palette {
        id: v["id"].as_str()?.to_owned(),
        name: v["name"].as_str()?.to_owned(),
        light: v["light_theme"].as_bool().unwrap_or(false),
        slot: group(&v, "node_slot"),
        lg: group(&v, "litegraph_base"),
        comfy: group(&v, "comfy_base"),
    })
}

pub fn user_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("comfy-gnome").join("themes")
}

/// Palettes the user made in the web UI live in the server setting `Comfy.CustomColorPalettes`.
pub fn set_server_palettes(v: Option<&Value>) {
    let list = v.and_then(Value::as_object).into_iter().flatten().map(|(k, p)| (format!("custom_{k}"), p.to_string())).collect();
    SERVER.with(|s| *s.borrow_mut() = list);
}

/// `(id, name)`: built-ins first, then the web UI's custom ones, then user files (id = file stem; a stem that shadows a
/// built-in is ignored).
pub fn list() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = BUILTIN.iter().filter_map(|(_, j)| parse(j)).map(|p| (p.id, p.name)).collect();
    let mut files: Vec<_> = std::fs::read_dir(user_dir()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    out.extend(SERVER.with(|s| s.borrow().iter().filter_map(|(i, j)| Some((i.clone(), parse(j)?.name))).collect::<Vec<_>>()));
    for f in files {
        let (Some(stem), Some(p)) = (f.file_stem().and_then(|s| s.to_str()), std::fs::read_to_string(&f).ok().and_then(|j| parse(&j))) else { continue };
        if !out.iter().any(|(i, _)| i == stem) {
            out.push((stem.to_owned(), p.name));
        }
    }
    out
}

fn load(id: &str) -> Option<Palette> {
    if let Some((_, j)) = BUILTIN.iter().find(|(i, _)| *i == id) {
        return parse(j);
    }
    if let Some(j) = SERVER.with(|s| s.borrow().iter().find(|(i, _)| i == id).map(|(_, j)| j.clone())) {
        return parse(&j);
    }
    let p = user_dir().join(format!("{id}.json"));
    parse(&std::fs::read_to_string(p).ok()?)
}

/// Make `id` the live palette (unknown id → dark): canvas colours via `lg`/`slot`, GTK chrome via CSS.
/// Callers redraw the canvases.
pub fn set(id: &str) {
    let p = Rc::new(load(id).or_else(|| load("dark")).unwrap_or_default());
    adw::StyleManager::default().set_color_scheme(if p.light { adw::ColorScheme::ForceLight } else { adw::ColorScheme::ForceDark });
    // dark/light already look like libadwaita; the rest recolour its named colours.
    let hex = |k: &str| format!("#{:06x}", p.comfy.get(k).copied().unwrap_or(0x202020));
    let css = if matches!(p.id.as_str(), "dark" | "light") || p.comfy.is_empty() {
        String::new()
    } else {
        let (bg, menu, sec, input, fg) = (hex("bg-color"), hex("comfy-menu-bg"), hex("comfy-menu-secondary-bg"), hex("comfy-input-bg"), hex("fg-color"));
        format!(
            "@define-color window_bg_color {bg}; @define-color dialog_bg_color {bg}; @define-color window_fg_color {fg};
             @define-color view_bg_color {input}; @define-color view_fg_color {fg};
             @define-color headerbar_bg_color {menu}; @define-color headerbar_fg_color {fg}; @define-color headerbar_backdrop_color {menu};
             @define-color sidebar_bg_color {menu}; @define-color sidebar_fg_color {fg};
             @define-color card_bg_color {sec}; @define-color card_fg_color {fg};
             @define-color popover_bg_color {menu}; @define-color popover_fg_color {fg};"
        )
    };
    CSS.with(|c| {
        let mut c = c.borrow_mut();
        let prov = c.get_or_insert_with(|| {
            let prov = gtk::CssProvider::new();
            if let Some(d) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(&d, &prov, gtk::STYLE_PROVIDER_PRIORITY_USER);
            }
            prov
        });
        prov.load_from_string(&css);
    });
    CURRENT.with(|c| *c.borrow_mut() = p);
}

pub fn current() -> Rc<Palette> {
    CURRENT.with(|c| c.borrow().clone())
}

impl Palette {
    /// A `litegraph_base` colour, `default` when the palette doesn't set it.
    pub fn lg(&self, key: &str, default: u32) -> u32 {
        self.lg.get(key).copied().unwrap_or(default)
    }
    pub fn comfy(&self, key: &str, default: u32) -> u32 {
        self.comfy.get(key).copied().unwrap_or(default)
    }
    /// Socket colour for a type; types the palette doesn't list get the link colour.
    pub fn slot(&self, ty: &str) -> u32 {
        self.slot.get(ty).copied().unwrap_or_else(|| self.lg("LINK_COLOR", 0x99aa99))
    }
}

/// Linear mix of two 0xRRGGBB colours (`t` = share of `b`).
pub fn mix(a: u32, b: u32, t: f64) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xff) as f64, ((b >> s) & 0xff) as f64);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtins_parse() {
        for (id, j) in super::BUILTIN {
            let p = super::parse(j).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(p.id, id);
            assert!(p.lg("NODE_DEFAULT_BGCOLOR", 0) != 0 && !p.comfy.is_empty(), "{id}");
        }
        assert_eq!(super::mix(0, 0xffffff, 0.5), 0x808080);
    }
}
