# Changelog

All notable changes to Blygger Desktop are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/). Before 1.0, minor versions may
break things.

To cut a release: rename `[Unreleased]` to `[x.y.z] - YYYY-MM-DD`, bump
`version` in the root `Cargo.toml`, commit, and push a `vx.y.z` tag. The
release workflow publishes that section as the release notes.

## [0.3.0] - 2026-09-25

### Added

- **Automatic, signed updates.** Blygger checks GitHub for a new release at
  launch and about once a day, downloads it in the background, and shows
  "Blygger X is ready · Restart to update" in the status bar (quitting
  installs it too). **Blygger › Check for Updates…** checks right away. An
  update is installed only if its `SHA256SUMS` has a valid Ed25519 signature
  from the project's release key, the zip matches it, and the new app is
  `org.blygger.desktop`, newer, and passes `codesign --verify`; otherwise
  nothing changes. `auto-update = install | notify | off` (default install)
  in the config. Releases now include `SHA256SUMS.sig`, and the one-line
  installer checks it when OpenSSL 3 is installed. People on 0.2.0 or
  earlier need to run the installer once more.
- **Read state syncs between your Macs.** A post you read on one Mac now reads
  as read on your others, and a fresh install no longer shows everything
  unread. It syncs through your own blyg when the server supports it (the
  optional owner-API extension 5, see `docs/SERVER.md`). Marking a post read
  is still instant and works offline; the change is sent when you're back
  online, and read state never goes backwards. The first sync uploads what
  you've already read on this Mac. On a server without the extension,
  nothing changes.

## [0.2.0] - 2026-09-25

### Added

- **Typing `![[` opens the quote picker** (#5). At the start of a line in a
  thread, `![[` opens the same picker as ⌘K, and what you type next filters
  it. ⏎ inserts the `![[id]]` line; esc puts the `![[` back. Pasting, fenced
  code and fragments don't trigger it (a fragment shows the threads hint).
- **Delete a draft** (#2). ⇧⌘⌫, Post › Delete Draft…, or the toolbar's
  Delete button deletes the current draft or scratch note after a short
  confirmation (⏎ deletes, esc cancels). The list moves on to the next post.
- **Withdraw a published post.** Post › Withdraw…, or the same toolbar slot
  on a published post, asks first, takes an optional note, and withdraws it.
  Withdrawn posts stay listed as withdrawn, and it can't be undone. Published
  work is never deleted, and ⇧⌘⌫ on a published post says so.
- **Search the reading list** (#6). ⌘F or `/` puts the caret in a search field
  above the reading list; typing filters it by title, author or blyg name, and
  text, case-insensitively, over posts already held on this Mac (nothing is
  fetched to search). Matches are highlighted, ↑/↓ and ⏎ work from the field,
  esc clears the search, and "No posts match “…”" says when nothing does.
- **Paste a link over selected text**, as in WordPress. Select some words
  and paste a web or mail address: they become `[words](address)`. Pasting
  anything else, or over a selected address or link, pastes as usual.

### Changed

- **Reading actions say what they create** (#3). The actions under a post now
  read "Quote into a thread" (adds `![[id]]` to a thread of yours), "Reply ·
  new stub" (a stub thread, `stub_of`) and "AI reply · new stub", each with a
  one-sentence tooltip. Fork shows on the current version too, greyed out,
  explaining that forks descend from pins only and pointing to the 📌 in
  ‹ vN ▾ › when the post has one.

### Fixed

- **Typing stopped working after the window sat in the background** (#4).
  A preview could keep the keyboard, or drop it to nowhere, so only menu
  shortcuts like paste worked. The app now takes the keyboard back when the
  window becomes active again and before hiding a preview.
- **The tour's Quotes step ringed the editor over the quote picker** (#1).
  A step's ring now hides while a sheet or picker covers the panes.

## [0.1.0] - 2026-09-25

The first release: a local-first writing studio for a Blygger blog.

### Added

- **One window, Notational Velocity style.** An always-there omnibar filters
  your posts as you type, with matches highlighted. ↑/↓ preview each post, ⏎
  opens it, and ⏎ with no match starts a draft seeded with the query.
- **Side-by-side list and editor.** Plain Markdown with no toolbar. Every
  keystroke saves locally, and changes sync to your blyg about 800 ms after you
  stop typing. There is no save button.
- **Fragments and threads.** ⌘T toggles the kind. The status bar shows a live
  1000-character counter for fragments (amber past 900, red past 1000) and
  says how to fix a fragment that's too long.
- **Publish with an optional version note** (⌘⏎), from a sheet under the
  title bar, with a toast that links to the published post (⌘O opens it).
- **Offline first.** Nothing waits on the network. Edits queue while you're
  offline and flush when you're back, and the status bar shows how many are
  waiting.
- **Conflict resolution.** When the server changed under you, a side-by-side
  sheet shows both versions: keep mine, take the server's, or keep both.
- **Images.** Paste or drop an image to upload it and insert its Markdown
  link.
- **Quick capture into scratch notes.** A global hotkey (⌃⌥B by default,
  configurable) opens a floating panel over any app. esc keeps a *scratch
  note* that lives only on your Mac (images included); ⌘D makes it a draft on
  your blyg and ⌘⏎ publishes it. Scratch notes are searchable and editable in
  the main list.
- **Full editor with a faithful preview.** ⌘1 write, ⌘2 list + editor +
  preview, ⌘3 editor + preview (⌘E toggles). The preview is a Rust port of the
  reference Worker's renderer (tested byte-for-byte against it): quotes of
  other posts, the AI-text tint, YouTube and remote images, in your blyg's own
  stylesheet.
- **Reading.** Your reading list with one entry per post however often it's
  edited; edited posts move to the top with the author's notes. Posts show as
  their blyg published them (sanitized). Subscriptions, mentions (a list,
  never a count), thumbs, and your blyg's site settings.
- **Versions and pins.** For other people's posts the `‹ vN ▾ ›` pill steps
  through only the current and pinned versions. ⌘Y shows your own full
  history with restore, and pinning asks you to confirm in words.
- **Quote, reply and fork.** ⌘K quotes a post you hold into a thread; Reply
  makes a stub; forks descend from pins only.
- **Profiles.** ⌘I (or click an author, a quote, or the stub/fork line) shows
  who someone is: bio, links, their blogroll, recent posts and the blygs they
  quote, stub or fork. Follow in one click; add to your blogroll separately.
  Fetched from public files only when you open one, never with your token.
- **Optional toolbar buttons** (`show-buttons`), generated from the same table
  as the shortcuts and menus, so every tooltip teaches the key.
- **First-run onboarding and a replayable tutorial** that runs on sample data.
- **Tufte-inspired theme** that follows the system's light or dark mode.
- **Switchable fonts** for writing and for the UI. Literata, Inter, Source
  Serif 4, iA Writer Quattro and ET Book are bundled, and system fonts are
  also offered. ⌘+ and ⌘− change the size.
- **AI helpers (TK)** that work with your own provider accounts: Anthropic or
  OpenAI API keys, Cloudflare Workers AI, a local Claude Code or Codex
  install, your blyg server, or Sign in with ChatGPT (unofficial). ⌘G fills a
  `[TK]` gap, ⇧⌘G shortens to fit, and a palette offers continue, outline,
  proofread and an AI reply. Generated text is always disclosed as generated.
- Keychain storage for the owner token and API keys. Secrets never go in a
  file.
- A universal macOS app (Apple silicon and Intel), shipped as a dmg and a
  zip with SHA-256 checksums.
