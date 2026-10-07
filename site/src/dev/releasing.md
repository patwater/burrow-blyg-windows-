# Releasing

## The changelog

Every user-visible change gets a line in `CHANGELOG.md` under
`## [Unreleased]`, in the section that fits (`### Added`, `### Changed`,
`### Fixed`, …). The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/). Before 1.0, minor versions may
break things.

## Cutting a release

1. Rename `[Unreleased]` in `CHANGELOG.md` to `[x.y.z] - YYYY-MM-DD`, and add
   a fresh, empty `[Unreleased]` above it.
2. Bump `version` in the root `Cargo.toml`.
3. Commit, and merge it to `main` (through a PR, as usual). Don't push a tag.

On every push to `main`, `.github/workflows/release.yml` checks whether the
Cargo version has a `vx.y.z` tag yet. If it does, nothing runs. If not, it
checks that the version and the changelog agree, builds a universal app, signs `SHA256SUMS` for the
in-app updater, and creates the GitHub release, tagging the merged commit, with
that changelog section as its notes (`scripts/changelog-notes.sh x.y.z` prints it). A version with a
`-` in it (`0.7.0-rc.1`) is published as a prerelease, which the updater
skips.

It runs on `main` rather than on the tag so the build cache is saved where
the next release can read it: GitHub's cache from one tag can't be read by
another, so tag builds started cold every time. A failed release can be run
again from the Actions tab (**Run workflow** on Release).

The release needs the repository secret `UPDATE_SIGNING_KEY` (the Ed25519
key the app's updater trusts). Developer ID signing and notarization are
optional; the workflow's header lists their secrets.

## What a release contains

- versioned assets: `Burrow-<version>-macos-universal.zip` and `.dmg`;
- version-less copies, `Burrow-macos-universal.zip` and `.dmg`, which the
  `releases/latest/download/…` URLs (and the one-line installer) point to;
- `SHA256SUMS`, signed as `SHA256SUMS.sig`;
- for now, `Blygger-<version>-macos-universal.zip` and
  `Blygger-macos-universal.zip`: the same signed app in a folder called
  `Blygger.app`, because that's the only asset 0.6.0 and earlier can update
  from (see "the old name" in `scripts/bundle.sh`, and
  [The rename](../install.md#the-rename)).

## The full history

The [Changelog](../changelog.md) has every release's notes, and the
[Releases page](https://github.com/aneeshsathe/blygger-desktop/releases) has
the downloads.
