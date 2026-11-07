// SPDX-License-Identifier: GPL-3.0-only
//! Splash / launch-configuration screen.

use crate::backend::{self, LaunchConfig, DEFAULT_HOST};
use adw::{gio, gtk, prelude::*};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub fn build(window: &adw::ApplicationWindow, on_start: impl Fn(LaunchConfig) + 'static) -> gtk::Widget {
    let on_start = Rc::new(on_start);
    let saved = LaunchConfig::load();
    let script: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(saved.script.clone()));
    let comfy_dir: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(saved.comfy_dir.clone()));

    let icon_button = |icon: &str, tip: &str| {
        gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).css_classes(["flat"]).build()
    };

    // --- ComfyUI folder row (auto-detected) ---------------------------------
    let dir_row = adw::ActionRow::builder()
        .title("ComfyUI Folder")
        .subtitle(match &*comfy_dir.borrow() {
            Some(d) => d.display().to_string(),
            None => "Not found — select your ComfyUI folder".into(),
        })
        .build();
    let browse_dir = icon_button("folder-symbolic", "Select Folder…");
    dir_row.add_suffix(&browse_dir);
    dir_row.set_activatable_widget(Some(&browse_dir));

    // --- launch script row (optional, overrides folder) ---------------------
    let script_row = adw::ActionRow::builder()
        .title("Launch Script (optional)")
        .subtitle("Runs instead of the folder's main.py")
        .build();
    let clear = icon_button("edit-clear-symbolic", "Clear Script");
    clear.set_visible(false);
    let browse = icon_button("document-open-symbolic", "Browse…");
    script_row.add_suffix(&clear);
    script_row.add_suffix(&browse);
    script_row.set_activatable_widget(Some(&browse));

    let args_row = adw::EntryRow::builder().title("Extra Arguments (e.g. --lowvram --listen)").text(&saved.args).build();
    let host_row = adw::EntryRow::builder().title("Host").text(&saved.host).build();
    let port_row = adw::SpinRow::builder()
        .title("Port")
        .adjustment(&gtk::Adjustment::new(saved.port as f64, 1.0, 65535.0, 1.0, 10.0, 0.0))
        .build();

    let launch_group = adw::PreferencesGroup::builder()
        .title("Launch ComfyUI")
        .description("Start ComfyUI from its folder, or with your own script")
        .build();
    launch_group.add(&dir_row);
    launch_group.add(&script_row);
    launch_group.add(&args_row);
    let suppress_row = adw::SwitchRow::builder()
        .title("Block Browser Launch")
        .subtitle("Stop launch scripts from opening Thorium, Chromium, xdg-open…")
        .active(saved.suppress_browser)
        .build();
    launch_group.add(&suppress_row);

    let conn_group = adw::PreferencesGroup::builder().title("Backend Address").build();
    conn_group.add(&host_row);
    conn_group.add(&port_row);

    // --- buttons -------------------------------------------------------------
    let launch_btn = gtk::Button::builder()
        .label("Launch ComfyUI")
        .sensitive(comfy_dir.borrow().is_some())
        .css_classes(["pill", "suggested-action"])
        .build();
    let skip_btn = gtk::Button::builder()
        .label("Skip — Connect to Running Backend")
        .css_classes(["pill"])
        .build();
    // Bottom bar: always visible regardless of window height.
    let buttons = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    buttons.append(&skip_btn);
    buttons.append(&launch_btn);

    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(24).build();
    body.append(&launch_group);
    body.append(&conn_group);

    let status = adw::StatusPage::builder()
        .icon_name("applications-graphics-symbolic")
        .title("Comfy-GNOME")
        .description("Native GTK frontend for ComfyUI")
        .child(&adw::Clamp::builder().maximum_size(520).child(&body).build())
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    toolbar.set_content(Some(&gtk::ScrolledWindow::builder().child(&status).build()));
    toolbar.add_bottom_bar(&buttons);
    toolbar.set_bottom_bar_style(adw::ToolbarStyle::Raised);

    // --- behaviour -----------------------------------------------------------
    let refresh = {
        let (script, comfy_dir, launch_btn, clear, script_row) =
            (script.clone(), comfy_dir.clone(), launch_btn.clone(), clear.clone(), script_row.clone());
        move || {
            let s = script.borrow();
            launch_btn.set_sensitive(s.is_some() || comfy_dir.borrow().is_some());
            clear.set_visible(s.is_some());
            script_row.set_subtitle(&match &*s {
                Some(p) => p.display().to_string(),
                None => "Runs instead of the folder's main.py".into(),
            });
        }
    };
    {
        let (window, comfy_dir, dir_row, refresh) = (window.clone(), comfy_dir.clone(), dir_row.clone(), refresh.clone());
        browse_dir.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select ComfyUI Folder").modal(true).build();
            if let Some(d) = &*comfy_dir.borrow() {
                dialog.set_initial_folder(Some(&gio::File::for_path(d)));
            }
            let (comfy_dir, dir_row, refresh) = (comfy_dir.clone(), dir_row.clone(), refresh.clone());
            dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |res| {
                let Some(path) = res.ok().and_then(|f| f.path()) else { return };
                if backend::is_comfy_dir(&path) {
                    dir_row.set_subtitle(&path.display().to_string());
                    *comfy_dir.borrow_mut() = Some(path);
                } else {
                    dir_row.set_subtitle(&format!("⚠ No ComfyUI main.py in {}", path.display()));
                    *comfy_dir.borrow_mut() = None;
                }
                refresh();
            });
        });
    }
    {
        let (window, script, comfy_dir, refresh) = (window.clone(), script.clone(), comfy_dir.clone(), refresh.clone());
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select ComfyUI Launch Script").modal(true).build();
            if let Some(d) = &*comfy_dir.borrow() {
                dialog.set_initial_folder(Some(&gio::File::for_path(d)));
            }
            let (script, refresh) = (script.clone(), refresh.clone());
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    *script.borrow_mut() = Some(path);
                    refresh();
                }
            });
        });
    }
    refresh();
    {
        let script = script.clone();
        clear.connect_clicked(move |_| {
            *script.borrow_mut() = None;
            refresh();
        });
    }

    let make_cfg = {
        let (host_row, port_row, suppress_row) = (host_row.clone(), port_row.clone(), suppress_row.clone());
        move |script: Option<PathBuf>, comfy_dir: Option<PathBuf>, args: String| {
            let host = host_row.text().trim().to_string();
            LaunchConfig {
                script,
                comfy_dir,
                args,
                host: if host.is_empty() { DEFAULT_HOST.into() } else { host },
                port: port_row.value() as u16,
                suppress_browser: suppress_row.is_active(),
            }
        }
    };
    let current = move || make_cfg(script.borrow().clone(), comfy_dir.borrow().clone(), args_row.text().to_string());
    {
        let (on_start, current) = (on_start.clone(), current.clone());
        skip_btn.connect_clicked(move |_| {
            let cfg = current();
            cfg.save();
            on_start(LaunchConfig { script: None, comfy_dir: None, args: String::new(), ..cfg })
        });
    }
    launch_btn.connect_clicked(move |_| {
        let cfg = current();
        cfg.save();
        on_start(cfg)
    });

    toolbar.upcast()
}
