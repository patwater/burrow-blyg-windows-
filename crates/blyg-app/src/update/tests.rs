//! The updater end to end, offline: a fake HTTP client serving a release
//! signed with a throwaway key (fixed seed), fake bundles in a temp dir,
//! and the real `ditto` / `plutil` (codesign is stubbed: the fake bundles
//! aren't signed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ed25519_dalek::{Signer as _, SigningKey};
use semver::Version;

use super::check::{self, Assets, CheckOutcome, Offer};
use super::install::{self, Tools};
use super::net::Http;
use super::*;

fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

#[derive(Default)]
struct FakeHttp {
    files: HashMap<String, Vec<u8>>,
    requested: Mutex<Vec<String>>,
}

impl Http for FakeHttp {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, UpdateError> {
        self.requested.lock().unwrap().push(url.to_string());
        let body = self
            .files
            .get(url)
            .cloned()
            .ok_or_else(|| UpdateError::Network(format!("HTTP 404 from {url}")))?;
        if body.len() as u64 > limit {
            return Err(UpdateError::Network("too large".into()));
        }
        Ok(body)
    }
}

impl FakeHttp {
    fn fetched(&self, suffix: &str) -> bool {
        self.requested
            .lock()
            .unwrap()
            .iter()
            .any(|u| u.ends_with(suffix))
    }
}

/// Real plutil and ditto; codesign answers `codesign_ok`.
struct FakeTools {
    codesign_ok: bool,
}

impl Tools for FakeTools {
    fn codesign_verify(&self, _bundle: &Path) -> Result<(), UpdateError> {
        if self.codesign_ok {
            Ok(())
        } else {
            Err(UpdateError::Verify("codesign says no".into()))
        }
    }
}

fn make_bundle(dir: &Path, id: &str, version: &str, marker: &str) -> PathBuf {
    let app = dir.join("Blygger.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>blygger</string>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
</dict>
</plist>
"#
        ),
    )
    .unwrap();
    std::fs::write(app.join("Contents/MacOS/blygger"), marker).unwrap();
    app
}

fn zip_bundle(app: &Path, out: &Path) -> Vec<u8> {
    let ok = std::process::Command::new("/usr/bin/ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(app)
        .arg(out)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    std::fs::read(out).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const BASE: &str = "https://github.com/example/blygger-desktop/releases/download/v9.9.9";

/// A temp world: an installed 0.2.0 bundle, and a signed 9.9.9 release.
struct World {
    _tmp: tempfile::TempDir,
    target: PathBuf,
    tmp_root: PathBuf,
    http: Arc<FakeHttp>,
    assets: Assets,
    version: Version,
}

struct ReleaseSpec<'a> {
    id: &'a str,
    plist_version: &'a str,
    sign_with: SigningKey,
    tamper_zip: bool,
}

impl Default for ReleaseSpec<'_> {
    fn default() -> Self {
        ReleaseSpec {
            id: install::BUNDLE_ID,
            plist_version: "9.9.9",
            sign_with: test_key(),
            tamper_zip: false,
        }
    }
}

fn world(spec: ReleaseSpec) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let apps = tmp.path().join("Applications");
    let target = make_bundle(&apps, install::BUNDLE_ID, "0.2.0", "old");
    let tmp_root = tmp.path().join("tmp");
    std::fs::create_dir_all(&tmp_root).unwrap();

    let build = tmp.path().join("build");
    let new = make_bundle(&build, spec.id, spec.plist_version, "new");
    let version = Version::parse("9.9.9").unwrap();
    let zip_name = check::zip_name(&version);
    let mut zip = zip_bundle(&new, &tmp.path().join(&zip_name));
    let sums = format!(
        "{}  {zip_name}\n{}  Blygger-9.9.9-macos-universal.dmg\n",
        hex(&verify::sha256(&zip)),
        hex(&verify::sha256(b"dmg"))
    );
    let sig = spec.sign_with.sign(sums.as_bytes()).to_bytes();
    if spec.tamper_zip {
        let n = zip.len();
        zip[n / 2] ^= 0xff;
    }
    let assets = Assets {
        zip_name: zip_name.clone(),
        zip_url: format!("{BASE}/{zip_name}"),
        zip_size: zip.len() as u64,
        sums_url: format!("{BASE}/SHA256SUMS"),
        sig_url: format!("{BASE}/SHA256SUMS.sig"),
    };
    let mut http = FakeHttp::default();
    http.files.insert(assets.zip_url.clone(), zip);
    http.files
        .insert(assets.sums_url.clone(), sums.into_bytes());
    http.files.insert(assets.sig_url.clone(), sig.to_vec());
    World {
        _tmp: tmp,
        target,
        tmp_root,
        http: Arc::new(http),
        assets,
        version,
    }
}

impl World {
    fn env(&self, codesign_ok: bool) -> Env {
        Env {
            http: self.http.clone(),
            tools: Arc::new(FakeTools { codesign_ok }),
            key: test_key().verifying_key(),
            api_url: "https://api.github.com/repos/example/blygger-desktop/releases/latest".into(),
            current: Version::parse("0.2.0").unwrap(),
            tmp_root: self.tmp_root.clone(),
        }
    }

    fn stage(&self, codesign_ok: bool) -> Result<install::Staged, UpdateError> {
        download_and_stage(
            &self.env(codesign_ok),
            &self.version,
            &self.assets,
            &self.target,
        )
    }

    fn installed_marker(&self) -> String {
        std::fs::read_to_string(self.target.join("Contents/MacOS/blygger")).unwrap()
    }
}

#[test]
fn a_signed_release_is_staged_and_installed_over_the_old_bundle() {
    let w = world(ReleaseSpec::default());
    let staged = w.stage(true).unwrap();
    assert_eq!(staged.version, w.version);
    assert!(
        staged.app.starts_with(&w.tmp_root),
        "same volume → under tmp"
    );
    assert_eq!(w.installed_marker(), "old", "nothing installed yet");

    let tools = FakeTools { codesign_ok: true };
    let current = Version::parse("0.2.0").unwrap();
    install::install_staged(&w.target, &staged, &current, &tools).unwrap();
    assert_eq!(w.installed_marker(), "new");
    assert!(!staged.dir.exists(), "staging cleaned up");
    let parent = w.target.parent().unwrap();
    let left: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["Blygger.app"], "the old bundle is gone");
    assert_eq!(
        tools
            .plist_string(
                &w.target.join("Contents/Info.plist"),
                "CFBundleShortVersionString"
            )
            .unwrap(),
        "9.9.9"
    );
}

#[test]
fn a_bad_signature_is_refused_before_the_zip_is_downloaded() {
    let w = world(ReleaseSpec {
        sign_with: SigningKey::from_bytes(&[1u8; 32]),
        ..Default::default()
    });
    let err = w.stage(true).unwrap_err();
    assert!(matches!(err, UpdateError::Verify(_)), "{err:?}");
    assert!(err.to_string().contains("signature"), "{err}");
    assert!(w.http.fetched("/SHA256SUMS.sig"));
    assert!(!w.http.fetched(".zip"), "the zip is never downloaded");
    assert_eq!(w.installed_marker(), "old");
}

#[test]
fn the_real_release_key_rejects_a_test_signed_release() {
    let w = world(ReleaseSpec::default());
    let env = Env {
        key: verify::release_key(),
        ..w.env(true)
    };
    let err = download_and_stage(&env, &w.version, &w.assets, &w.target).unwrap_err();
    assert!(matches!(err, UpdateError::Verify(_)), "{err:?}");
}

#[test]
fn a_zip_that_doesnt_match_the_signed_sums_is_refused() {
    let w = world(ReleaseSpec {
        tamper_zip: true,
        ..Default::default()
    });
    let err = w.stage(true).unwrap_err();
    assert!(err.to_string().contains("SHA-256"), "{err}");
    assert_eq!(w.installed_marker(), "old");
}

#[test]
fn a_missing_signature_is_refused() {
    let mut w = world(ReleaseSpec::default());
    let sig = w.assets.sig_url.clone();
    Arc::get_mut(&mut w.http).unwrap().files.remove(&sig);
    let err = w.stage(true).unwrap_err();
    assert!(matches!(err, UpdateError::Network(_)), "{err:?}");
}

#[test]
fn a_bundle_with_another_identifier_is_refused() {
    let w = world(ReleaseSpec {
        id: "com.example.other",
        ..Default::default()
    });
    let err = w.stage(true).unwrap_err();
    assert!(err.to_string().contains("identifier"), "{err}");
    assert!(
        !install::staging_dir(&w.target, &w.tmp_root).exists(),
        "a refused update leaves no staging behind"
    );
}

#[test]
fn a_bundle_that_isnt_the_release_version_or_newer_is_refused() {
    // The zip's app claims an older version than the release tag.
    let w = world(ReleaseSpec {
        plist_version: "0.1.0",
        ..Default::default()
    });
    let err = w.stage(true).unwrap_err();
    assert!(err.to_string().contains("not the release's"), "{err}");

    // Even a consistent release isn't installed over a newer app.
    let w = world(ReleaseSpec::default());
    let env = Env {
        current: Version::parse("9.9.9").unwrap(),
        ..w.env(true)
    };
    let err = download_and_stage(&env, &w.version, &w.assets, &w.target).unwrap_err();
    assert!(err.to_string().contains("isn't newer"), "{err}");
}

#[test]
fn a_bundle_whose_code_signature_fails_is_refused() {
    let w = world(ReleaseSpec::default());
    let err = w.stage(false).unwrap_err();
    assert!(err.to_string().contains("codesign"), "{err}");
}

#[test]
fn a_staged_bundle_is_rechecked_before_install() {
    let w = world(ReleaseSpec::default());
    let staged = w.stage(true).unwrap();
    // Something changed it while it waited.
    std::fs::remove_file(staged.app.join("Contents/MacOS/blygger")).unwrap();
    let current = Version::parse("0.2.0").unwrap();
    let tools = FakeTools { codesign_ok: true };
    assert!(install::install_staged(&w.target, &staged, &current, &tools).is_err());
    assert_eq!(w.installed_marker(), "old");
}

#[test]
fn the_real_codesign_check_accepts_an_ad_hoc_bundle_and_notices_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let app = make_bundle(tmp.path(), install::BUNDLE_ID, "9.9.9", "");
    std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/blygger")).unwrap();
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .output()
        .unwrap()
        .status
        .success();
    assert!(signed, "ad-hoc signing a test bundle");
    let tools = install::System;
    tools.codesign_verify(&app).unwrap();
    let current = Version::parse("0.2.0").unwrap();
    install::verify_bundle(&app, &Version::parse("9.9.9").unwrap(), &current, &tools).unwrap();
    // A file added after signing breaks the seal.
    std::fs::write(app.join("Contents/MacOS/extra"), "x").unwrap();
    assert!(tools.codesign_verify(&app).is_err());
}

#[test]
fn a_failed_swap_puts_the_old_bundle_back() {
    let tmp = tempfile::tempdir().unwrap();
    let target = make_bundle(
        &tmp.path().join("Applications"),
        install::BUNDLE_ID,
        "0.2.0",
        "old",
    );
    let missing = tmp.path().join("nowhere/Blygger.app");
    let err = install::swap(&target, &missing).unwrap_err();
    assert!(err.to_string().contains("the old one is back"), "{err}");
    assert_eq!(
        std::fs::read_to_string(target.join("Contents/MacOS/blygger")).unwrap(),
        "old"
    );
    assert!(!target.with_file_name(".Blygger.app.old").exists());
}

#[test]
fn swap_replaces_a_stale_leftover() {
    let tmp = tempfile::tempdir().unwrap();
    let apps = tmp.path().join("Applications");
    let target = make_bundle(&apps, install::BUNDLE_ID, "0.2.0", "old");
    std::fs::create_dir_all(apps.join(".Blygger.app.old/junk")).unwrap();
    let new = make_bundle(&tmp.path().join("new"), install::BUNDLE_ID, "9.9.9", "new");
    install::swap(&target, &new).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("Contents/MacOS/blygger")).unwrap(),
        "new"
    );
    assert!(!apps.join(".Blygger.app.old").exists());
}

#[test]
fn only_our_writable_bundle_is_updated_in_place() {
    let tools = FakeTools { codesign_ok: true };
    let tmp = tempfile::tempdir().unwrap();
    let ours = make_bundle(&tmp.path().join("a"), install::BUNDLE_ID, "0.2.0", "x");
    install::can_update_in_place(&ours, &tools).unwrap();
    assert_eq!(
        std::fs::read_dir(tmp.path().join("a")).unwrap().count(),
        1,
        "the write probe is removed"
    );

    let other = make_bundle(&tmp.path().join("b"), "com.example.other", "0.2.0", "x");
    let err = install::can_update_in_place(&other, &tools).unwrap_err();
    assert!(err.contains("isn't org.blygger.desktop"), "{err}");

    let translocated = make_bundle(
        &tmp.path().join("AppTranslocation/ABC/d"),
        install::BUNDLE_ID,
        "0.2.0",
        "x",
    );
    assert!(
        install::can_update_in_place(&translocated, &tools)
            .unwrap_err()
            .contains("temporary copy")
    );

    use std::os::unix::fs::PermissionsExt as _;
    let ro = tmp.path().join("ro");
    let locked = make_bundle(&ro, install::BUNDLE_ID, "0.2.0", "x");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = install::can_update_in_place(&locked, &tools).unwrap_err();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.contains("isn't writable"), "{err}");
}

#[test]
fn the_bundle_is_found_from_the_executable_path() {
    assert_eq!(
        install::bundle_from_exe(Path::new(
            "/Applications/Blygger.app/Contents/MacOS/blygger"
        )),
        Some(PathBuf::from("/Applications/Blygger.app"))
    );
    assert_eq!(
        install::bundle_from_exe(Path::new("/x/My Blygger.app/Contents/MacOS/blygger")),
        Some(PathBuf::from("/x/My Blygger.app"))
    );
    // cargo run
    assert_eq!(
        install::bundle_from_exe(Path::new("/src/target/debug/blygger")),
        None
    );
    assert_eq!(
        install::bundle_from_exe(Path::new("/x/Blygger/Contents/MacOS/blygger")),
        None
    );
}

#[test]
fn staging_is_on_the_bundles_volume() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("Applications/Blygger.app");
    std::fs::create_dir_all(&target).unwrap();
    let root = tmp.path().join("tmp");
    std::fs::create_dir_all(&root).unwrap();
    assert!(install::staging_dir(&target, &root).starts_with(&root));
    // A tmp root that can't be examined → next to the bundle.
    let dir = install::staging_dir(&target, &tmp.path().join("missing"));
    assert_eq!(dir.parent(), target.parent());
    assert!(dir.file_name().unwrap().to_string_lossy().starts_with('.'));
}

#[test]
fn the_relauncher_waits_for_the_process_to_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("opened");
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("0.6")
        .spawn()
        .unwrap();
    // `touch` stands in for `open`.
    let mut relauncher = install::relaunch_command(child.id(), &marker, &["/usr/bin/touch"])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!marker.exists(), "still waiting for the process");
    child.wait().unwrap();
    relauncher.wait().unwrap();
    assert!(marker.exists(), "opened after the process exited");
}

#[test]
fn check_reads_the_release_json() {
    let json = format!(
        r#"{{"tag_name":"v9.9.9","draft":false,"prerelease":false,
        "html_url":"https://github.com/example/blygger-desktop/releases/tag/v9.9.9",
        "assets":[
          {{"name":"Blygger-9.9.9-macos-universal.zip","browser_download_url":"{BASE}/Blygger-9.9.9-macos-universal.zip","size":3}},
          {{"name":"SHA256SUMS","browser_download_url":"{BASE}/SHA256SUMS","size":1}},
          {{"name":"SHA256SUMS.sig","browser_download_url":"{BASE}/SHA256SUMS.sig","size":64}}]}}"#
    );
    let api = "https://api.github.com/repos/example/blygger-desktop/releases/latest";
    let mut http = FakeHttp::default();
    http.files.insert(api.into(), json.into_bytes());
    let cur = Version::parse("0.2.0").unwrap();
    let CheckOutcome::Available(offer) = check::check(&http, api, &cur).unwrap() else {
        panic!("expected an update")
    };
    assert_eq!(offer.version, Version::parse("9.9.9").unwrap());
    assert!(offer.assets.is_some());
    let newer = Version::parse("10.0.0").unwrap();
    assert!(matches!(
        check::check(&http, api, &newer).unwrap(),
        CheckOutcome::UpToDate { .. }
    ));
    assert!(check::check(&http, "https://api.github.com/nope", &cur).is_err());
}

fn offer(assets: bool) -> Offer {
    Offer {
        version: Version::parse("0.3.0").unwrap(),
        page_url: "https://github.com/example/blygger-desktop/releases/tag/v0.3.0".into(),
        assets: assets.then(|| Assets {
            zip_name: "z".into(),
            zip_url: "u".into(),
            zip_size: 1,
            sums_url: "s".into(),
            sig_url: "g".into(),
        }),
    }
}

#[test]
fn the_notice_says_what_to_do() {
    assert_eq!(Phase::Idle.notice(), None);
    assert_eq!(Phase::Checking { manual: true }.notice(), None);
    assert_eq!(
        Phase::Downloading {
            offer: offer(true),
            shown: false
        }
        .notice(),
        None,
        "background downloads are quiet"
    );
    let staged = install::Staged {
        version: Version::parse("0.3.0").unwrap(),
        app: "/tmp/x".into(),
        dir: "/tmp".into(),
    };
    let n = Phase::Ready {
        offer: offer(true),
        staged,
    }
    .notice()
    .unwrap();
    assert_eq!(n.text, "Blygger 0.3.0 is ready");
    assert_eq!(n.action, Some(("Restart to update", NoticeAction::Restart)));
    assert_eq!(n.link.unwrap().0, "What's new");

    let n = Phase::Available {
        offer: offer(true),
        blocked: None,
    }
    .notice()
    .unwrap();
    assert_eq!(n.text, "Blygger 0.3.0 is available");
    assert_eq!(n.action, Some(("Download", NoticeAction::Download)));

    let why = cant_update_in_place("it isn't running from an app bundle");
    let n = Phase::Available {
        offer: offer(true),
        blocked: Some(why),
    }
    .notice()
    .unwrap();
    assert_eq!(
        n.text,
        "Blygger 0.3.0 is available · Can't update in place: it isn't running from an app \
         bundle; download from the release page"
    );
    assert_eq!(n.action, None);
    assert!(n.link.unwrap().1.ends_with("/v0.3.0"));
}

#[test]
fn unsigned_releases_and_unwritable_installs_are_notify_only() {
    let ok: Result<PathBuf, String> = Ok("/Applications/Blygger.app".into());
    assert_eq!(blocked_reason(&ok, &offer(true)), None);
    assert!(
        blocked_reason(&ok, &offer(false))
            .unwrap()
            .contains("isn't signed")
    );
    let no: Result<PathBuf, String> = Err("Can't update in place: x".into());
    assert_eq!(
        blocked_reason(&no, &offer(true)).as_deref(),
        Some("Can't update in place: x")
    );
}

#[test]
fn tests_never_check() {
    assert_eq!(disabled_reason(), Some("test build"));
    check::current_version(); // CARGO_PKG_VERSION is semver
}
