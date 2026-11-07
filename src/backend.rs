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
    /// User launch script. Takes priority over `comfy_dir`.
    pub script: Option<PathBuf>,
    /// ComfyUI checkout: launched directly (`[venv] python main.py --port N args`).
    /// Both `None` = don't launch anything, connect to an already-running backend.
    pub comfy_dir: Option<PathBuf>,
    /// Extra arguments, shell-quoted (parsed with g_shell_parse_argv).
    pub args: String,
    pub host: String,
    pub port: u16,
    /// Put no-op browser shims first on the script's PATH.
    pub suppress_browser: bool,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self { script: None, comfy_dir: None, args: String::new(), host: DEFAULT_HOST.into(), port: DEFAULT_PORT, suppress_browser: true }
    }
}

impl LaunchConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    fn file() -> PathBuf {
        glib::user_config_dir().join("comfy-gnome").join("launch.ini")
    }

    /// Last splash settings (defaults + auto-detected folder if none saved).
    pub fn load() -> Self {
        let mut c = Self { comfy_dir: find_comfy_dir(), ..Self::default() };
        let kf = glib::KeyFile::new();
        if kf.load_from_file(Self::file(), glib::KeyFileFlags::NONE).is_err() {
            return c;
        }
        let s = |k: &str| kf.string("launch", k).ok().map(|v| v.to_string()).filter(|v| !v.is_empty());
        c.script = s("script").map(PathBuf::from).filter(|p| p.exists());
        if let Some(d) = s("comfy_dir").map(PathBuf::from).filter(|p| is_comfy_dir(p)) {
            c.comfy_dir = Some(d);
        }
        c.args = s("args").unwrap_or_default();
        c.host = s("host").unwrap_or(c.host);
        c.port = kf.integer("launch", "port").ok().and_then(|p| u16::try_from(p).ok()).unwrap_or(c.port);
        c.suppress_browser = kf.boolean("launch", "suppress_browser").unwrap_or(true);
        c
    }

    pub fn save(&self) {
        let kf = glib::KeyFile::new();
        let p = |o: &Option<PathBuf>| o.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        kf.set_string("launch", "script", &p(&self.script));
        kf.set_string("launch", "comfy_dir", &p(&self.comfy_dir));
        kf.set_string("launch", "args", &self.args);
        kf.set_string("launch", "host", &self.host);
        kf.set_integer("launch", "port", self.port.into());
        kf.set_boolean("launch", "suppress_browser", self.suppress_browser);
        let f = Self::file();
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        if let Err(e) = kf.save_to_file(&f) {
            eprintln!("saving {}: {e}", f.display());
        }
    }
}

/// True if `dir` looks like a ComfyUI checkout.
pub fn is_comfy_dir(dir: &std::path::Path) -> bool {
    dir.join("main.py").is_file() && dir.join("comfy").is_dir()
}

/// First existing ComfyUI checkout in the usual places.
pub fn find_comfy_dir() -> Option<PathBuf> {
    let home = glib::home_dir();
    ["ComfyUI", "Documents/ComfyUI", "comfyui", "Documents/comfyui", "src/ComfyUI"]
        .iter()
        .map(|p| home.join(p))
        .find(|p| is_comfy_dir(p))
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
    let dir = if in_flatpak() { base.join("app").join(crate::APP_ID) } else { base.join("comfy-gnome") }
        .join("browser-shim");
    std::fs::create_dir_all(&dir)?;
    for (names, body) in [(BROWSER_SHIMS, "exec sleep infinity"), (OPENER_SHIMS, "exit 0")] {
        for name in names {
            let f = dir.join(name);
            std::fs::write(&f, format!("#!/bin/sh\n# Comfy-GNOME: browser launch suppressed\n{body}\n"))?;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(dir)
}

pub struct Backend {
    pub proc: gio::Subprocess,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Backend {
    /// Kill the whole process group (the script plus the python it spawned).
    pub fn stop(&self) {
        eprintln!("DEBUG stop pid={:?}", self.proc.identifier());
        if let Some(pid) = self.proc.identifier() {
            let _ = std::process::Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).status();
        }
        self.proc.send_signal(15);
    }
}

/// Launch ComfyUI per `cfg` (script or folder; caller checks one is set), streaming
/// merged stdout/stderr lines to `on_line` on the GTK main loop.
///
/// Runs as `sh -c '<cd + optional venv> PATH=<shim>:$PATH exec setsid <cmd> "$@"' …`
/// so the process gets its own group (killable as a unit) and, if `suppress_browser`,
/// any browser a script tries to open is a no-op. Inside Flatpak the same command
/// runs on the host via `flatpak-spawn --host`, so host python/venv/CUDA are used.
pub fn spawn(cfg: &LaunchConfig, on_line: impl Fn(&str) + 'static) -> Result<Backend, glib::Error> {
    let suppress_browser = cfg.suppress_browser;
    let io_err = |e: std::io::Error| glib::Error::new(gio::IOErrorEnum::Failed, &e.to_string());
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    if in_flatpak() {
        argv.extend(["flatpak-spawn".into(), "--host".into(), "--watch-bus".into()]);
    }
    // Unbuffered python, otherwise the log panel only updates every 4–8 KiB.
    let mut prefix = String::from("PYTHONUNBUFFERED=1 ");
    if suppress_browser {
        let d = browser_shim_dir().map_err(io_err)?;
        prefix += &format!("PATH='{}':\"$PATH\" ", d.display());
    }
    // $0 = script (or "python"), $1 = working dir, rest = args.
    let (body, arg0, workdir): (String, std::ffi::OsString, PathBuf) = match (&cfg.script, &cfg.comfy_dir) {
        (Some(script), _) => (
            format!("cd \"$1\" && shift && {prefix}exec setsid \"$0\" \"$@\""),
            script.into(),
            script.parent().unwrap_or(std::path::Path::new("/")).into(),
        ),
        (None, Some(dir)) => (
            format!(
                "cd \"$1\" && shift && \
                 for v in venv .venv; do [ -f \"$v/bin/activate\" ] && . \"$v/bin/activate\" && break; done; \
                 {prefix}exec setsid \"$(command -v python || command -v python3)\" main.py \"$@\""
            ),
            "python".into(),
            dir.clone(),
        ),
        (None, None) => return Err(glib::Error::new(gio::IOErrorEnum::InvalidArgument, "nothing to launch")),
    };
    argv.extend(["sh".into(), "-c".into(), body.into(), arg0, workdir.into()]);
    if cfg.script.is_none() {
        // Before user args so theirs win (argparse: last occurrence wins).
        argv.extend(["--port".into(), cfg.port.to_string().into()]);
    }
    if !cfg.args.trim().is_empty() {
        argv.extend(glib::shell_parse_argv(cfg.args.as_str())?.into_iter().map(Into::into));
    }
    let argv_ref: Vec<&std::ffi::OsStr> = argv.iter().map(|s| s.as_os_str()).collect();

    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE | gio::SubprocessFlags::STDIN_INHERIT,
    );
    // If we die without running stop() (crash, X connection loss), the kernel sends the
    // child SIGTERM. The child is python itself in folder mode (exec chain), or the
    // script (whose own cleanup trap then runs). Flatpak: --watch-bus covers this.
    // SAFETY: prctl is async-signal-safe.
    launcher.set_child_setup(|| unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    });
    let proc = launcher.spawn(&argv_ref)?;
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
