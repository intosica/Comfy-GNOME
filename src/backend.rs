// SPDX-License-Identifier: GPL-3.0-only
//! Backend process launch + connectivity probe.
//!
//! Deliberately std + gio only: no tokio/reqwest.

use adw::{gio, glib};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::rc::Rc;
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

/// Where the backend's merged stdout/stderr goes.
///
/// A plain file, never a pipe: any reader that stops reading — invalid UTF-8 in one line, an app
/// restart, a dropped read future — closes the read end, and then every `print` in ComfyUI dies
/// with `[Errno 32] Broken pipe`. Inside a sampler that exception aborts the node and the user
/// loses the run, so the pipe itself has to go.
pub fn log_path() -> PathBuf {
    glib::user_cache_dir().join("comfy-gnome/backend.log")
}

/// Tail of [`log_path`]: reads only what is new since the last call, so a slow poll costs nothing.
struct Tail {
    path: PathBuf,
    file: Option<std::fs::File>,
    off: u64,
    /// Bytes after the last newline (a progress bar writes `\r` chunks, not lines).
    part: Vec<u8>,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self { path, file: None, off: 0, part: Vec::new() }
    }

    fn read(&mut self, mut emit: impl FnMut(&str)) {
        use std::io::{Read, Seek, SeekFrom};
        if self.file.is_none() {
            self.file = std::fs::File::open(&self.path).ok();
        }
        let Some(f) = self.file.as_mut() else { return };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.off {
            // Truncated under us (a fresh launch): start over instead of reading garbage.
            self.off = 0;
            self.part.clear();
        }
        if len == self.off || f.seek(SeekFrom::Start(self.off)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.take(len - self.off).read_to_end(&mut buf).is_err() {
            return;
        }
        self.off += buf.len() as u64;
        self.part.extend_from_slice(&buf);
        while let Some(i) = self.part.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.part.drain(..=i).collect();
            emit(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
        }
        // A progress bar without newlines must still show up; cap the pending tail.
        if self.part.len() > 4096 {
            emit(&String::from_utf8_lossy(&self.part));
            self.part.clear();
        }
    }
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

/// Flatpak: TERM (then KILL after 3 s) the host process group recorded in `pidfile`.
/// Dash's `kill` rejects `--`, hence no `--` in the host snippet.
fn kill_host_group(pidfile: &Path) {
    let Ok(pid) = std::fs::read_to_string(pidfile) else { return };
    let pid = pid.trim();
    if pid.is_empty() {
        return;
    }
    let _ = std::process::Command::new("flatpak-spawn")
        .args([
            "--host", "sh", "-c",
            "p=$1; kill -TERM -\"$p\" 2>/dev/null; i=0; \
             while kill -0 -\"$p\" 2>/dev/null && [ $i -lt 30 ]; do sleep 0.1; i=$((i+1)); done; \
             kill -KILL -\"$p\" 2>/dev/null",
            "sh", pid,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

impl Backend {
    /// Kill the whole process group (the script plus the python it spawned).
    pub fn stop(&self) {
        if let Some(pf) = &self.pidfile {
            // Flatpak: the group lives in the host pid namespace; ask the portal
            // to TERM it, wait up to 3 s, then KILL. Dash's `kill` rejects `--`,
            // hence no `--` in the host snippet.
            kill_host_group(pf);
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
    // A previous app instance that died without stop() (SIGTERM/crash) leaves its host group
    // running with a stdout pipe nobody reads: every print in ComfyUI then fails with
    // "[Errno 32] Broken pipe" (KSampler's progress bar). Reap it before starting a new one.
    if let Some((_, pidfile)) = &host_files {
        kill_host_group(pidfile);
    }
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
    // Merged output goes to a file: stdout and stderr are the same fd, so their order survives and
    // nothing in this process can close a pipe the backend is writing to (see [`log_path`]).
    let log = log_path();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&log, b"");
    let body = format!("{} >>{} 2>&1", body, shell_quote(&log));
    argv.extend(["sh".into(), "-c".into(), body.into(), arg0, workdir.into()]);
    // The app owns the port: pass it in script mode too. Scripts commonly forward
    // "$@" to main.py; before user args so theirs win (argparse: last wins).
    argv.extend(["--port".into(), cfg.port.to_string().into()]);
    if !cfg.args.trim().is_empty() {
        argv.extend(glib::shell_parse_argv(cfg.args.as_str())?.into_iter().map(Into::into));
    }
    let argv_ref: Vec<&std::ffi::OsStr> = argv.iter().map(|s| s.as_os_str()).collect();

    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::STDIN_INHERIT);
    // If we die without running stop() (crash, X connection loss), the kernel sends the
    // child SIGTERM. Native: that child is the session leader we kill by group. Flatpak:
    // --watch-bus reaches flatpak-spawn's direct child; stop() additionally kills the
    // recorded host session (see flatpak_host_files).
    // SAFETY: prctl is async-signal-safe.
    launcher.set_child_setup(|| unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    });
    let proc = launcher.spawn(&argv_ref)?;
    on_line(&format!("[backend] output → {}", log.display()));
    // Poll the file instead of reading a pipe: no reader of ours can ever close the backend's
    // output. The timer stops by itself once the process is gone (and drains what is left).
    let tail = Rc::new(std::cell::RefCell::new(Tail::new(log.clone())));
    let exited = Rc::new(std::cell::Cell::new(false));
    {
        let (p, e) = (proc.clone(), exited.clone());
        glib::spawn_future_local(async move {
            let _ = p.wait_check_future().await;
            e.set(true);
        });
    }
    let on_line = Rc::new(on_line);
    glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
        tail.borrow_mut().read(|l| on_line(l));
        if exited.get() {
            tail.borrow_mut().read(|l| on_line(l));
            on_line("[backend process exited]");
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    Ok(Backend { proc, pidfile: host_files.map(|(_, p)| p) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The log reader must survive what killed the old pipe reader: one non-UTF-8 byte, then more
    /// output. A pipe that dies here is exactly what turned into `[Errno 32] Broken pipe` inside a
    /// sampler, so this behaviour is load-bearing.
    #[test]
    fn tail_reads_new_bytes_only_and_survives_garbage() {
        let dir = std::env::temp_dir().join(format!("cg-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("backend.log");
        std::fs::write(&path, b"first\r\nsecond\n").unwrap();
        let mut t = Tail::new(path.clone());
        let mut got = Vec::new();
        t.read(|l| got.push(l.to_owned()));
        assert_eq!(got, ["first", "second"]);

        // Append a non-UTF-8 line and a partial progress line (no newline yet).
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"\xff\xfe bad bytes\nprog 1/3\r").unwrap();
        drop(f);
        let mut got = Vec::new();
        t.read(|l| got.push(l.to_owned()));
        assert_eq!(got.len(), 1, "only the completed line is emitted: {got:?}");
        assert!(got[0].contains("bad bytes"), "lossy decode keeps the text: {got:?}");

        // A fresh launch truncates the file: read from the top instead of skipping the new run.
        std::fs::write(&path, b"fresh\n").unwrap();
        let mut got = Vec::new();
        t.read(|l| got.push(l.to_owned()));
        assert_eq!(got, ["fresh"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
