# FAQ

### Why is it called Burrow, when everything says "blygger"?

Burrow is a blygger client: a client for the Blygger protocol. It was called
Blygger Desktop up to 0.6.0. Only the name you see changed. The bundle ID, the
`blygger` command, the config path (`~/.config/blygger/config`), the
`BLYGGER_*` environment variables, the data folder and the Keychain items kept
the old name, so existing installs carry on without migrating. See
[The rename](install.md#the-rename).

### My app is still called Blygger.app in Finder.

That's expected if it updated itself from 0.6.0 or earlier: the updater
replaces the app where it is, so the file keeps its name while being Burrow
inside. Run the [one-line installer](install.md) to get `Burrow.app`, or
rename it while it isn't running. See
[Blygger.app → Burrow.app](install.md#blyggerapp--burrowapp).

### Can I use Burrow with a stock Blygger blog?

Yes, if it runs blygger-studio 0.9 or later. On 0.28 or later, sign in with
your browser (or an API token from the studio's Client access page); on an
older one, with your studio password. On a current studio only one thing
needs an extension upstream doesn't have yet: read state across your Macs.
An older studio limits a little more (AI disclosure for text generated in
Burrow, removing images you pasted and then deleted), and Burrow tells you
which the first time it connects. See [Server requirements](server.md).

### Is my data safe? Where is it?

Your posts live on your blyg; Burrow keeps a local copy in
`~/Library/Application Support/org.blygger.desktop/`, so everything works
offline. Scratch notes and the notes drawer stay on your Mac until you make
them drafts. Your sign-in (browser grant, token or password) and AI keys are in the macOS Keychain, never in
a file, and never logged.

Burrow is vibecoded and provided as is, with no warranty. Keep backups of
your blyg.

### Does Burrow send anything anywhere else?

Only what a feature needs, when you use it:

- your blyg, with your owner token;
- other blygs' **public** files, without your token, when you open a profile
  or a quoted post you don't hold;
- GitHub, about once a day, to check for updates (`auto-update = off` stops
  it);
- the AI provider you switched on, only when you ask it to generate;
- the filter lists for the browser pane's content blocking, about once a
  week (`content-blocking = false` stops it);
- the sites behind links you open, images in posts, and YouTube (through
  `youtube-nocookie.com`) when you click a video.

Burrow has no analytics or telemetry.

### Why can't I delete a published post?

Blygger doesn't delete published work. **Post › Withdraw…** withdraws it: the
post stays listed as withdrawn, visibly, and that can't be undone. Drafts and
scratch notes can be deleted (⇧⌘⌫).

### Why can't I fork this post?

Forking descends only from **pinned** versions. Pick a pin in the post's
version pill (`‹ vN ▾ ›`, or press [), then **Fork this pin**. If the post has
no pins, it can't be forked.

### Why won't my quote go in?

Quotes (`![[id]]`) only go in threads; ⌘T makes a fragment a thread. A
partial quote's passage must be in the quoted post exactly as written; the
preview flags one that isn't, and publishing refuses it.

### Why doesn't Burrow show follower counts or likes?

Blygger shows responses as a list, never a count, and following is private.
Burrow follows the protocol's rules: no counts anywhere social.

### Can I sign in with my Claude Pro/Max subscription?

Not directly: Anthropic's terms don't allow third-party apps to use claude.ai
logins. Install Claude Code and switch on the local Claude Code provider
instead. See [AI helpers](ai.md).

### macOS says Burrow "can't be opened" or "Not Opened".

Releases aren't notarized yet. The one-line installer avoids this; for a
browser download, see
[Downloading from the Releases page](install.md#downloading-from-the-releases-page-in-a-browser).

### How do I report a bug?

**Burrow › About Burrow › Copy build info** copies the version, commit, update
status and what your server supports (no paths, no token). Paste it into a
[GitHub issue](https://github.com/aneeshsathe/blygger-desktop/issues).
