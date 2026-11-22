//! Chrome DevTools Protocol over `--remote-debugging-pipe`.
//!
//! The browser reads commands from fd 3 and writes replies to fd 4, each
//! message NUL-terminated. No TCP port is opened, so no other local process or
//! web page can attach to the browser. Only this process holds the pipe.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;

#[cfg(unix)]
use std::{os::fd::AsRawFd, process::Child};
#[cfg(windows)]
#[allow(unsafe_code)]
#[path = "cdp_windows.rs"]
mod windows;
#[cfg(windows)]
use windows::Child;

/// Largest single protocol message accepted from the browser.
const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const MAX_PENDING: usize = 64;
const PROFILE_PREFIX: &str = "notdot-browser-";

/// Remove disposable profiles left by helpers that were killed before they
/// could clean up (Codex may stop an MCP server without letting it exit).
/// Only `notdot-browser-<pid>-…` directories whose process is gone are removed.
pub(crate) fn sweep_stale_profiles(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten().take(4096) {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(PROFILE_PREFIX)) else {
            continue;
        };
        let Some(pid) = rest
            .split_once('-')
            .and_then(|(pid, _)| pid.parse::<i32>().ok())
        else {
            continue;
        };
        let directory = entry
            .file_type()
            .is_ok_and(|t| t.is_dir() && !t.is_symlink());
        if directory && pid > 0 && pid as u32 != std::process::id() && !process_alive(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_alive(pid: i32) -> bool {
    windows::process_alive(pid as u32)
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub(crate) struct Cdp {
    child: Mutex<Child>,
    writer: Mutex<std::io::PipeWriter>,
    pending: Pending,
    sequence: AtomicU64,
    // Dropped after the child is reaped, so the profile is removed last.
    _profile: tempfile::TempDir,
}

impl Cdp {
    /// Launch `executable` with a new disposable profile directory.
    pub(crate) fn launch(executable: &Path, headless: bool) -> Result<Self, String> {
        sweep_stale_profiles(&std::env::temp_dir());
        let profile = tempfile::Builder::new()
            .prefix(&format!("{PROFILE_PREFIX}{}-", std::process::id()))
            .tempdir()
            .map_err(|e| format!("Create private browser profile: {e}"))?;
        let (to_browser_read, to_browser_write) =
            std::io::pipe().map_err(|e| format!("Create browser pipe: {e}"))?;
        let (from_browser_read, from_browser_write) =
            std::io::pipe().map_err(|e| format!("Create browser pipe: {e}"))?;
        let mut command = Command::new(executable);
        command
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .args([
                "--remote-debugging-pipe",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-sync",
                "--disable-extensions",
                "--disable-default-apps",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-client-side-phishing-detection",
                "--disable-features=Translate,MediaRouter,OptimizationHints,AutofillServerCommunication",
                // Keep the disposable profile out of the OS keychain entirely:
                // no keychain item is read or created, so no prompt can appear.
                "--password-store=basic",
                "--use-mock-keychain",
                "--deny-permission-prompts",
                "--new-window",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if headless {
            command.arg("--headless=new");
        }
        #[cfg(unix)]
        let mut child = {
            place_pipe_fds(
                &mut command,
                to_browser_read.as_raw_fd(),
                from_browser_write.as_raw_fd(),
            );
            command
                .spawn()
                .map_err(|e| format!("Start the private browser: {e}"))?
        };
        #[cfg(windows)]
        let mut child = windows::spawn(&command, &to_browser_read, &from_browser_write)
            .map_err(|e| format!("Start the private browser: {e}"))?;
        // The child holds its own copies; closing ours makes EOF reliable.
        drop(to_browser_read);
        drop(from_browser_write);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let replies = pending.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("notdot-cdp-reader".into())
            .spawn(move || read_loop(from_browser_read, replies))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
        Ok(Self {
            child: Mutex::new(child),
            writer: Mutex::new(to_browser_write),
            pending,
            sequence: AtomicU64::new(1),
            _profile: profile,
        })
    }

    /// Send one command, optionally to an attached target session, and wait
    /// at most `budget` for its reply.
    pub(crate) async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        budget: Duration,
    ) -> Result<Value, String> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| "Browser state lost")?;
            if pending.len() >= MAX_PENDING {
                return Err("Too many browser requests in flight".into());
            }
            pending.insert(id, tx);
        }
        let mut message = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let mut bytes = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        bytes.push(0);
        let written = self
            .writer
            .lock()
            .map_err(|_| "Browser state lost".to_string())
            .and_then(|mut w| w.write_all(&bytes).map_err(|e| e.to_string()));
        if let Err(e) = written {
            self.forget(id);
            return Err(format!("The private browser closed ({e})"));
        }
        let outcome = tokio::time::timeout(budget, rx).await;
        self.forget(id);
        match outcome {
            Err(_) => Err(format!("The browser did not answer in time ({method})")),
            Ok(Err(_)) => Err("The private browser closed".into()),
            Ok(Ok(result)) => result,
        }
    }

    fn forget(&self, id: u64) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.child.lock().ok().map(|c| c.id())
    }

    pub(crate) fn alive(&self) -> bool {
        self.child
            .lock()
            .map(|mut c| matches!(c.try_wait(), Ok(None)))
            .unwrap_or(false)
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn read_loop(reader: std::io::PipeReader, pending: Pending) {
    let mut reader = BufReader::new(reader);
    let mut frame = Vec::new();
    loop {
        frame.clear();
        let read = Read::by_ref(&mut reader)
            .take((MAX_MESSAGE + 1) as u64)
            .read_until(0, &mut frame);
        if !matches!(read, Ok(n) if n > 0) || frame.len() > MAX_MESSAGE {
            break;
        }
        if frame.last() == Some(&0) {
            frame.pop();
        }
        let Ok(message) = serde_json::from_slice::<Value>(&frame) else {
            continue;
        };
        // Events are not needed: every operation polls its own result.
        let Some(id) = message["id"].as_u64() else {
            continue;
        };
        let reply = pending.lock().ok().and_then(|mut p| p.remove(&id));
        if let Some(reply) = reply {
            let result = match message.get("error") {
                Some(error) => Err(error["message"]
                    .as_str()
                    .unwrap_or("Browser command failed")
                    .chars()
                    .take(300)
                    .collect()),
                None => Ok(message["result"].clone()),
            };
            let _ = reply.send(result);
        }
    }
    if let Ok(mut pending) = pending.lock() {
        pending.clear();
    }
}

#[cfg(unix)]
/// Make the child's fd 3 our command pipe and fd 4 our reply pipe.
#[allow(unsafe_code)]
fn place_pipe_fds(command: &mut Command, read: i32, write: i32) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child before exec and calls only
    // async-signal-safe functions (fcntl, dup2). The source fds are first
    // duplicated above 10 so placing one cannot overwrite the other when the
    // parent's pipes happen to be fds 3 or 4. dup2 clears close-on-exec on
    // fds 3 and 4 only; the high duplicates close at exec.
    unsafe {
        command.pre_exec(move || {
            let high_read = libc::fcntl(read, libc::F_DUPFD_CLOEXEC, 10);
            let high_write = libc::fcntl(write, libc::F_DUPFD_CLOEXEC, 10);
            if high_read < 0
                || high_write < 0
                || libc::dup2(high_read, 3) < 0
                || libc::dup2(high_write, 4) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_profiles_of_dead_helpers_are_swept() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir
            .path()
            .join(format!("notdot-browser-{}-a", std::process::id()));
        let dead = dir.path().join("notdot-browser-999999-b");
        let unrelated = dir.path().join("notdot-browser-notapid");
        let other = dir.path().join("something-else-1-c");
        for d in [&own, &dead, &unrelated, &other] {
            std::fs::create_dir(d).unwrap();
        }
        let file = dir.path().join("notdot-browser-999998-file");
        std::fs::write(&file, "x").unwrap();
        super::sweep_stale_profiles(dir.path());
        assert!(own.exists() && unrelated.exists() && other.exists() && file.exists());
        assert!(!dead.exists());
    }
}
