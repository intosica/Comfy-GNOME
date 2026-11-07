// SPDX-License-Identifier: GPL-3.0-only
//! Splash / launch-configuration screen.

use crate::backend::{LaunchConfig, DEFAULT_HOST, DEFAULT_PORT};
use adw::{gio, gtk, prelude::*};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub fn build(window: &adw::ApplicationWindow, on_start: impl Fn(LaunchConfig) + 'static) -> gtk::Widget {
    let on_start = Rc::new(on_start);
    let script: Rc<RefCell<Option<PathBuf>>> = Rc::default();

    // --- launch script row -------------------------------------------------
    let script_row = adw::ActionRow::builder()
        .title("Launch Script")
        .subtitle("No script selected")
        .build();
    let browse = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Browse…")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    script_row.add_suffix(&browse);
    script_row.set_activatable_widget(Some(&browse));

    let args_row = adw::EntryRow::builder().title("Extra Arguments (e.g. --lowvram --listen)").build();
    let host_row = adw::EntryRow::builder().title("Host").text(DEFAULT_HOST).build();
    let port_row = adw::SpinRow::builder()
        .title("Port")
        .adjustment(&gtk::Adjustment::new(DEFAULT_PORT as f64, 1.0, 65535.0, 1.0, 10.0, 0.0))
        .build();

    let launch_group = adw::PreferencesGroup::builder()
        .title("Custom Launch")
        .description("Start ComfyUI with your own script and parameters")
        .build();
    launch_group.add(&script_row);
    launch_group.add(&args_row);
    let suppress_row = adw::SwitchRow::builder()
        .title("Block Browser Launch")
        .subtitle("Neutralise thorium/chromium/xdg-open calls inside the script")
        .active(true)
        .build();
    launch_group.add(&suppress_row);

    let conn_group = adw::PreferencesGroup::builder().title("Backend Address").build();
    conn_group.add(&host_row);
    conn_group.add(&port_row);

    // --- buttons -------------------------------------------------------------
    let launch_btn = gtk::Button::builder()
        .label("Launch Script")
        .sensitive(false)
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
        .title("GNOME-ComfyUI")
        .description("Native GTK frontend for ComfyUI")
        .child(&adw::Clamp::builder().maximum_size(520).child(&body).build())
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    toolbar.set_content(Some(&gtk::ScrolledWindow::builder().child(&status).build()));
    toolbar.add_bottom_bar(&buttons);
    toolbar.set_bottom_bar_style(adw::ToolbarStyle::Raised);

    // --- behaviour -----------------------------------------------------------
    {
        let (window, script, script_row, launch_btn) =
            (window.clone(), script.clone(), script_row.clone(), launch_btn.clone());
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select ComfyUI Launch Script").modal(true).build();
            let (script, script_row, launch_btn) = (script.clone(), script_row.clone(), launch_btn.clone());
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    script_row.set_subtitle(&path.display().to_string());
                    launch_btn.set_sensitive(true);
                    *script.borrow_mut() = Some(path);
                }
            });
        });
    }

    let make_cfg = {
        let (host_row, port_row, suppress_row) = (host_row.clone(), port_row.clone(), suppress_row.clone());
        move |script: Option<PathBuf>, args: String| {
            let host = host_row.text().trim().to_string();
            LaunchConfig {
                script,
                args,
                host: if host.is_empty() { DEFAULT_HOST.into() } else { host },
                port: port_row.value() as u16,
                suppress_browser: suppress_row.is_active(),
            }
        }
    };
    {
        let (on_start, make_cfg) = (on_start.clone(), make_cfg.clone());
        skip_btn.connect_clicked(move |_| on_start(make_cfg(None, String::new())));
    }
    launch_btn.connect_clicked(move |_| {
        on_start(make_cfg(script.borrow().clone(), args_row.text().to_string()))
    });

    toolbar.upcast()
}
