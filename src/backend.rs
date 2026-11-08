// SPDX-License-Identifier: GPL-3.0-only
//! Backend process launch + connectivity probe.
//!
//! Deliberately std + gio only: no tokio/reqwest.

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

pub fn in_flatpak() -> bool {
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

/// Flatpak teardown needs the *host* process group: the pids visible here are in
/// the sandbox namespace, and `--watch-bus` only kills flatpak-spawn's direct
/// child (setsid detaches the real backend from it). This host helper becomes the
/// session leader, records its pid, then execs the target in place, so the
/// recorded pid stays the group leader.
fn flatpak_host_files() -> std::io::Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    let dir = glib::user_cache_dir().join("comfy-gnome");
    std::fs::create_dir_all(&dir)?;
    let helper = dir.join("host-exec.sh");
    std::fs::write(&helper, "#!/bin/sh\n# Comfy-GNOME: backend host session (pid file arg, then command)\necho $$ > \"$1\" || exit 1\nshift\nexec \"$@\"\n")?;
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))?;
    Ok((helper, dir.join("backend.pid")))
}

fn shell_quote(p: &std::path::Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

pub struct Backend {
    pub proc: gio::Subprocess,
    /// Flatpak only: host-side pid file written by the session-leader helper.
    /// ponytail: graceful stop kills the host group; a hard crash before stop
    /// still leaves it behind (same as before).
    pidfile: Option<PathBuf>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Backend {
    /// Kill the whole process group (the script plus the python it spawned).
    pub fn stop(&self) {
        if let Some(pf) = &self.pidfile {
            // Flatpak: the group lives in the host pid namespace; ask the portal
            // to TERM it, wait up to 3 s, then KILL. Dash's `kill` rejects `--`,
            // hence no `--` in the host snippet.
            if let Ok(pid) = std::fs::read_to_string(pf) {
                let pid = pid.trim().to_string();
                if !pid.is_empty() {
                    let _ = std::process::Command::new("flatpak-spawn")
                        .args([
                            "--host", "sh", "-c",
                            "p=$1; kill -TERM -\"$p\" 2>/dev/null; i=0; \
                             while kill -0 -\"$p\" 2>/dev/null && [ $i -lt 30 ]; do sleep 0.1; i=$((i+1)); done; \
                             kill -KILL -\"$p\" 2>/dev/null",
                            "sh", pid.as_str(),
                        ])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
            let _ = std::fs::remove_file(pf);
            self.proc.send_signal(15);
            return;
        }
        if let Some(pid) = self.proc.identifier() {
            let _ = std::process::Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).stderr(std::process::Stdio::null()).status();
        }
        self.proc.send_signal(15);
    }

    /// `stop`, then make sure the group is really gone: a ComfyUI that ignores
    /// SIGTERM (or a script whose trap does not kill its python) used to outlive
    /// the app, and the next launch then connected to that leftover backend —
    /// which looks like "my history/queue came back from a dead session".
    pub fn stop_hard(&self) {
        self.stop();
        // Flatpak: stop() already TERMed/waited/KILLed the host group.
        if self.pidfile.is_some() {
            return;
        }
        let Some(pid) = self.proc.identifier().map(|p| p.to_string()) else { return };
        // -0 tests the group, not just our direct child: scripts that did not
        // `exec` leave python behind in the same group while exiting themselves.
        for _ in 0..30 {
            if !group_alive(&pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!("backend: group {pid} survived SIGTERM for 3 s, sending SIGKILL");
        let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).stderr(std::process::Stdio::null()).status();
        self.proc.force_exit();
    }

    pub fn is_running(&self) -> bool {
        if let Some(pf) = &self.pidfile {
            let Ok(pid) = std::fs::read_to_string(pf) else { return false };
            let pid = pid.trim().to_string();
            return !pid.is_empty()
                && std::process::Command::new("flatpak-spawn")
                    .args(["--host", "sh", "-c", "kill -0 -\"$1\"", "sh", pid.as_str()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
        }
        self.proc.identifier().is_some_and(|p| group_alive(&p))
    }
}

fn group_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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
    let host_files = if in_flatpak() { Some(flatpak_host_files().map_err(io_err)?) } else { None };
    let launch = match &host_files {
        Some((helper, pidfile)) => format!("exec setsid {} {} ", shell_quote(helper), shell_quote(pidfile)),
        None => "exec setsid ".to_string(),
    };
    // $0 = script (or "python"), $1 = working dir, rest = args.
    let (body, arg0, workdir): (String, std::ffi::OsString, PathBuf) = match (&cfg.script, &cfg.comfy_dir) {
        (Some(script), _) => (
            format!("cd \"$1\" && shift && {prefix}{launch}\"$0\" \"$@\""),
            script.into(),
            script.parent().unwrap_or(std::path::Path::new("/")).into(),
        ),
        (None, Some(dir)) => (
            format!(
                "cd \"$1\" && shift && \
                 for v in venv .venv; do [ -f \"$v/bin/activate\" ] && . \"$v/bin/activate\" && break; done; \
                 {prefix}{launch}\"$(command -v python || command -v python3)\" main.py \"$@\""
            ),
            "python".into(),
            dir.clone(),
        ),
        (None, None) => return Err(glib::Error::new(gio::IOErrorEnum::InvalidArgument, "nothing to launch")),
    };
    argv.extend(["sh".into(), "-c".into(), body.into(), arg0, workdir.into()]);
    // The app owns the port: pass it in script mode too. Scripts commonly forward
    // "$@" to main.py; before user args so theirs win (argparse: last wins).
    argv.extend(["--port".into(), cfg.port.to_string().into()]);
    if !cfg.args.trim().is_empty() {
        argv.extend(glib::shell_parse_argv(cfg.args.as_str())?.into_iter().map(Into::into));
    }
    let argv_ref: Vec<&std::ffi::OsStr> = argv.iter().map(|s| s.as_os_str()).collect();

    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE | gio::SubprocessFlags::STDIN_INHERIT,
    );
    // If we die without running stop() (crash, X connection loss), the kernel sends the
    // child SIGTERM. Native: that child is the session leader we kill by group. Flatpak:
    // --watch-bus reaches flatpak-spawn's direct child; stop() additionally kills the
    // recorded host session (see flatpak_host_files).
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
    Ok(Backend { proc, pidfile: host_files.map(|(_, p)| p) })
}
