// SPDX-License-Identifier: GPL-3.0-only
// Comfy-GNOME — native GTK4/Libadwaita frontend for ComfyUI.
// Canvas design adapted from Helvum (c) Tom A. Wagner, GPL-3.0-only.

mod api;
mod backend;
mod dasiwa;
mod erenodes;
mod graph_view;
mod model_preview;
mod pngmeta;
mod prompt;
mod registry;
mod settings_defs;
mod workflow;
mod canvas;
mod splash;
mod workspace;

use adw::prelude::*;

pub const APP_ID: &str = "io.github.comfygnome.ComfyGnome";

fn gtk_weak_upgrade(w: &adw::gtk::glib::WeakRef<adw::ApplicationWindow>) -> Option<adw::ApplicationWindow> {
    w.upgrade()
}

fn main() -> adw::glib::ExitCode {
    // Bundled Adwaita symbolic icons. GTK picks up <resource-base-path>/icons
    // (derived from APP_ID) automatically, so they work under any icon theme.
    adw::gio::resources_register_include!("comfy-gnome.gresource").expect("register resources");
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

thread_local! {
    /// The workspace window and workspace. Dialogs (Settings, Templates) are registered with the
    /// application too, so a second launch used to present whichever of them had focus last —
    /// which reads as "the app only opens Settings". Activation now always raises the workspace.
    static MAIN: std::cell::RefCell<Option<adw::gtk::glib::WeakRef<adw::ApplicationWindow>>> = const { std::cell::RefCell::new(None) };
    static WS: std::cell::RefCell<Option<workspace::Workspace>> = const { std::cell::RefCell::new(None) };
}

fn build_window(app: &adw::Application) {
    if let Some(win) = MAIN.with(|m| m.borrow().as_ref().and_then(gtk_weak_upgrade)) {
        if let Some(ws) = WS.with(|w| w.borrow().clone()) {
            ws.hide_dialogs();
        }
        win.present();
        return;
    }
    let stack = adw::gtk::Stack::builder()
        .transition_type(adw::gtk::StackTransitionType::Crossfade)
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Comfy-GNOME")
        .default_width(1280)
        .default_height(800)
        .content(&stack)
        .build();

    let ws = workspace::Workspace::new();
    WS.with(|w| *w.borrow_mut() = Some(ws.clone()));
    stack.add_named(&ws.root, Some("workspace"));
    {
        let ws = ws.clone();
        // Not connect_destroy: it never fires when the app quits via the last window closing.
        window.connect_close_request(move |_| {
            ws.stop_backend();
            adw::glib::Propagation::Proceed
        });
    }
    {
        // Safety net for the other exit paths (Ctrl+Q from the app menu, D-Bus quit,
        // a crash in another window): never leave a spawned ComfyUI behind.
        let ws = ws.clone();
        app.connect_shutdown(move |_| ws.stop_backend());
    }

    let stack_c = stack.clone();
    let splash = splash::build(&window, move |cfg| {
        stack_c.set_visible_child_name("workspace");
        ws.start(cfg);
    });
    stack.add_named(&splash, Some("splash"));
    stack.set_visible_child_name("splash");
    MAIN.with(|m| *m.borrow_mut() = Some(window.downgrade()));
    window.present();
}
