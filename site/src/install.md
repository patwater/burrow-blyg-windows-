# Install and update

Burrow runs on macOS 11 (Big Sur) or later, on Apple silicon or Intel.

## Install (recommended: one line in Terminal)

```sh
curl -fsSL https://raw.githubusercontent.com/aneeshsathe/blygger-desktop/main/scripts/install.sh | bash
```

This downloads the latest release, checks it against the release's
`SHA256SUMS`, and puts **Burrow.app** in `/Applications`, or in
`~/Applications` if you can't write to `/Applications`. To launch it, run
`open /Applications/Burrow.app` or find it in Spotlight. If a `Blygger.app`
from before the rename is in the same folder, the installer replaces it with
`Burrow.app` (your posts, settings and sign-ins aren't in the app, so they
carry over).

Why Terminal? Releases aren't notarized by Apple yet. Files downloaded with
`curl` don't get macOS's quarantine flag, so Gatekeeper doesn't block the app.
The script also clears the flag explicitly, in case a future macOS adds it.

To read the script before running it:

```sh
curl -fsSL -o install.sh https://raw.githubusercontent.com/aneeshsathe/blygger-desktop/main/scripts/install.sh
less install.sh
bash install.sh
```

Or install by hand, verifying the checksum yourself:

```sh
cd "$(mktemp -d)"
base=https://github.com/aneeshsathe/blygger-desktop/releases/latest/download
curl -fLO "$base/Burrow-macos-universal.zip" -fLO "$base/SHA256SUMS"
grep ' Burrow-macos-universal.zip$' SHA256SUMS | shasum -a 256 -c -   # must print "OK"
ditto -x -k Burrow-macos-universal.zip /Applications
open /Applications/Burrow.app
```

`BLYGGER_VERSION=0.1.0` installs a specific version, and `BLYGGER_DEST=<dir>`
installs somewhere else.

## Downloading from the Releases page in a browser

The [Releases page](https://github.com/aneeshsathe/blygger-desktop/releases/latest)
has a `.dmg` and a `.zip`. Files downloaded in a browser are quarantined, and
because the app is only ad-hoc signed (not notarized), macOS blocks the first
launch. On macOS 15 (Sequoia) and later, you see a dialog saying **"Burrow"
Not Opened**, with the message that Apple could not verify it is free of
malware, and only **Done** and **Move to Trash** buttons. Control-click →
**Open** no longer gets around this on macOS 15 and later. To allow it:

1. Drag Burrow to Applications and try to open it once. Click **Done**.
2. Open **System Settings › Privacy & Security** and scroll down to
   **Security**. It says "Burrow" was blocked to protect your Mac. Click
   **Open Anyway**. The button only appears for about an hour after the
   blocked attempt.
3. Confirm with **Open Anyway** in the next dialog and enter your password.
   After that, Burrow opens normally.

Or skip all that with Terminal:
`xattr -dr com.apple.quarantine /Applications/Burrow.app`

On macOS 14 and earlier, Control-click → **Open** → **Open** still works.

## Updates

From 0.3.0 on, Burrow updates itself. About once a day it checks the
[Releases page](https://github.com/aneeshsathe/blygger-desktop/releases/latest),
downloads a new version in the background, and shows **Burrow X is ready ·
Restart to update** in the status bar (quitting installs it too). **Burrow ›
Check for Updates…** checks right away.

Updates are signed: the app installs a release only if its `SHA256SUMS` carries
a valid Ed25519 signature from the project's release key (built into the app),
the download matches that file, and the new app is Burrow (the same bundle
ID, `org.blygger.desktop`, as before the rename), newer, and passes
`codesign --verify`. Downloads come only from GitHub, over HTTPS. The one-line
installer checks the same signature when OpenSSL 3 is installed.

`auto-update = notify` in the config only tells you a new version is out, and
`auto-update = off` stops the automatic checks (see
[`auto-update`](config.md#auto-update)). If Burrow can't replace itself (say,
the folder it's in isn't writable), it says so and links to the release page.

**On 0.2.0 or earlier?** Those versions can't update themselves. Run the
one-line installer above once more; after that, updates are automatic.

**Burrow › About Burrow** shows the version, commit, update status and what
your blyg's server supports. Its **Copy build info** button copies all of that
(no paths, no token) for a bug report.

## The rename

The app was called Blygger (Blygger Desktop) up to 0.6.0. Only the name you
see changed: the menus (Burrow › Settings…, Burrow › Check for Updates…,
Help › Burrow Tutorial), the About window, the window title and the app's
messages say Burrow, and new installs are `Burrow.app`. The bundle ID
(`org.blygger.desktop`), the config file (`~/.config/blygger/config`), the
`BLYGGER_*` environment variables, the `blygger` command, the data folder and
database, the Keychain items and the key-binding contexts are all the same, so
nothing needs migrating.

### Blygger.app → Burrow.app

**Updating from 0.6.0 or earlier:** the in-app update works as usual. The app
replaces itself where it is, so it stays `Blygger.app` on disk (Finder and
Spotlight show that name) while being Burrow inside. That's expected. To get
`Burrow.app`:

- run the [one-line installer](#install-recommended-one-line-in-terminal),
  which replaces a `Blygger.app` in the same folder (posts, settings and
  sign-ins carry over), or
- rename the app yourself while it isn't running.

A Dock shortcut to the old name may need re-adding.

For a transition period, each release also publishes
`Blygger-<version>-macos-universal.zip` and `Blygger-macos-universal.zip` (the
same signed app in a folder called `Blygger.app`), so 0.6.0 and earlier can
still update. The signed `SHA256SUMS` covers them too.

## Uninstall

Quit Burrow and move `Burrow.app` (or `Blygger.app`) to the Trash. Your posts
live on your blyg; the local copy, settings and sign-ins stay on your Mac
until you remove them:

- the data folder, `~/Library/Application Support/org.blygger.desktop/`
  (**Burrow › Disconnect…** can also delete the local copy);
- the config file, `~/.config/blygger/config`;
- the Keychain items, under the service `org.blygger.desktop` (search
  Keychain Access for `blygger`).
