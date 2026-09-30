//! Asking GitHub for the latest release, and deciding whether it's an
//! update: version comparison, asset selection and check throttling.

use semver::Version;
use serde::Deserialize;

use super::UpdateError;
use super::net::Http;

/// The GitHub repository (set with scripts/set-repo.sh).
macro_rules! repo {
    () => {
        "aneeshsathe/blygger-desktop"
    };
}
pub const LATEST_URL: &str = concat!("https://api.github.com/repos/", repo!(), "/releases/latest");

/// How often to check while running (and between launches).
pub const INTERVAL_SECS: u64 = 24 * 60 * 60;
/// After a failed check (offline, rate limited), try again in an hour.
pub const RETRY_SECS: u64 = 60 * 60;

/// The part of GitHub's release JSON the updater reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<ApiAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// A newer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: Version,
    /// The release page ("What's new").
    pub page_url: String,
    /// `Blygger-<ver>-macos-universal.zip`, `SHA256SUMS`, `SHA256SUMS.sig`;
    /// `None` when the release can't be installed from the app (a release
    /// from before signing, or a missing asset): then it's notify-only.
    pub assets: Option<Assets>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assets {
    pub zip_name: String,
    pub zip_url: String,
    pub zip_size: u64,
    pub sums_url: String,
    pub sig_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate { latest: Option<Version> },
    Available(Offer),
}

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is semver")
}

/// `v0.3.0` → 0.3.0.
pub fn parse_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.trim().strip_prefix('v').unwrap_or(tag.trim())).ok()
}

pub fn zip_name(v: &Version) -> String {
    format!("Blygger-{v}-macos-universal.zip")
}

/// Is `release` an update from `current`, and what would be downloaded?
pub fn evaluate(release: &ApiRelease, current: &Version) -> CheckOutcome {
    let Some(version) = parse_tag(&release.tag_name) else {
        return CheckOutcome::UpToDate { latest: None };
    };
    if release.draft || release.prerelease || !version.pre.is_empty() {
        return CheckOutcome::UpToDate { latest: None };
    }
    if version <= *current {
        return CheckOutcome::UpToDate {
            latest: Some(version),
        };
    }
    let find = |name: &str| release.assets.iter().find(|a| a.name == name);
    let zip_name = zip_name(&version);
    let assets = match (find(&zip_name), find("SHA256SUMS"), find("SHA256SUMS.sig")) {
        (Some(zip), Some(sums), Some(sig)) => Some(Assets {
            zip_name,
            zip_url: zip.browser_download_url.clone(),
            zip_size: zip.size,
            sums_url: sums.browser_download_url.clone(),
            sig_url: sig.browser_download_url.clone(),
        }),
        _ => None,
    };
    CheckOutcome::Available(Offer {
        version,
        page_url: release.html_url.clone(),
        assets,
    })
}

/// GET the latest release and evaluate it.
pub fn check(
    http: &dyn Http,
    api_url: &str,
    current: &Version,
) -> Result<CheckOutcome, UpdateError> {
    let body = http.get(api_url, 1024 * 1024)?;
    let release: ApiRelease = serde_json::from_slice(&body)
        .map_err(|e| UpdateError::Network(format!("unexpected release JSON: {e}")))?;
    Ok(evaluate(&release, current))
}

/// When the first check of this launch is due: now (plus the launch delay)
/// unless a check within the last day already found nothing newer.
/// A `last` in the future (a clock that jumped back) counts as stale.
pub fn first_due(now: u64, last_up_to_date: Option<u64>) -> u64 {
    match last_up_to_date {
        Some(t) if t <= now && now - t < INTERVAL_SECS => t + INTERVAL_SECS,
        _ => now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const REPO: &str = repo!();

    fn asset(name: &str) -> ApiAsset {
        ApiAsset {
            name: name.into(),
            browser_download_url: format!("https://github.com/{REPO}/releases/download/v/{name}"),
            size: 10,
        }
    }

    fn release(tag: &str, names: &[&str]) -> ApiRelease {
        ApiRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            html_url: format!("https://github.com/{REPO}/releases/tag/{tag}"),
            assets: names.iter().map(|n| asset(n)).collect(),
        }
    }

    const FULL: &[&str] = &[
        "Blygger-0.3.0-macos-universal.dmg",
        "Blygger-0.3.0-macos-universal.zip",
        "Blygger-macos-universal.zip",
        "SHA256SUMS",
        "SHA256SUMS.sig",
    ];

    #[test]
    fn versions_compare_as_semver() {
        let cur = Version::parse("0.2.0").unwrap();
        assert!(matches!(
            evaluate(&release("v0.3.0", FULL), &cur),
            CheckOutcome::Available(_)
        ));
        let cur10 = Version::parse("0.10.0").unwrap();
        assert_eq!(
            evaluate(&release("v0.9.0", FULL), &cur10),
            CheckOutcome::UpToDate {
                latest: Some(Version::parse("0.9.0").unwrap())
            },
            "0.9 < 0.10 (not a string compare)"
        );
        let same = Version::parse("0.3.0").unwrap();
        assert!(matches!(
            evaluate(&release("0.3.0", FULL), &same),
            CheckOutcome::UpToDate { .. }
        ));
        assert!(matches!(
            evaluate(&release("latest", FULL), &cur),
            CheckOutcome::UpToDate { latest: None }
        ));
    }

    #[test]
    fn drafts_and_prereleases_are_skipped() {
        let cur = Version::parse("0.2.0").unwrap();
        let mut r = release("v0.3.0", FULL);
        r.prerelease = true;
        assert!(matches!(evaluate(&r, &cur), CheckOutcome::UpToDate { .. }));
        let mut r = release("v0.3.0", FULL);
        r.draft = true;
        assert!(matches!(evaluate(&r, &cur), CheckOutcome::UpToDate { .. }));
        let r = release("v0.3.0-beta.1", FULL);
        assert!(matches!(evaluate(&r, &cur), CheckOutcome::UpToDate { .. }));
    }

    #[test]
    fn assets_are_picked_by_exact_name() {
        let cur = Version::parse("0.2.0").unwrap();
        let CheckOutcome::Available(o) = evaluate(&release("v0.3.0", FULL), &cur) else {
            panic!()
        };
        let a = o.assets.unwrap();
        assert_eq!(a.zip_name, "Blygger-0.3.0-macos-universal.zip");
        assert!(a.zip_url.ends_with("/Blygger-0.3.0-macos-universal.zip"));
        assert!(a.sums_url.ends_with("/SHA256SUMS"));
        assert!(a.sig_url.ends_with("/SHA256SUMS.sig"));
        // A release without a signature (0.2.0 and earlier) is notify-only.
        let unsigned = &FULL[..4];
        let CheckOutcome::Available(o) = evaluate(&release("v0.3.0", unsigned), &cur) else {
            panic!()
        };
        assert_eq!(o.assets, None);
        assert_eq!(
            o.page_url,
            format!("https://github.com/{REPO}/releases/tag/v0.3.0")
        );
    }

    #[test]
    fn github_json_parses() {
        let json = r#"{"tag_name":"v0.3.0","draft":false,"prerelease":false,
            "html_url":"https://github.com/o/r/releases/tag/v0.3.0","name":"Blygger 0.3.0",
            "assets":[{"name":"SHA256SUMS","browser_download_url":"https://github.com/x","size":5,"id":1}]}"#;
        let r: ApiRelease = serde_json::from_str(json).unwrap();
        assert_eq!(r.assets[0].name, "SHA256SUMS");
    }

    #[test]
    fn checks_are_throttled_across_launches() {
        let now = 1_000_000_000;
        assert_eq!(first_due(now, None), now);
        assert_eq!(first_due(now, Some(now - 3600)), now - 3600 + INTERVAL_SECS);
        assert_eq!(first_due(now, Some(now - INTERVAL_SECS)), now);
        assert_eq!(first_due(now, Some(now + 7200)), now, "clock went back");
    }

    #[test]
    fn the_api_url_names_the_repo() {
        assert_eq!(
            LATEST_URL,
            format!("https://api.github.com/repos/{REPO}/releases/latest")
        );
    }
}
