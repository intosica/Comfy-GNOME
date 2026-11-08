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
    app.connect_startup(|_| match_system_look());
    app.connect_activate(build_window);
    app.run()
}

/// The Flatpak runtime ships a newer libadwaita (other palette) and has no host fonts, so the same
/// binary looked different there. Pin the 1.5 palette the native build uses, and the font the
/// host's generic "Sans" resolves to.
fn match_system_look() {
    use adw::gtk;
    if adw::minor_version() > 5 {
        let provider = gtk::CssProvider::new();
        if let Some(d) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let apply = move |sm: &adw::StyleManager| {
            provider.load_from_string(if sm.is_dark() { include_str!("../data/adw-1.5-dark.css") } else { include_str!("../data/adw-1.5-light.css") })
        };
        let sm = adw::StyleManager::default();
        apply(&sm);
        sm.connect_dark_notify(apply);
    }
    if backend::in_flatpak() {
        // GNOME 50's default UI font differs from the host's; use the one the native build gets.
        if let Some(s) = gtk::Settings::default() {
            let size = s.gtk_font_name().and_then(|f| f.rsplit_once(' ').map(|(_, n)| n.to_owned())).unwrap_or_else(|| "9".into());
            s.set_gtk_font_name(Some(&format!("Noto Sans {size}")));
        }
    }
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
        let app = app.clone();
        // Not connect_destroy: it never fires when the app quits via the last window closing.
        window.connect_close_request(move |_| {
            ws.stop_backend();
            // The hidden Settings/Templates windows are still registered with the app and keep
            // it alive after the main window is gone (zombie process holding the D-Bus name).
            app.quit();
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
