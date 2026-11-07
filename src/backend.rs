// SPDX-License-Identifier: GPL-3.0-only
//! Backend process launch + connectivity probe.
//!
//! Phase 1 deliberately uses std + gio only (no tokio/reqwest). The REST/WS
//! client arrives in Phase 2 (see ROADMAP.md).

use adw::{gio, glib, prelude::*};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8188;

#[derive(Debug, Clone)]
pub struct LaunchConfig {
    /// `None` = don't launch anything, connect to an already-running backend.
    pub script: Option<PathBuf>,
    /// Extra arguments, shell-quoted (parsed with g_shell_parse_argv).
    pub args: String,
    pub host: String,
    pub port: u16,
    /// Put no-op browser shims first on the script's PATH.
    pub suppress_browser: bool,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self { script: None, args: String::new(), host: DEFAULT_HOST.into(), port: DEFAULT_PORT, suppress_browser: true }
    }
}

impl LaunchConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

/// Blocking GET /system_stats. Run off the main thread (gio::spawn_blocking).
/// Returns the HTTP status line on success.
pub fn probe(host: &str, port: u16) -> Result<String, String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("could not resolve host")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok();
    write!(s, "GET /system_stats HTTP/1.0\r\nHost: {host}:{port}\r\n\r\n").map_err(|e| e.to_string())?;
    let mut buf = [0u8; 512];
    let n = s.read(&mut buf).map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let status = head.lines().next().unwrap_or("").to_string();
    if status.contains(" 200") { Ok(status) } else { Err(format!("unexpected response: {status}")) }
}

fn in_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// Browsers that user launch scripts typically open on top of the backend.
/// Shims block forever instead of exiting: many scripts run the browser in the
/// foreground and kill ComfyUI from a cleanup trap once it returns.
const BROWSER_SHIMS: &[&str] = &[
    "thorium-browser", "thorium", "chromium", "chromium-browser", "google-chrome",
    "google-chrome-stable", "brave-browser", "firefox", "microsoft-edge",
];
/// Fire-and-forget openers just succeed immediately.
const OPENER_SHIMS: &[&str] = &["xdg-open", "sensible-browser", "x-www-browser", "gio"];

/// Writes the shim dir under XDG_RUNTIME_DIR (inside Flatpak, `$XDG_RUNTIME_DIR/app/<id>`
/// is visible to the host at the same path, so the host-side script can use it).
fn browser_shim_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let base = glib::user_runtime_dir();
    let dir = if in_flatpak() { base.join("app").join(crate::APP_ID) } else { base.join("gnome-comfyui") }
        .join("browser-shim");
    std::fs::create_dir_all(&dir)?;
    for (names, body) in [(BROWSER_SHIMS, "exec sleep infinity"), (OPENER_SHIMS, "exit 0")] {
        for name in names {
            let f = dir.join(name);
            std::fs::write(&f, format!("#!/bin/sh\n# GNOME-ComfyUI: browser launch suppressed\n{body}\n"))?;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(dir)
}

pub struct Backend {
    pub proc: gio::Subprocess,
}

impl Backend {
    /// Kill the whole process group (the script plus the python it spawned).
    pub fn stop(&self) {
        if let Some(pid) = self.proc.identifier() {
            let _ = std::process::Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).status();
        }
        self.proc.send_signal(15);
    }
}

/// Spawn the user's launch script, streaming merged stdout/stderr lines to `on_line`
/// on the GTK main loop.
///
/// Runs as `sh -c 'PATH=<shim>:$PATH exec setsid "$0" "$@"' script args…` so that
/// the script gets its own process group (killable as a unit) and, if
/// `suppress_browser`, any browser it tries to open is a no-op. Inside Flatpak
/// the same command is run on the host via `flatpak-spawn --host`.
pub fn spawn_script(
    script: &std::path::Path,
    args: &str,
    suppress_browser: bool,
    on_line: impl Fn(&str) + 'static,
) -> Result<Backend, glib::Error> {
    let io_err = |e: std::io::Error| glib::Error::new(gio::IOErrorEnum::Failed, &e.to_string());
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    if in_flatpak() {
        argv.extend(["flatpak-spawn".into(), "--host".into(), "--watch-bus".into()]);
    }
    let prefix = if suppress_browser {
        let d = browser_shim_dir().map_err(io_err)?;
        format!("PATH='{}':\"$PATH\" ", d.display())
    } else {
        String::new()
    };
    let workdir = script.parent().unwrap_or(std::path::Path::new("/"));
    argv.extend([
        "sh".into(),
        "-c".into(),
        format!("cd \"$1\" && shift && {prefix}exec setsid \"$0\" \"$@\"").into(),
        script.into(),
        workdir.into(),
    ]);
    if !args.trim().is_empty() {
        argv.extend(glib::shell_parse_argv(args)?.into_iter().map(Into::into));
    }
    let argv_ref: Vec<&std::ffi::OsStr> = argv.iter().map(|s| s.as_os_str()).collect();

    let proc = gio::Subprocess::newv(
        &argv_ref,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE | gio::SubprocessFlags::STDIN_INHERIT,
    )?;
    let stdout = gio::DataInputStream::new(&proc.stdout_pipe().expect("stdout piped"));
    glib::spawn_future_local(async move {
        loop {
            match stdout.read_line_utf8_future(glib::Priority::DEFAULT).await {
                Ok(Some(line)) => on_line(&line),
                Ok(None) => break,
                Err(e) => { on_line(&format!("[log read error] {e}")); break; }
            }
        }
        on_line("[backend process exited]");
    });
    Ok(Backend { proc })
}
