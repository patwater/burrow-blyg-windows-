# Burrow

**Burrow is a blygger client.**

It's a native macOS studio for [Blygger](https://blygger.org) blogs, built to
be as fast as Notational Velocity. Burrow was called Blygger Desktop up to
0.6.0 (see [the rename](install.md#the-rename)).

![Burrow, light theme](screenshots/light.png)

## What's a blyg?

[Blygger](https://blygger.org) is an open protocol for personal blogs
that you own. A blog that speaks it is a **blyg**: your own site, on your
own domain, holding short **fragments** (up to 1000 characters) and longer
**threads**. Posts have public versions. A version can be **pinned**, which
means it's served unchanged forever, and published posts are never deleted,
only **withdrawn**, visibly. Blygs quote each other by id (`![[id]]`), reply
with **stubs**, **fork** each other's pinned posts, and follow each other
privately. There are no follower counts, likes or handles: the domain is the
name, and responses are shown as a list, never a count.

The reference server is a Cloudflare Worker with a web studio. Burrow is a
second studio, on your Mac.

## What Burrow is

There's one window: type to search, press ⏎ to create, and nothing ever waits
on the network. It's local-first: everything you write is saved on your Mac
first and synced to your blyg in the background, so it works offline. It's
written in Rust with [GPUI](https://www.gpui.rs), the UI framework behind Zed.

- **Write** fragments and threads in plain Markdown, with a live preview that
  matches what your blyg publishes. There's no save button. See
  [Writing and publishing](writing.md).
- **Read** everything you follow as one stream, or in a three-pane reader,
  and quote, reply, fork or link from any post. See
  [Reading, quoting and responses](reading.md).
- **Collect** as you go: a global quick-capture panel, a notes drawer for
  reading notes, and a built-in browser with ad blocking. See
  [Notes and the browser](notes-and-browser.md).
- **Get help from AI**, if you want it. It's off until you switch it on, you
  bring your own accounts, and generated text is always disclosed. See
  [AI helpers](ai.md).
- **Configure** it in one plain-text file, Ghostty-style. See
  [Configuration](config.md).

Burrow needs a blyg with a few owner-API extensions that aren't in upstream
Blygger yet (see [Server requirements](server.md)). You can try it without
one: the first launch offers sample data.

## Disclaimer

> **Burrow is entirely vibecoded:** it was written with AI assistance.
> It's provided **as is, with no warranty or guarantee of any kind**, and you
> use it **at your own risk**. That includes the risk of losing or corrupting
> posts on your blyg. Keep backups.

The source code is under the MIT license, and the documentation, mockups and
screenshots under CC BY 4.0. See the
[repository](https://github.com/aneeshsathe/blygger-desktop#license) for the
details.

## A tour in screenshots

| | |
|---|---|
| ![The stream](screenshots/stream.png) | ![The three-pane Reader with folders](screenshots/reader-three-pane.png) |
| ![A post's lineage (⌘J)](screenshots/lineage.png) | ![The action ring](screenshots/lineage-ring.png) |
| ![The notes drawer over the stream](screenshots/notes-drawer.png) | ![Spelling suggestions](screenshots/spellcheck.png) |
| ![Picking a blyg after @](screenshots/mention-picker.png) | ![Dark theme](screenshots/dark.png) |
| ![Quick capture](screenshots/quick-capture.png) | ![Publish sheet](screenshots/publish-sheet.png) |
| ![Conflict resolution](screenshots/conflict.png) | ![Toolbar, with a disabled button's reason](screenshots/toolbar.png) |
| ![Quick capture's button row](screenshots/quick-capture-buttons.png) | ![Mentions](screenshots/mentions.png) |
| ![A profile, opened from a reading item](screenshots/profile.png) | ![Your own profile, with blogroll toggles](screenshots/profile-own.png) |
