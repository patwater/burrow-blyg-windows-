//! Build info for the About window (`src/about/`): the git commit, whether
//! the tree was dirty, and when it was built. Nothing machine-specific is
//! embedded (no paths, no hostname, no user name).
//!
//! - `BLYGGER_GIT_SHA`: `git rev-parse --short HEAD`, or `unknown` (no git,
//!   or a source tarball that isn't a checkout of this repo).
//! - `BLYGGER_GIT_DIRTY`: `1` if `git status --porcelain` listed anything.
//! - `BLYGGER_BUILD_EPOCH`: Unix seconds; `SOURCE_DATE_EPOCH` when set
//!   (reproducible builds), otherwise now.
//!
//! Re-runs only when the checked-out commit, the index, a ref or a source
//! file under `crates/` changes, so an unchanged tree doesn't recompile the
//! crate on every build.
//!
//! On Windows it also embeds the exe's icon (`windows_icon`).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    for p in watch_paths(&manifest) {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    let (sha, dirty) = commit(&manifest).unwrap_or_else(|| ("unknown".to_string(), false));
    // Sources anywhere in the workspace's crates: an edit makes the tree
    // dirty (and recompiles this crate or a dependency of it anyway).
    if let Some(crates) = manifest.parent().filter(|p| p.ends_with("crates")) {
        println!("cargo:rerun-if-changed={}", crates.display());
    }

    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

    println!("cargo:rustc-env=BLYGGER_GIT_SHA={sha}");
    println!(
        "cargo:rustc-env=BLYGGER_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );
    println!("cargo:rustc-env=BLYGGER_BUILD_EPOCH={epoch}");

    windows_icon(&manifest);
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Don't let `git status` refresh (rewrite) the index: that would
        // touch a watched file and re-run this script on the next build.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// (short SHA, dirty), when this crate is part of a git checkout.
fn commit(dir: &Path) -> Option<(String, bool)> {
    // A tarball unpacked inside some other repository must not report that
    // repository's commit: this file has to be tracked here.
    git(dir, &["ls-files", "--error-unmatch", "build.rs"])?;
    let sha = git(dir, &["rev-parse", "--short", "HEAD"]).filter(|s| !s.is_empty())?;
    let dirty = git(dir, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    Some((sha, dirty))
}

/// The files whose change means HEAD or the status moved: HEAD, the index,
/// the refs. Watched whenever there's a repository at all, so the first
/// commit of a new checkout is picked up too.
fn watch_paths(dir: &Path) -> Vec<PathBuf> {
    // `.git` is a file in a worktree: ask git where things are.
    let abs = |p: String| {
        let p = PathBuf::from(p);
        if p.is_absolute() { p } else { dir.join(p) }
    };
    let Some(git_dir) = git(dir, &["rev-parse", "--git-dir"]).map(abs) else {
        return Vec::new();
    };
    let common = git(dir, &["rev-parse", "--git-common-dir"])
        .map(abs)
        .unwrap_or_else(|| git_dir.clone());
    let mut watch = vec![
        git_dir.join("HEAD"),
        git_dir.join("index"),
        common.join("packed-refs"),
    ];
    // The branch HEAD points at (absent while detached).
    if let Some(r) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        watch.push(common.join(r));
    }
    watch.retain(|p| p.exists());
    watch
}

/// Windows only: embed the icon Explorer and the taskbar show into the exe.
/// The manifest (per-monitor DPI awareness, common controls v6) comes from
/// GPUI's own `windows-manifest` feature, and a second one would be a
/// duplicate resource. Other targets need nothing here.
fn windows_icon(manifest: &Path) {
    let icon = manifest.join("../../packaging/Blygger.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // An absolute path, so neither rc.exe nor windres has to guess what a
    // relative one is relative to.
    let icon = icon.canonicalize().unwrap_or(icon);
    let rc = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("blygger.rc");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
