//! What the About window shows, as plain data, and the "Copy build info"
//! text. Pure: no GPUI, so it's unit-tested directly.
//!
//! Never holds a token, and the copied text leaves out the data directory
//! and config file paths (they contain the user name).

use crate::prefs::AutoUpdate;
use crate::update::check::REPO;

/// How this binary was built (`build.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: &'static str,
    /// Short SHA, or "unknown" (built from a source tarball, or without git).
    pub sha: &'static str,
    pub dirty: bool,
    /// Unix seconds (`SOURCE_DATE_EPOCH` when it was set).
    pub epoch: Option<i64>,
    /// "release" or "debug".
    pub profile: &'static str,
    /// The architecture of the running slice: "arm64" or "x86_64".
    pub arch: &'static str,
    /// The executable is a universal (fat) binary, as releases are: `arch`
    /// is then the slice that's running.
    pub universal: bool,
}

impl BuildInfo {
    pub fn current() -> Self {
        BuildInfo {
            version: env!("CARGO_PKG_VERSION"),
            sha: env!("BLYGGER_GIT_SHA"),
            dirty: env!("BLYGGER_GIT_DIRTY") == "1",
            epoch: env!("BLYGGER_BUILD_EPOCH").parse().ok(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            arch: arch_name(std::env::consts::ARCH),
            universal: {
                static FAT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                *FAT.get_or_init(|| std::env::current_exe().is_ok_and(|p| is_universal(&p)))
            },
        }
    }

    /// `1a2b3c4`, `1a2b3c4-dirty`, or `unknown`.
    pub fn commit(&self) -> String {
        match (self.sha, self.dirty) {
            ("" | "unknown", _) => "unknown".into(),
            (sha, true) => format!("{sha}-dirty"),
            (sha, false) => sha.into(),
        }
    }

    /// `2026-09-27 14:03 UTC`.
    pub fn date(&self) -> String {
        self.epoch
            .map(|s| utc(s as u64))
            .unwrap_or_else(|| "unknown".into())
    }

    /// `arm64`, or `arm64 (universal binary)`.
    pub fn arch_line(&self) -> String {
        if self.universal {
            format!("{} (universal binary)", self.arch)
        } else {
            self.arch.to_string()
        }
    }
}

/// Whether `exe` starts with a Mach-O fat header (`lipo -create` output).
pub fn is_universal(exe: &std::path::Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 4];
    std::fs::File::open(exe)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && matches!(u32::from_be_bytes(magic), 0xcafe_babe | 0xcafe_babf)
}

/// Apple's names: `aarch64` is arm64.
pub fn arch_name(rust_arch: &'static str) -> &'static str {
    match rust_arch {
        "aarch64" => "arm64",
        other => other,
    }
}

/// Unix seconds as `YYYY-MM-DD HH:MM UTC`.
pub fn utc(secs: u64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn repo_url() -> String {
    format!("https://github.com/{REPO}")
}

/// The GitHub release page for `version` (tags are `v<version>`).
pub fn release_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{version}")
}

pub fn license_url() -> String {
    format!("https://github.com/{REPO}/blob/main/LICENSE")
}

pub fn auto_update_value(a: AutoUpdate) -> &'static str {
    match a {
        AutoUpdate::Install => "install",
        AutoUpdate::Notify => "notify",
        AutoUpdate::Off => "off",
    }
}

/// A server capability, as far as the app has found out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub label: &'static str,
    pub on: bool,
}

/// Where the app is connected, without secrets: the host only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionInfo {
    Live {
        host: String,
        caps: Vec<Capability>,
    },
    /// `BLYGGER_FAKE`: in-memory sample data.
    Sample {
        caps: Vec<Capability>,
    },
    Disconnected,
}

impl ConnectionInfo {
    pub fn summary(&self) -> String {
        match self {
            ConnectionInfo::Live { host, .. } => host.clone(),
            ConnectionInfo::Sample { .. } => "sample data (BLYGGER_FAKE)".into(),
            ConnectionInfo::Disconnected => "not connected".into(),
        }
    }

    pub fn caps(&self) -> &[Capability] {
        match self {
            ConnectionInfo::Live { caps, .. } | ConnectionInfo::Sample { caps } => caps,
            ConnectionInfo::Disconnected => &[],
        }
    }

    /// Connected to a server without Burrow's extensions (a stock
    /// blygger-studio): the About window says what that limits.
    pub fn limited(&self) -> bool {
        matches!(self, ConnectionInfo::Live { caps, .. }
            if caps.iter().any(|c| c.label == EXTENSIONS && !c.on))
    }
}

const EXTENSIONS: &str = "Server extensions";

/// The capabilities, in display order, from what the backend recorded:
/// the docs/SERVER.md extensions at all, read state across Macs, and AI
/// disclosure for text generated in the app.
pub fn capabilities(extensions: bool, read_sync: bool, provenance: bool) -> Vec<Capability> {
    vec![
        Capability {
            label: EXTENSIONS,
            on: extensions,
        },
        Capability {
            label: "Read-state sync",
            on: read_sync,
        },
        Capability {
            label: "AI disclosure",
            on: provenance,
        },
    ]
}

/// The updater, as the About window describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub auto_update: AutoUpdate,
    /// Unix seconds of the last check that found nothing newer.
    pub last_check: Option<u64>,
    /// "Burrow 0.4.0 is downloaded and ready", "up to date", "checks are
    /// off (…)", and so on.
    pub status: String,
}

impl UpdateInfo {
    pub fn last_check_text(&self) -> String {
        self.last_check.map(utc).unwrap_or_else(|| "never".into())
    }
}

/// Everything "Copy build info" puts on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub build: BuildInfo,
    pub macos: String,
    pub update: UpdateInfo,
    pub connection: ConnectionInfo,
}

impl Report {
    /// Plain text for a bug report. No paths (they contain the user name),
    /// no token.
    pub fn copy_text(&self) -> String {
        let b = &self.build;
        let mut rows: Vec<(&str, String)> = vec![
            ("Version", b.version.to_string()),
            ("Commit", b.commit()),
            ("Built", b.date()),
            ("Profile", b.profile.to_string()),
            ("Architecture", b.arch_line()),
            ("macOS", self.macos.clone()),
            (
                "Auto-update",
                auto_update_value(self.update.auto_update).to_string(),
            ),
            ("Last check", self.update.last_check_text()),
            ("Update", self.update.status.clone()),
            ("Blyg", self.connection.summary()),
        ];
        for c in self.connection.caps() {
            rows.push((c.label, if c.on { "yes" } else { "no" }.to_string()));
        }
        let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 1;
        let mut out = format!("Burrow {}\n", b.version);
        for (k, v) in rows {
            out.push_str(&format!("{:<width$} {v}\n", format!("{k}:")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> BuildInfo {
        BuildInfo {
            version: "0.3.0",
            sha: "1a2b3c4",
            dirty: false,
            epoch: Some(1_790_000_000),
            profile: "release",
            arch: "arm64",
            universal: true,
        }
    }

    fn report() -> Report {
        Report {
            build: build(),
            macos: "15.6.1".into(),
            update: UpdateInfo {
                auto_update: AutoUpdate::Notify,
                last_check: Some(1_790_003_600),
                status: "up to date".into(),
            },
            connection: ConnectionInfo::Live {
                host: "blyg.example.com".into(),
                caps: capabilities(true, false, true),
            },
        }
    }

    #[test]
    fn commit_marks_a_dirty_tree_and_an_unknown_one() {
        let mut b = build();
        assert_eq!(b.commit(), "1a2b3c4");
        b.dirty = true;
        assert_eq!(b.commit(), "1a2b3c4-dirty");
        b.sha = "unknown";
        assert_eq!(b.commit(), "unknown", "no -dirty on an unknown commit");
        b.sha = "";
        assert_eq!(b.commit(), "unknown");
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(build().date(), "2026-09-21 14:13 UTC");
        let b = BuildInfo {
            epoch: None,
            ..build()
        };
        assert_eq!(b.date(), "unknown");
    }

    #[test]
    fn arch_names_follow_apple() {
        assert_eq!(arch_name("aarch64"), "arm64");
        assert_eq!(arch_name("x86_64"), "x86_64");
        assert_eq!(build().arch_line(), "arm64 (universal binary)");
        let thin = BuildInfo {
            universal: false,
            ..build()
        };
        assert_eq!(thin.arch_line(), "arm64");
    }

    #[test]
    fn fat_headers_are_universal() {
        let dir = tempfile::tempdir().unwrap();
        let fat = dir.path().join("fat");
        std::fs::write(&fat, [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]).unwrap();
        assert!(is_universal(&fat));
        let thin = dir.path().join("thin");
        // MH_MAGIC_64, little-endian, as a one-architecture build starts.
        std::fs::write(&thin, [0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]).unwrap();
        assert!(!is_universal(&thin));
        assert!(!is_universal(&dir.path().join("missing")));
    }

    #[test]
    fn links_point_at_the_update_repo() {
        assert_eq!(repo_url(), format!("https://github.com/{REPO}"));
        assert_eq!(
            release_url("0.3.0"),
            format!("https://github.com/{REPO}/releases/tag/v0.3.0")
        );
        assert!(license_url().ends_with("/LICENSE"));
        assert!(crate::update::check::LATEST_URL.contains(REPO));
    }

    #[test]
    fn the_embedded_build_info_is_sane() {
        let b = BuildInfo::current();
        assert_eq!(b.version, env!("CARGO_PKG_VERSION"));
        assert!(!b.sha.is_empty());
        assert!(b.sha == "unknown" || b.sha.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(b.epoch.is_some_and(|e| e > 0));
        assert!(["arm64", "x86_64"].contains(&b.arch));
        assert!(!b.universal, "cargo test builds one architecture");
    }

    #[test]
    fn copy_text_lists_everything_but_paths_and_secrets() {
        let text = report().copy_text();
        let want = "\
Burrow 0.3.0
Version:           0.3.0
Commit:            1a2b3c4
Built:             2026-09-21 14:13 UTC
Profile:           release
Architecture:      arm64 (universal binary)
macOS:             15.6.1
Auto-update:       notify
Last check:        2026-09-21 15:13 UTC
Update:            up to date
Blyg:              blyg.example.com
Server extensions: yes
Read-state sync:   no
AI disclosure:     yes
";
        assert_eq!(text, want);
        assert!(!text.contains('/'), "no paths or URLs: {text}");
    }

    #[test]
    fn copy_text_when_disconnected_or_on_sample_data() {
        let mut r = report();
        r.connection = ConnectionInfo::Disconnected;
        r.update.last_check = None;
        let text = r.copy_text();
        assert!(text.contains("Blyg:         not connected\n"), "{text}");
        assert!(text.contains("Last check:   never\n"), "{text}");
        assert!(!text.contains("AI disclosure"), "{text}");
        r.connection = ConnectionInfo::Sample {
            caps: capabilities(true, true, true),
        };
        assert!(r.copy_text().contains("sample data (BLYGGER_FAKE)"));
    }
}
