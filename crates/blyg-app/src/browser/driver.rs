//! Keeps the compiled block lists current (macOS, not in tests): the cached
//! lists first, else a compile of what's on disk (the bundled list before the
//! first download), then a download when the lists are a week old, and a
//! recompile when they changed. Conversion runs on a background thread;
//! WebKit compiles on its own queue. Opening a page never waits on any of
//! this for longer than `RULES_WAIT`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use adblock::content_blocking::CbRule;
use gpui_kit::AsyncApp;
use objc2_web_kit::WKContentRuleListStore;

use super::blocklist::{self, ConvertReport, Manifest};
use super::rules::{self, RuleList};
use super::rules_handle::Handle;

fn timing() -> bool {
    std::env::var_os("BLYGGER_TIMING").is_some()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A list to compile: a file the converter wrote, or half of one WebKit
/// refused.
enum Job {
    File(PathBuf),
    Rules(Vec<CbRule>),
}

/// Run `blygger +convert-blocklists` (this same executable) so the
/// conversion's memory goes back to the system when it exits; in-process if
/// the child can't be started.
fn convert_in_child(
    data_dir: &Path,
    out: &Path,
    unless_key: Option<&str>,
) -> Result<ConvertReport, String> {
    let _ = std::fs::remove_dir_all(out);
    let child = std::env::current_exe().ok().and_then(|exe| {
        let mut cmd = Command::new(exe);
        cmd.arg("+convert-blocklists")
            .arg(out)
            .env("BLYGGER_DATA_DIR", data_dir)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(k) = unless_key {
            cmd.arg(format!("--unless-key={k}"));
        }
        cmd.output().ok()
    });
    match child {
        Some(o) if o.status.success() => serde_json::from_slice(&o.stdout)
            .map_err(|e| format!("the converter's report didn't parse: {e}")),
        Some(o) => Err(format!("the converter failed ({})", o.status)),
        None => blocklist::convert_to_dir(data_dir, out, unless_key).map_err(|e| e.to_string()),
    }
}

/// WebKit compiles rule lists in this process and its DFAs take a few
/// hundred MB for the full lists; hand the freed pages back to the system
/// afterwards instead of keeping them in malloc's free lists.
fn release_memory() {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    // SAFETY: libmalloc's public API; a null zone means every zone, goal 0
    // means as much as possible.
    let freed = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    if timing() {
        println!("browser-blocklist released_mb={}", freed / (1024 * 1024));
    }
}

/// Rules below this many aren't split further when WebKit refuses a list.
const MIN_BISECT: usize = 256;

/// How often a running app looks at whether the lists are due.
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Driver {
    pub data_dir: PathBuf,
    pub handle: Handle,
    /// Downloads are allowed (never in fake mode or with
    /// `BLYGGER_NO_BLOCKLIST_DOWNLOAD`).
    pub download: bool,
    /// Called on the main thread after the lists changed.
    pub on_change: Box<dyn Fn(&mut AsyncApp)>,
}

impl Driver {
    pub async fn run(self, cx: &mut AsyncApp) {
        let Some(store) = rules::open_store(&blocklist::compiled_dir(&self.data_dir)) else {
            eprintln!("blygger: content blocking is unavailable (no rule-list store)");
            return;
        };
        let mut manifest = Manifest::load(&self.data_dir);
        let t0 = Instant::now();
        let cached = self.load_cached(&store, &manifest).await;
        if cached {
            if timing() {
                println!(
                    "browser-blocklist cached lists={} rules={} lookup_ms={}",
                    manifest.ids.len(),
                    manifest.rules,
                    t0.elapsed().as_millis()
                );
            }
            (self.on_change)(cx);
        } else {
            self.rebuild(&store, &mut manifest, cx).await;
        }
        loop {
            if self.download && blocklist::due(manifest.fetched_at, SystemTime::now()) {
                let dir = self.data_dir.clone();
                let t = Instant::now();
                let got = cx
                    .background_executor()
                    .spawn(async move { blocklist::download_all(&dir) })
                    .await;
                if timing() {
                    println!(
                        "browser-blocklist downloaded={got}/{} ms={}",
                        blocklist::SOURCES.len(),
                        t.elapsed().as_millis()
                    );
                }
                if got > 0 {
                    manifest.fetched_at = Some(now_secs());
                    let _ = manifest.save(&self.data_dir);
                    self.rebuild(&store, &mut manifest, cx).await;
                }
            }
            cx.background_executor().timer(RECHECK).await;
        }
    }

    /// Attach the lists the manifest names, if the store has all of them.
    async fn load_cached(&self, store: &WKContentRuleListStore, m: &Manifest) -> bool {
        if m.ids.is_empty() {
            return false;
        }
        let mut lists = Vec::new();
        for id in &m.ids {
            match rules::lookup(store, id).await {
                Some(l) => lists.push(l),
                None => return false,
            }
        }
        self.install(lists, m.rules);
        true
    }

    fn install(&self, lists: Vec<RuleList>, rules: usize) {
        let mut h = self.handle.borrow_mut();
        h.lists = lists;
        h.rules = rules;
        h.generation += 1;
    }

    /// Convert what's on disk and compile it, unless that's what's compiled.
    async fn rebuild(&self, store: &WKContentRuleListStore, m: &mut Manifest, cx: &mut AsyncApp) {
        let dir = self.data_dir.clone();
        let have = m.key.clone().filter(|_| self.handle.borrow().ready());
        let out = blocklist::browser_dir(&dir).join("convert");
        let t0 = Instant::now();
        let report = {
            let out = out.clone();
            cx.background_executor()
                .spawn(async move { convert_in_child(&dir, &out, have.as_deref()) })
                .await
        };
        let report = match report {
            Ok(r) if r.unchanged => return,
            Ok(r) => r,
            Err(e) => {
                eprintln!("blygger: couldn't convert the block lists ({e})");
                return;
            }
        };
        let convert_ms = t0.elapsed().as_millis();
        let t1 = Instant::now();
        let key = report.key.clone();
        let mut queue: VecDeque<Job> = (0..report.lists)
            .map(|i| Job::File(blocklist::list_file(&out, i)))
            .collect();
        let mut compiled = Vec::new();
        let mut ids = Vec::new();
        let mut skipped = 0;
        let mut n = 0;
        while let Some(job) = queue.pop_front() {
            let id = blocklist::identifier(&key, n);
            n += 1;
            let json = match &job {
                Job::File(path) => {
                    let path = path.clone();
                    cx.background_executor()
                        .spawn(async move { std::fs::read_to_string(path).unwrap_or_default() })
                        .await
                }
                Job::Rules(r) => blocklist::encode(r),
            };
            match rules::compile(store, &id, &json).await {
                Ok(list) => {
                    compiled.push(list);
                    ids.push(id);
                }
                Err(e) => {
                    // One rule WebKit won't take sinks the whole list: halve
                    // it until the bad part is small, and drop that part.
                    let chunk: Vec<CbRule> = match job {
                        Job::Rules(r) => r,
                        Job::File(_) => serde_json::from_str(&json).unwrap_or_default(),
                    };
                    let len = chunk.len();
                    if len > MIN_BISECT {
                        eprintln!("blygger: a block list didn't compile ({e}); splitting it");
                        let (a, b) = chunk.split_at(len / 2);
                        queue.push_front(Job::Rules(b.to_vec()));
                        queue.push_front(Job::Rules(a.to_vec()));
                    } else {
                        eprintln!("blygger: skipped {len} block rules WebKit refused ({e})");
                        skipped += len;
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&out);
        release_memory();
        if compiled.is_empty() {
            return;
        }
        let rules_in = report.rules.saturating_sub(skipped);
        if timing() {
            println!(
                "browser-blocklist compiled lists={} rules={rules_in} filters_used={} dropped={} convert_ms={convert_ms} (in the child: {}) compile_ms={}",
                compiled.len(),
                report.filters_used,
                report.dropped,
                report.convert_ms,
                t1.elapsed().as_millis()
            );
        }
        let old = std::mem::replace(&mut m.ids, ids);
        m.key = Some(key);
        m.rules = rules_in;
        let _ = m.save(&self.data_dir);
        self.install(compiled, rules_in);
        (self.on_change)(cx);
        // Sweep lists nothing names any more (older keys, failed halves).
        let keep: std::collections::HashSet<&String> = m.ids.iter().collect();
        for id in old.iter().filter(|i| !keep.contains(i)) {
            rules::remove(store, id);
        }
        for id in rules::identifiers(store).await {
            if id.starts_with("blyg-") && !keep.contains(&id) {
                rules::remove(store, &id);
            }
        }
    }
}
