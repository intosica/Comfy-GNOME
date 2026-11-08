// SPDX-License-Identifier: GPL-3.0-only
//! Resource Reaper panel: live VRAM/RAM, manual clean, what-to-close suggestions
//! and the reaper's own settings. Mirrors `resource_reaper.js` from ComfyUI_Cleaner.

use super::Workspace;
use adw::{gtk, prelude::*};
use serde_json::{json, Value};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn fmt(n: f64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut n = n.max(0.0);
    let mut i = 0;
    while n >= 1024.0 && i + 1 < U.len() {
        n /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n:.0} {}", U[i]) } else { format!("{n:.1} {}", U[i]) }
}

fn severity(pct: f64) -> &'static str {
    if pct >= 92.0 {
        "rr-crit"
    } else if pct >= 80.0 {
        "rr-warn"
    } else {
        "rr-ok"
    }
}

fn meter(label: &str, used: f64, total: f64, extra: &str) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.add_css_class("rr-meter");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let l = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).css_classes(["rr-label"]).build();
    let mut value = if total > 0.0 { format!("{} of {}", fmt(used), fmt(total)) } else { "unavailable".into() };
    if !extra.is_empty() {
        value.push_str(" · ");
        value.push_str(extra);
    }
    let v = gtk::Label::builder().label(value).xalign(1.0).justify(gtk::Justification::Right).wrap(true).css_classes(["rr-value"]).build();
    row.append(&l);
    row.append(&v);
    let bar = gtk::ProgressBar::new();
    bar.set_fraction(if total > 0.0 { (used / total).clamp(0.0, 1.0) } else { 0.0 });
    bar.add_css_class(severity(if total > 0.0 { used / total * 100.0 } else { 0.0 }));
    b.append(&row);
    b.append(&bar);
    b.upcast()
}

fn heading(s: &str) -> gtk::Label {
    gtk::Label::builder().label(s).xalign(0.0).margin_top(14).css_classes(["rr-h"]).build()
}

#[derive(Clone)]
struct Reaper {
    ws: Workspace,
    root_page: gtk::Widget,
    platform: gtk::Label,
    meters: gtk::Box,
    note: gtk::Label,
    last: gtk::Label,
    suggestions: gtk::Box,
    psutil_hint: gtk::Label,
    nvml_hint: gtk::Label,
    procs: gtk::Box,
    level: gtk::DropDown,
    clean: gtk::Button,
    auto_clean: gtk::Switch,
    mode: gtk::DropDown,
    adaptive_row: gtk::Box,
    adaptive_pct: gtk::SpinButton,
    purge: gtk::Switch,
    purge_row: gtk::Box,
    purge_every: gtk::SpinButton,
    allow_terminate: gtk::Switch,
    verbose: gtk::Switch,
    busy: Rc<Cell<bool>>,
    syncing: Rc<Cell<bool>>,
}

impl Reaper {
    fn post(&self, path: &'static str, body: Value, done: impl FnOnce(Result<Value, String>) + 'static) {
        let Some(client) = self.ws.client.borrow().clone() else {
            self.note.set_text("Not connected");
            return;
        };
        gtk::glib::spawn_future_local(async move {
            let r = gtk::gio::spawn_blocking(move || client.post(path, body)).await;
            done(match r {
                Ok(v) => v,
                Err(_) => Err("worker panicked".into()),
            });
        });
    }

    fn poll(&self) {
        if self.busy.replace(true) {
            return;
        }
        let Some(client) = self.ws.client.borrow().clone() else {
            self.busy.set(false);
            self.note.set_text("Not connected");
            return;
        };
        let me = self.clone();
        gtk::glib::spawn_future_local(async move {
            let r = gtk::gio::spawn_blocking(move || client.get("/reaper/status")).await;
            me.busy.set(false);
            match r {
                Ok(Ok(v)) => {
                    // Drop a stale connection error; keep result notes from Free memory.
                    let n = me.note.text();
                    if n.starts_with("Not connected") || n.starts_with("Cannot reach") {
                        me.note.set_text("");
                    }
                    me.apply(&v)
                }
                Ok(Err(e)) => me.note.set_text(&format!("Cannot reach Resource Reaper on the server: {e}")),
                Err(_) => me.note.set_text("Resource Reaper worker panicked"),
            }
        });
    }

    fn apply(&self, state: &Value) {
        self.platform.set_text(&text(state, "platform"));
        while let Some(c) = self.meters.first_child() {
            self.meters.remove(&c);
        }
        if let Some(gpus) = state.get("gpus").and_then(Value::as_array) {
            for gpu in gpus {
                let total = num(gpu, "total");
                let mine = gpu
                    .get("processes")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().find(|p| p.get("is_self").and_then(Value::as_bool).unwrap_or(false)))
                    .map(|p| num(p, "vram"))
                    .unwrap_or(0.0);
                let mut extra = vec![format!("{} free", fmt(num(gpu, "free")))];
                if let Some(u) = gpu.get("utilization").and_then(Value::as_f64) {
                    extra.push(format!("{u:.0}% busy"));
                }
                if let Some(t) = gpu.get("temperature").and_then(Value::as_f64) {
                    extra.push(format!("{t:.0}°C"));
                }
                let label = text(gpu, "name");
                self.meters.append(&meter(&label, num(gpu, "used"), total, &extra.join(" · ")));
                if mine > 0.0 {
                    let l = gtk::Label::builder()
                        .label(format!("ComfyUI {} · other programs {}", fmt(mine), fmt((num(gpu, "used") - mine).max(0.0))))
                        .xalign(0.0)
                        .wrap(true)
                        .css_classes(["rr-value"])
                        .build();
                    self.meters.append(&l);
                }
            }
        } else if state.get("torch").map(|t| num(t, "total") > 0.0).unwrap_or(false) {
            let t = &state["torch"];
            self.meters.append(&meter("Graphics memory", num(t, "used"), num(t, "total"), &format!("{} free", fmt(num(t, "free")))));
        }
        let mem = &state["memory"];
        if mem.get("available").and_then(Value::as_bool).unwrap_or(false) {
            self.meters.append(&meter("System memory", num(mem, "used"), num(mem, "total"), &format!("{} free", fmt(num(mem, "free")))));
        }

        self.last.set_text(&match state.get("last_pass") {
            Some(lp) if !lp.is_null() => format!(
                "Last automatic pass: {} before run {}, freed {}.",
                text(lp, "level"),
                lp.get("run").map(|v| v.to_string()).unwrap_or_default(),
                fmt(num(lp, "reclaimed"))
            ),
            _ => String::new(),
        });

        while let Some(c) = self.suggestions.first_child() {
            self.suggestions.remove(&c);
        }
        let cfg = &state["config"];
        let allow = cfg.get("allow_terminate").and_then(Value::as_bool).unwrap_or(false);
        let psutil = state.get("psutil").and_then(Value::as_bool).unwrap_or(true);
        let nvml = state.get("nvml").and_then(Value::as_bool).unwrap_or(true);
        self.psutil_hint.set_visible(!psutil);
        self.nvml_hint.set_visible(psutil && !nvml);
        if psutil {
            for item in state.get("suggestions").and_then(Value::as_array).into_iter().flatten() {
                self.suggestions.append(&self.suggestion(item, allow));
            }
        }

        while let Some(c) = self.procs.first_child() {
            self.procs.remove(&c);
        }
        for p in state.get("processes").and_then(Value::as_array).into_iter().flatten() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let mut name = text(p, "name");
            if p.get("is_self").and_then(Value::as_bool).unwrap_or(false) {
                name.push_str(" (this ComfyUI)");
            }
            let count = p.get("count").and_then(Value::as_i64).unwrap_or(1);
            if count > 1 {
                name.push_str(&format!(" · {count} processes"));
            }
            row.append(&gtk::Label::builder().label(name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
            row.append(&gtk::Label::builder().label(fmt(num(p, "rss"))).xalign(1.0).css_classes(["rr-value"]).build());
            self.procs.append(&row);
        }

        self.sync_settings(cfg);
    }

    fn suggestion(&self, item: &Value, allow: bool) -> gtk::Widget {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 3);
        b.add_css_class("rr-item");
        // Server severities: critical | warn | info.
        b.add_css_class(match text(item, "severity").as_str() {
            "critical" => "rr-crit",
            "warn" => "rr-warn",
            _ => "rr-ok",
        });
        b.append(&gtk::Label::builder().label(text(item, "title")).xalign(0.0).wrap(true).css_classes(["rr-title"]).build());
        b.append(&gtk::Label::builder().label(text(item, "detail")).xalign(0.0).wrap(true).css_classes(["rr-value"]).build());
        let pid = item.get("pid").and_then(Value::as_i64).unwrap_or(0);
        if allow && pid > 0 {
            let btn = gtk::Button::with_label(&format!("Close {}", text(item, "process")));
            btn.add_css_class("rr-btn");
            let me = self.clone();
            let proc_name = text(item, "process");
            btn.connect_clicked(move |_| {
                let dlg = adw::AlertDialog::builder()
                    .heading(format!("Close {proc_name}?"))
                    .body(format!("pid {pid}\n\nUnsaved work in that program will be lost."))
                    .build();
                dlg.add_responses(&[("cancel", "Cancel"), ("ok", "Close")]);
                dlg.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
                dlg.set_close_response("cancel");
                let me2 = me.clone();
                let proc_name = proc_name.clone();
                dlg.connect_response(None, move |_, r| {
                    if r != "ok" {
                        return;
                    }
                    let me3 = me2.clone();
                    let proc_name = proc_name.clone();
                    me2.post("/reaper/terminate", json!({"pid": pid}), move |res| match res {
                        Ok(v) => {
                            let n = text(&v, "name");
                            me3.note.set_text(&format!("Closed {}.", if n.is_empty() { &proc_name } else { &n }));
                            me3.poll();
                        }
                        Err(e) => me3.note.set_text(&e),
                    });
                });
                dlg.present(Some(&me.ws.root));
            });
            b.append(&btn);
        }
        b.upcast()
    }

    fn sync_settings(&self, cfg: &Value) {
        self.syncing.set(true);
        self.auto_clean.set_active(cfg.get("auto_clean").and_then(Value::as_bool).unwrap_or(false));
        let mode = text(cfg, "mode");
        let idx = match mode.as_str() {
            "adaptive" => 0,
            "light" => 1,
            "standard" => 2,
            "aggressive" => 3,
            "off" => 4,
            _ => 2,
        };
        self.mode.set_selected(idx);
        self.adaptive_row.set_visible(mode == "adaptive");
        self.adaptive_pct.set_value(num(cfg, "adaptive_free_vram_pct"));
        self.purge.set_active(cfg.get("purge_execution_cache").and_then(Value::as_bool).unwrap_or(false));
        self.purge_row.set_visible(self.purge.is_active());
        self.purge_every.set_value(num(cfg, "purge_every_n_runs").max(1.0));
        self.allow_terminate.set_active(cfg.get("allow_terminate").and_then(Value::as_bool).unwrap_or(false));
        self.verbose.set_active(cfg.get("verbose").and_then(Value::as_bool).unwrap_or(false));
        self.syncing.set(false);
    }

    fn push(&self, key: &'static str, value: Value) {
        let me = self.clone();
        self.post("/reaper/config", json!({key: value}), move |res| {
            if let Err(e) = res {
                me.note.set_text(&e);
            }
            me.poll();
        });
    }
}

fn switch_row(title: &str, hint: &str) -> (adw::ActionRow, gtk::Switch) {
    let row = adw::ActionRow::builder().title(title).subtitle(hint).build();
    let sw = gtk::Switch::builder().valign(gtk::Align::Center).build();
    row.add_suffix(&sw);
    row.set_activatable_widget(Some(&sw));
    (row, sw)
}

pub(super) fn page(ws: &Workspace) -> gtk::Widget {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let sc = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
    sc.set_child(Some(&page));
    page.add_css_class("rr");
    page.set_margin_top(10);
    page.set_margin_bottom(20);
    page.set_margin_start(12);
    page.set_margin_end(12);

    let platform = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["rr-value"]).build();
    page.append(&platform);
    let meters = gtk::Box::new(gtk::Orientation::Vertical, 10);
    page.append(&meters);

    page.append(&heading("Free memory now"));
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let level = gtk::DropDown::from_strings(&[
        "Light",
        "Standard",
        "Deep",
    ]);
    level.set_selected(1);
    level.set_tooltip_text(Some("Light keeps models loaded · Standard releases unused models · Deep unloads everything"));
    level.set_hexpand(true);
    let clean = gtk::Button::with_label("Free memory");
    clean.add_css_class("suggested-action");
    controls.append(&level);
    controls.append(&clean);
    page.append(&controls);
    let note = gtk::Label::builder().xalign(0.0).wrap(true).build();
    note.add_css_class("rr-note");
    page.append(&note);
    let last = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["rr-value"]).build();
    page.append(&last);

    page.append(&heading("What to close"));
    let suggestions = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.append(&suggestions);
    let psutil_hint = gtk::Label::builder()
        .label("Install psutil to see what else is using this machine: pip install psutil")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["rr-value"])
        .build();
    page.append(&psutil_hint);
    let nvml_hint = gtk::Label::builder()
        .label("Per-program graphics memory needs NVML: pip install nvidia-ml-py")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["rr-value"])
        .build();
    page.append(&nvml_hint);

    page.append(&heading("Biggest memory users"));
    let procs = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page.append(&procs);

    page.append(&heading("Settings"));
    let (auto_row, auto_clean) = switch_row("Free memory before every run", "Runs a pass on the queue worker, before any node executes.");
    page.append(&auto_row);
    let mode_row = adw::ActionRow::builder().title("How hard to clean").subtitle("Adaptive cleans only when memory is tight; Never turns it off").build();
    let mode = gtk::DropDown::from_strings(&[
        "Adaptive",
        "Light",
        "Standard",
        "Deep",
        "Never",
    ]);
    mode.set_valign(gtk::Align::Center);
    mode_row.add_suffix(&mode);
    page.append(&mode_row);

    let adaptive_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    adaptive_row.set_margin_start(12);
    adaptive_row.append(&gtk::Label::builder().label("Clean deeply below").xalign(0.0).hexpand(true).build());
    let adaptive_pct = gtk::SpinButton::with_range(5.0, 90.0, 5.0);
    adaptive_pct.set_value(20.0);
    adaptive_row.append(&adaptive_pct);
    adaptive_row.append(&gtk::Label::new(Some("% free VRAM")));
    page.append(&adaptive_row);

    let (purge_row, purge) = switch_row("Also drop cached node results", "Reclaims the most memory and forces the whole graph to run again next time.");
    page.append(&purge_row);
    let purge_every_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    purge_every_row.set_margin_start(12);
    purge_every_row.append(&gtk::Label::builder().label("Drop the cache every").xalign(0.0).hexpand(true).build());
    let purge_every = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    purge_every.set_value(10.0);
    purge_every_row.append(&purge_every);
    purge_every_row.append(&gtk::Label::new(Some("runs")));
    page.append(&purge_every_row);

    let (term_row, allow_terminate) = switch_row(
        "Let this panel close other programs",
        "Adds a close button to suggestions. Sends a normal quit request, never a force kill.",
    );
    page.append(&term_row);
    let (verbose_row, verbose) = switch_row("Log each pass to the console", "");
    page.append(&verbose_row);

    let r = Rc::new(Reaper {
        ws: ws.clone(),
        root_page: sc.clone().upcast(),
        platform,
        meters,
        note,
        last,
        suggestions,
        psutil_hint,
        nvml_hint,
        procs,
        level,
        clean,
        auto_clean,
        mode,
        adaptive_row,
        adaptive_pct,
        purge,
        purge_row: purge_every_row,
        purge_every,
        allow_terminate,
        verbose,
        busy: Rc::default(),
        syncing: Rc::default(),
    });

    {
        let w = r.clean.clone();
        let r = r.clone();
        w.connect_clicked(move |_| {
            if r.busy.replace(true) {
                return;
            }
            r.clean.set_sensitive(false);
            r.note.set_text("Working…");
            let level = ["light", "standard", "aggressive"][r.level.selected().min(2) as usize];
            let purge = r.purge.is_active();
            let r2 = r.clone();
            r.post("/reaper/clean", json!({"level": level, "purge_execution_cache": purge}), move |res| {
                r2.busy.set(false);
                r2.clean.set_sensitive(true);
                match res {
                    Ok(v) => {
                        let reclaimed = num(&v, "reclaimed");
                        let gained = if reclaimed > 0.0 { format!("Freed {}", fmt(reclaimed)) } else { "Nothing left to free".into() };
                        let after = v.get("after").map(|a| num(a, "free")).unwrap_or(0.0);
                        r2.note.set_text(&format!("{gained} · {} now available · {} ms", fmt(after), num(&v, "ms")));
                        r2.poll();
                    }
                    Err(e) => r2.note.set_text(&e),
                }
            });
        });
    }
    {
        let w = r.auto_clean.clone();
        let r = r.clone();
        w.connect_active_notify(move |s| {
            if !r.syncing.get() {
                r.push("auto_clean", json!(s.is_active()));
            }
        });
    }
    {
        let w = r.mode.clone();
        let r = r.clone();
        w.connect_selected_notify(move |d| {
            if r.syncing.get() {
                return;
            }
            let mode = ["adaptive", "light", "standard", "aggressive", "off"][d.selected().min(4) as usize];
            r.adaptive_row.set_visible(mode == "adaptive");
            r.push("mode", json!(mode));
        });
    }
    {
        let w = r.adaptive_pct.clone();
        let r = r.clone();
        w.connect_value_changed(move |s| {
            if !r.syncing.get() {
                r.push("adaptive_free_vram_pct", json!(s.value()));
            }
        });
    }
    {
        let w = r.purge.clone();
        let r = r.clone();
        w.connect_active_notify(move |s| {
            if !r.syncing.get() {
                r.purge_row.set_visible(s.is_active());
                r.push("purge_execution_cache", json!(s.is_active()));
            }
        });
    }
    {
        let w = r.purge_every.clone();
        let r = r.clone();
        w.connect_value_changed(move |s| {
            if !r.syncing.get() {
                r.push("purge_every_n_runs", json!(s.value() as i64));
            }
        });
    }
    {
        let w = r.allow_terminate.clone();
        let r = r.clone();
        w.connect_active_notify(move |s| {
            if !r.syncing.get() {
                r.push("allow_terminate", json!(s.is_active()));
            }
        });
    }
    {
        let w = r.verbose.clone();
        let r = r.clone();
        w.connect_active_notify(move |s| {
            if !r.syncing.get() {
                r.push("verbose", json!(s.is_active()));
            }
        });
    }

    // Poll only while the page is on screen; the timer stays alive with the widget.
    let timer = r.clone();
    gtk::glib::timeout_add_local(Duration::from_secs(3), move || {
        if timer.root_page.is_mapped() {
            timer.poll();
        }
        gtk::glib::ControlFlow::Continue
    });
    r.poll();
    sc.upcast()
}
