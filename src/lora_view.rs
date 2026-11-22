//! LoRA Manager (comfyui-lora-manager) is a web app its backend serves at `/loras`; there is
//! no native equivalent. Native ComfyUI hands it to a browser, which a Flatpak sandbox cannot
//! see (and which the user asked us not to do), so it runs in a WebKit view of our own — the
//! same WebKitGTK build the implant already links.

use adw::prelude::*;
use adw::glib;
use webkit::prelude::*;

thread_local! {
    /// One window, reused: the page holds the manager's SPA state (filters, scroll).
    static VIEW: std::cell::RefCell<Option<adw::Window>> = const { std::cell::RefCell::new(None) };
}

pub fn open(parent: Option<&gtk::Window>, url: &str) {
    VIEW.with(|slot| {
        if let Some(w) = slot.borrow().as_ref() {
            w.present();
            return;
        }
        let win = adw::Window::builder().title("LoRA Manager").default_width(1400).default_height(900).build();
        if let Some(p) = parent {
            win.set_transient_for(Some(p));
        }
        let head = adw::HeaderBar::new();
        let view = webkit::WebView::builder().build();
        // Matches the implant: software rendering, this machine's GL stack stalls WebKit.
        if let Some(s) = webkit::prelude::WebViewExt::settings(&view) {
            s.set_hardware_acceleration_policy(webkit::HardwareAccelerationPolicy::Never);
        }
        let tv = adw::ToolbarView::builder().content(&view).build();
        tv.add_top_bar(&head);
        win.set_content(Some(&tv));
        view.load_uri(url);
        let w = win.clone();
        win.connect_close_request(move |_| {
            VIEW.with(|s| s.borrow_mut().take());
            w.destroy();
            glib::Propagation::Proceed
        });
        win.present();
        slot.replace(Some(win));
    });
}
