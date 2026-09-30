//! Shared plumbing for the local CLI bridges (`claude`, `codex`): finding the
//! binary (a GUI app's PATH is minimal, so common install dirs are searched
//! too), and running it with the prompt on stdin, streaming stdout lines,
//! killing the child when the cancel flag is set.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::{AiError, Result};
use crate::provider::CancelFlag;

/// Where to look for a CLI. `Default` = the process PATH plus the usual
/// install locations under `$HOME`.
#[derive(Debug, Clone, Default)]
pub struct CliLocator {
    /// Overrides `$PATH` (tests point this at a temp dir).
    pub path_var: Option<OsString>,
    /// Overrides `$HOME` for the per-user install dirs.
    pub home: Option<PathBuf>,
    /// When false, only `path_var`/`$PATH` is searched (tests).
    pub skip_common_dirs: bool,
}

impl CliLocator {
    /// A locator that searches only `dir` (tests).
    pub fn only(dir: impl Into<PathBuf>) -> Self {
        CliLocator {
            path_var: Some(dir.into().into_os_string()),
            home: None,
            skip_common_dirs: true,
        }
    }

    /// Every directory searched, in order.
    pub fn search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = match &self.path_var {
            Some(p) => std::env::split_paths(p).collect(),
            None => std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
        };
        if !self.skip_common_dirs {
            let home = self
                .home
                .clone()
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
            if let Some(h) = home {
                for d in [
                    ".claude/local",
                    ".local/bin",
                    ".npm-global/bin",
                    ".bun/bin",
                    ".volta/bin",
                    ".cargo/bin",
                ] {
                    dirs.push(h.join(d));
                }
            }
            for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
                dirs.push(PathBuf::from(d));
            }
        }
        let mut seen = std::collections::HashSet::new();
        dirs.retain(|d| seen.insert(d.clone()));
        dirs
    }

    pub fn find(&self, name: &str) -> Option<PathBuf> {
        self.search_dirs()
            .into_iter()
            .map(|d| d.join(name))
            .find(|p| is_executable(p))
    }

    /// `PATH` for the child: the binary's own dir first (node shims need
    /// `node` next to them), then everything we searched.
    pub fn child_path(&self, bin: &Path) -> OsString {
        let mut dirs = vec![];
        if let Some(d) = bin.parent() {
            dirs.push(d.to_path_buf());
        }
        dirs.extend(self.search_dirs());
        std::env::join_paths(dirs).unwrap_or_default()
    }
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A fresh empty working directory so the CLI sees no project files, hooks
/// or MCP configs. Removed on drop.
pub(crate) struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new() -> Result<Self> {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "blygger-ai-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&p).map_err(|e| AiError::Storage(e.to_string()))?;
        Ok(ScratchDir(p))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Result of a CLI run once stdout closed.
pub(crate) struct Exit {
    pub code: Option<i32>,
    pub stderr: String,
}

/// Spawn `cmd`, write `stdin`, and call `on_line` per stdout line.
/// `on_line` returning `Err` kills the child and propagates.
pub(crate) fn run_streaming(
    name: &str,
    mut cmd: Command,
    stdin: &str,
    cancel: &CancelFlag,
    mut on_line: impl FnMut(&str) -> Result<()>,
) -> Result<Exit> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child: Child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AiError::CliNotFound(name.to_string()),
        _ => AiError::CliFailed {
            name: name.to_string(),
            code: None,
            message: e.to_string(),
        },
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let mut stdin_pipe = child.stdin.take().expect("piped stdin");

    let input = stdin.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin_pipe.write_all(input.as_bytes());
        // dropping closes stdin → EOF for the CLI
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = (&mut stderr_pipe).take(1024 * 1024).read_to_string(&mut s);
        s
    });

    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let child = child.clone();
        let done = done.clone();
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if cancel.is_cancelled() {
                    if let Ok(mut c) = child.lock() {
                        let _ = c.kill();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
    };

    let mut failure: Option<AiError> = None;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Err(e) = on_line(line.trim_end_matches(['\r', '\n'])) {
                    failure = Some(e);
                    if let Ok(mut c) = child.lock() {
                        let _ = c.kill();
                    }
                    break;
                }
            }
            Err(e) => {
                failure = Some(AiError::CliFailed {
                    name: name.to_string(),
                    code: None,
                    message: e.to_string(),
                });
                break;
            }
        }
    }
    let status = child.lock().ok().and_then(|mut c| c.wait().ok());
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let _ = writer.join();
    let stderr = stderr_reader.join().unwrap_or_default();

    if cancel.is_cancelled() {
        return Err(AiError::Cancelled);
    }
    if let Some(e) = failure {
        return Err(e);
    }
    Ok(Exit {
        code: status.and_then(|s| s.code()),
        stderr,
    })
}

/// Last few lines of stderr, for error messages.
pub(crate) fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.trim().lines().collect();
    let start = lines.len().saturating_sub(5);
    lines[start..].join("\n")
}
