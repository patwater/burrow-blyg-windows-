//! Installing a verified release over the running bundle.
//!
//! The zip is extracted with `ditto` into a staging directory on the same
//! volume as the running `Blygger.app`, the extracted bundle is checked
//! (identifier, version, `codesign --verify`), and on restart or quit it
//! replaces the running bundle: old bundle aside, new bundle in, old bundle
//! removed, and the old one put back if anything fails. Nothing but the
//! bundle the app is running from is ever touched.

use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use semver::Version;

use super::UpdateError;

pub const BUNDLE_ID: &str = "org.blygger.desktop";
/// The bundle's name inside the release zip (`ditto -c -k --keepParent`).
pub const ZIP_APP: &str = "Blygger.app";
const STAGING_NAME: &str = "org.blygger.desktop.update";

/// The external tools the installer runs, behind a trait so tests can stub
/// the ones that need a real signed bundle.
pub trait Tools: Send + Sync {
    /// A string value from an Info.plist.
    fn plist_string(&self, plist: &Path, key: &str) -> Result<String, UpdateError> {
        let out = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(plist)
            .stderr(Stdio::null())
            .output()
            .map_err(|e| UpdateError::Install(format!("couldn't run plutil: {e}")))?;
        if !out.status.success() {
            return Err(UpdateError::Install(format!(
                "{key} is missing from {}",
                plist.display()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `codesign --verify --strict` (the ad-hoc signature, or Developer ID).
    fn codesign_verify(&self, bundle: &Path) -> Result<(), UpdateError> {
        let ok = Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(bundle)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| UpdateError::Install(format!("couldn't run codesign: {e}")))?
            .success();
        if ok {
            Ok(())
        } else {
            Err(UpdateError::Verify(
                "the new app's code signature doesn't verify".into(),
            ))
        }
    }

    /// `ditto -x -k`: keeps the bundle's structure, symlinks and xattrs.
    fn extract_zip(&self, zip: &Path, into: &Path) -> Result<(), UpdateError> {
        let ok = Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(zip)
            .arg(into)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| UpdateError::Install(format!("couldn't run ditto: {e}")))?
            .success();
        if ok {
            Ok(())
        } else {
            Err(UpdateError::Install("couldn't unzip the update".into()))
        }
    }
}

/// The real tools.
pub struct System;
impl Tools for System {}

/// `…/X.app/Contents/MacOS/blygger` → `…/X.app`.
pub fn bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_app = app.extension().is_some_and(|e| e == "app");
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app)
        .then(|| app.to_path_buf())
}

/// The bundle this process runs from, or why it can't be updated in place.
pub fn running_bundle(tools: &dyn Tools) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find the app ({e})"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let bundle = bundle_from_exe(&exe).ok_or("it isn't running from an app bundle")?;
    can_update_in_place(&bundle, tools)?;
    Ok(bundle)
}

/// Whether `bundle` is ours and its folder is writable.
pub fn can_update_in_place(bundle: &Path, tools: &dyn Tools) -> Result<(), String> {
    if bundle
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation")
    {
        return Err(
            "macOS is running it from a temporary copy (move it to Applications first)".into(),
        );
    }
    let id = tools
        .plist_string(&bundle.join("Contents/Info.plist"), "CFBundleIdentifier")
        .map_err(|_| "its Info.plist can't be read".to_string())?;
    if id != BUNDLE_ID {
        return Err(format!("it isn't {BUNDLE_ID} ({id})"));
    }
    let parent = bundle.parent().ok_or("it has no parent folder")?;
    let probe = parent.join(format!(".blygger-write-test-{}", std::process::id()));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            Ok(())
        }
        Err(_) => Err(format!("{} isn't writable", parent.display())),
    }
}

/// A staging directory on the same volume as `target` (so the final move is
/// a rename): under `tmp_root` when that's the same volume, else a hidden
/// folder next to the bundle.
pub fn staging_dir(target: &Path, tmp_root: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or(Path::new("/"));
    #[cfg(unix)]
    let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
    // No device ids off Unix: stage next to the bundle, which is always the
    // same volume. (In-place updates are macOS-only; see `disabled_reason`.)
    #[cfg(not(unix))]
    let dev = |_: &Path| None::<u64>;
    match (dev(parent), dev(tmp_root)) {
        (Some(a), Some(b)) if a == b => tmp_root.join(STAGING_NAME),
        _ => parent.join(format!(".{STAGING_NAME}")),
    }
}

/// A verified, extracted bundle waiting to be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub version: Version,
    /// `<staging>/unpacked/Blygger.app`.
    pub app: PathBuf,
    /// The staging directory (removed after installing).
    pub dir: PathBuf,
}

/// Extract `zip` (already hash-checked) for `target`, and verify the result.
pub fn stage(
    zip: &[u8],
    target: &Path,
    tmp_root: &Path,
    expected: &Version,
    current: &Version,
    tools: &dyn Tools,
) -> Result<Staged, UpdateError> {
    let dir = staging_dir(target, tmp_root);
    remove_dir_if_exists(&dir)?;
    std::fs::create_dir_all(&dir).map_err(io_err("create the staging folder"))?;
    let result = (|| {
        let zip_path = dir.join("update.zip");
        std::fs::write(&zip_path, zip).map_err(io_err("save the update"))?;
        let unpacked = dir.join("unpacked");
        tools.extract_zip(&zip_path, &unpacked)?;
        let _ = std::fs::remove_file(&zip_path);
        let app = unpacked.join(ZIP_APP);
        if !app.is_dir() {
            return Err(UpdateError::Install(format!("{ZIP_APP} isn't in the zip")));
        }
        verify_bundle(&app, expected, current, tools)?;
        Ok(Staged {
            version: expected.clone(),
            app,
            dir: dir.clone(),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// The new bundle is ours, is the release it claims to be, is newer than
/// what's running, and its code signature verifies.
pub fn verify_bundle(
    app: &Path,
    expected: &Version,
    current: &Version,
    tools: &dyn Tools,
) -> Result<(), UpdateError> {
    let plist = app.join("Contents/Info.plist");
    let id = tools.plist_string(&plist, "CFBundleIdentifier")?;
    if id != BUNDLE_ID {
        return Err(UpdateError::Verify(format!(
            "the new app's identifier is {id}, not {BUNDLE_ID}"
        )));
    }
    let v = tools.plist_string(&plist, "CFBundleShortVersionString")?;
    let v = Version::parse(&v)
        .map_err(|_| UpdateError::Verify(format!("the new app's version {v} isn't semver")))?;
    if v != *expected {
        return Err(UpdateError::Verify(format!(
            "the new app is version {v}, not the release's {expected}"
        )));
    }
    if v <= *current {
        return Err(UpdateError::Verify(format!(
            "the new app ({v}) isn't newer than this one ({current})"
        )));
    }
    let exe = tools.plist_string(&plist, "CFBundleExecutable")?;
    if exe.contains('/') || !app.join("Contents/MacOS").join(&exe).is_file() {
        return Err(UpdateError::Verify("the new app has no executable".into()));
    }
    tools.codesign_verify(app)
}

/// Re-check a staged bundle and move it over `target`; on success the
/// staging folder is removed.
pub fn install_staged(
    target: &Path,
    staged: &Staged,
    current: &Version,
    tools: &dyn Tools,
) -> Result<(), UpdateError> {
    verify_bundle(&staged.app, &staged.version, current, tools)?;
    swap(target, &staged.app)?;
    let _ = std::fs::remove_dir_all(&staged.dir);
    Ok(())
}

/// Replace the bundle at `target` with the one at `new` (same volume):
/// old aside, new in, old removed. If the new one can't be moved in, the
/// old one is put back.
pub fn swap(target: &Path, new: &Path) -> Result<(), UpdateError> {
    let name = target
        .file_name()
        .ok_or_else(|| UpdateError::Install("the app has no name".into()))?;
    let old = target.with_file_name(format!(".{}.old", name.to_string_lossy()));
    remove_dir_if_exists(&old)?;
    std::fs::rename(target, &old).map_err(io_err("move the old app aside"))?;
    if let Err(e) = std::fs::rename(new, target) {
        return match std::fs::rename(&old, target) {
            Ok(()) => Err(UpdateError::Install(format!(
                "couldn't move the new app in ({e}); the old one is back"
            ))),
            Err(e2) => Err(UpdateError::Install(format!(
                "couldn't move the new app in ({e}) or restore the old one ({e2}); \
                 it's at {}",
                old.display()
            ))),
        };
    }
    // The running process keeps its (now unlinked) files open; that's fine.
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Reopen `bundle` once this process has exited (a detached `/bin/sh`).
/// `open -n` launches exactly that bundle, even if another copy of Blygger
/// (same identifier, another path) is running.
pub fn relaunch_after_exit(bundle: &Path) -> io::Result<()> {
    relaunch_command(std::process::id(), bundle, &["/usr/bin/open", "-n"])
        .spawn()
        .map(|_| ())
}

/// Waits (up to a minute) for `pid` to exit, then runs `<opener…> <bundle>`.
/// Arguments are passed positionally, never spliced into the script.
pub fn relaunch_command(pid: u32, bundle: &Path, opener: &[&str]) -> Command {
    const SCRIPT: &str = r#"pid="$1"; shift
i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.2; i=$((i+1)); done
exec "$@""#;
    let mut c = Command::new("/bin/sh");
    c.arg("-c")
        .arg(SCRIPT)
        .arg("blygger-relaunch")
        .arg(pid.to_string())
        .args(opener)
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    c.process_group(0);
    c
}

fn remove_dir_if_exists(p: &Path) -> Result<(), UpdateError> {
    match std::fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(UpdateError::Install(format!(
            "couldn't clear {}: {e}",
            p.display()
        ))),
    }
}

fn io_err(what: &'static str) -> impl Fn(io::Error) -> UpdateError {
    move |e| UpdateError::Install(format!("couldn't {what}: {e}"))
}
