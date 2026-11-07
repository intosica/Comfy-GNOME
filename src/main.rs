// SPDX-License-Identifier: GPL-3.0-only
// GNOME-ComfyUI — native GTK4/Libadwaita frontend for ComfyUI.
// Canvas design adapted from Helvum (c) Tom A. Wagner, GPL-3.0-only.

mod backend;
mod canvas;
mod splash;
mod workspace;

use adw::prelude::*;

pub const APP_ID: &str = "io.github.gnomecomfyui.GnomeComfyUI";

fn main() -> adw::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    if let Some(win) = app.active_window() {
        win.present();
        return;
    }
    let stack = adw::gtk::Stack::builder()
        .transition_type(adw::gtk::StackTransitionType::Crossfade)
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("GNOME-ComfyUI")
        .default_width(1280)
        .default_height(800)
        .content(&stack)
        .build();

    let ws = workspace::Workspace::new();
    stack.add_named(&ws.root, Some("workspace"));

    let stack_c = stack.clone();
    let splash = splash::build(&window, move |cfg| {
        stack_c.set_visible_child_name("workspace");
        ws.start(cfg);
    });
    stack.add_named(&splash, Some("splash"));
    stack.set_visible_child_name("splash");
    window.present();
}
