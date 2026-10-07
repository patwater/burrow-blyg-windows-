# Changelog

All notable changes to Burrow (called Blygger Desktop up to 0.6.0) are
documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/). Before 1.0, minor versions may
break things.

To cut a release: rename `[Unreleased]` to `[x.y.z] - YYYY-MM-DD`, bump
`version` in the root `Cargo.toml`, commit, and merge to `main`: the release
workflow sees the untagged version, tags the commit, and publishes that
section as the release notes. If the release
teaches something new in the tour, set the newest entry's `version` in
`crates/blyg-app/src/onboarding/whats_new.rs` to the release's.

## [Unreleased]

## [0.9.0] - 2026-10-06

### Changed

- **Burrow follows blygger-studio 0.32's owner API.** The tests check against 0.32.1's `openapi.json`, and the end-to-end tests run against a real local studio 0.32.1.
- **Sign in with browser.** On studio 0.28 or later, connecting opens your studio's sign-in in the browser and asks you to allow Burrow (OAuth). Burrow keeps the grant in your Keychain and renews it; after 30 days, or if you revoke it under Client access, it asks you to sign in again. An **API token** from the studio's Client access page works too, as does a server's own owner token; the password stays for older studios. Studio 0.28 signed every session out, so a password sign-in has to happen once more.
- **Reply is the one response action.** A stub starts quoting the whole post; in the stub, **quote a passage instead** shows the post and the passage you select becomes the quote, and further passages go in as their own quotes. "Reply with this" is gone from the selection pill; "Quote in draft" (⇧⌘D) stays, and into a stub of the same post it sets or adds a passage.
- **Cheaper, quicker sync.** On studio 0.32, Burrow asks the blyg what changed before reading anything, so a pull with nothing new is one small request, and it checks every 15 seconds.
- **AI disclosure works on any studio 0.28 or later.** Text generated in Burrow is recorded with the post through the studio's own API; the Worker extension for it is only a fallback now.
- **Generated text from your blyg's own AI** goes straight into the editor.
- **The preview matches studio 0.32.2** byte for byte again, including its sanitizer for threads. A TK scope's sources are the posts its instruction names, not ones its output adds, as studio 0.32.2 records them.
- **The stock-server notice is shorter.** On a current studio, only read state across Macs needs an extension; AI disclosure (0.28), removing deleted images (0.26) and replies and forks (0.18) are upstream now.

### Added

- **Mark read and unread, several at once.** In the Reader, ⌘-click, ⇧-click and ⌘A pick posts; **r** and **u** (or the right-click menu) mark them read or unread. Marks sync in batches; unread syncs on a server that supports clearing read state, and otherwise stays on this Mac.
- **Subscription names follow their source** (studio 0.30). **Rename…** gives one your own name, and **Use the Blyg's Own Name** hands it back. **Check Now** checks one; **⇧⌘R** (Blyg › Check All Feeds Now) has your blyg check them all.
- **Highlight generated text** (studio 0.27): set your blyg's default in its site settings and override it per post from the ⌘G palette (**H** cycles default, on, off). The preview shows it as the page will.
- **A full-text picker for `[[` and `![[`.** Every word of your published posts and the blyg posts you read, searched on your Mac, filtered by source and blyg, newest or oldest first. Typing `[[` now opens it for links too. The blyg's *picker typing* setting chooses whether you type in the editor or in the picker.

### Fixed

- **A change the server rate-limits is no longer dropped.** Studio 0.28's work budgets answer 429; Burrow waits as asked and sends it then. Typing saves at most every three seconds per post.
- **A change your token lacks the access for waits** instead of being dropped, and Burrow says which permission it needs.
- **Images in drafts show again** on studio 0.28 or later, which keeps unpublished uploads private: Burrow reads them as you, from your blyg only.

## [0.8.2] - 2026-10-04

### Fixed

- A long title no longer runs past the hexagon in the lineage view: it fits two lines, broken between words.

## [0.8.1] - 2026-10-04

### Fixed

- The "What's new" card and the lineage view's side panel wrap their text instead of running past the edge.
- The lineage glyph shows in Reader mode too: on each list row and in the open post's header line.

## [0.8.0] - 2026-10-04

### Changed

- **Burrow follows blygger-studio 0.26's owner API.** Nothing it used changed; the tests now check against 0.26's `openapi.json`.
- **Who a post replies to, or forks, comes with the post** from studio 0.18 or later (and from the reading rows of a server with Burrow's extensions that say `lineage: true`), so Burrow no longer fetches each post's own document for it. Older servers work as before.
- **One response per post.** A reply that also quotes the post it replies to is listed once, as a reply; a fork outranks both. A reply or quote of a passage says so ("stubbed a passage of this").
- **Your own posts count as responses** to the posts they quote, reply to or fork.

### Added

- **Lineage.** A glyph beside each post's name in the stream shows which kinds of post it draws on and which draw on it (fork, reply, quote), never how many; it replaces the "↩" marker. **⌘J** (Post › Lineage…) opens the post's lineage: what it draws on above, its responses below, walkable one step at a time with ⏎ and ⌫.
- **Actions on a ring.** Space on the post in the middle of the lineage view opens fork, reply, quote, link post, versions and open in fixed places (the web Studio's layout). A letter previews what the action would make and what it means for the author; ⏎ does it.
- **What's new.** The first launch after an update opens the tutorial on what changed, with the choice to start at the new parts, take the whole tour, or skip it. The tour teaches lineage and the ring.

## [0.7.0] - 2026-10-02

### Changed

- **Burrow follows blygger-studio's documented owner API (studio 0.9 to 0.11).** Upstream redesigned `/api` as an OpenAPI contract and dropped the old routes, so saves, forks, pins, response settings, subscription pause and resume, mention hiding and site settings now use the new routes (`PATCH`, `POST /api/items {mode}`, `PUT …/versions/:v/pin`), and lists are read page by page. Burrow now needs studio 0.9 or later; 0.11 is current.
- The reading list loads from `/api/reading/imported`; upstream's `/api/reading` is now a different resource. Permalinks follow your blyg's site URL when it has one.
- **Sign in with your studio password, so any blygger-studio works.** The Connect sheet takes the password you use in your blyg's studio (or, as before, the owner token). The password is kept in your Keychain; Burrow signs in again by itself when the 30-day session ends.
- **A stock server is fully usable.** Without Burrow's server extensions, the reading list is built from upstream's own routes. The first time Burrow meets such a server it explains what's limited (AI disclosure for text generated in Burrow, read state across Macs, removing deleted images), and the About window lists it too.
- **Who a post replies to, or forks, shows in the reader** for posts from other blygs: Burrow fetches it from the post's own blyg, a few posts per sync.
- **An outdated server gets a clear message, not damage.** If your blyg's server is older than studio 0.9, Burrow says so when it starts, keeps a status-bar notice with a **How to update** link, and pushes nothing until the server is updated. Your edits wait and go up afterwards. The connect sheet names the problem too.
- Changing a draft between fragment and thread keeps the same draft on the server instead of making a new one.
- The tests check every request Burrow sends, and every reply they fake, against upstream's `openapi.json`, and the routes upstream lacks against a contract of their own.

### Fixed

- A blyg that lives under a path (`https://example.com/blyg`) now connects: its API is at the host root, which Burrow used to miss.
- A quoted post from another blyg shows its images in your draft's preview. Posts published before studio 0.11 use relative image paths, which used to resolve against your own blyg.
- `docs/SERVER.md` now lists only what upstream still lacks: bearer-token sign-in, the reader's reading rows, client-recorded AI provenance, read-state sync and attachment removal.

## [0.6.2] - 2026-09-30

### Fixed

- **Themes now carry through the whole app, and text reads everywhere.**
  - Konkan's subscriptions column showed dark text on dark. So did its selected rows in the Reader and the stream, Kumiko's title bar, and several menus, popovers and status-bar notices on the darker themes.
  - Every surface now measures its text against its own background. A new test checks each built-in theme.
- The Reader's subscriptions column draws the theme's sidebar ornaments. The theme's selected-row style, quote frames, markers and chip and sheet shapes now apply in the Reader, the stream, Mentions, Subscriptions and profiles, as they do in the posts list.
- The reading pane shows posts in the theme's writing font (e.g. ET Book in Portolan), with quote boxes, highlights and diff marks in the theme's colours.
- Sheets, cards, popovers, text fields, tooltips and chips take the theme's corners, edges and fonts. Portolan's sheets have a true double rule.
- Aizome's selection in the Reader's list and the stream was invisible, because the selected colour was the same as the page's.
- Cutaway's title bar is a deeper green, so its labels read.

## [0.6.1] - 2026-09-29

### Added

- **Themes.** Eight new themes join Paper and Ink, and they go beyond colour with ornament. The woody ones are **Cutaway** (a lit burrow room in a soil cross-section), **Kumiko** (Kyoto lattice and kintsugi), **Shola** (a mossy Western Ghats thicket) and **Fortress** (Dwarf Fortress stone and glyphs). The oceanic ones are **Portolan** (a chart with rhumb lines and soundings), **Aizome** (indigo, seigaiha waves, sashiko), **Saltspace** (weathered greys and tide lines) and **Konkan harbour** (a lit harbour in a night sea, with a tide sparkline of your writing). Pick one in Settings or with `theme = <name>`, and use `theme-dark` for another when macOS is dark. Themes are plain files: `blygger +copy-theme <name>` copies one into `~/.config/blygger/themes/` to edit, and it reloads when you save. Ornament slots can use your own SVG. `blygger +list-themes` lists them. The studio preview still shows your blyg's own style.
- The config file now reloads when it's edited outside the app (it used to reload only on ⌘⇧,).
- A documentation site at https://aneeshsathe.github.io/blygger-desktop/, and a shorter README.

### Changed

- **Blygger is now Burrow.** Burrow is a blygger client. The app, its menus (Burrow › Settings…, Burrow › Check for Updates…, Help › Burrow Tutorial), the About window, the window title and the app's messages say Burrow, and new installs are `Burrow.app`. Release assets are named `Burrow-<version>-macos-universal.{zip,dmg}`.
- Only the name changed. The bundle ID (`org.blygger.desktop`), your data folder and database, the Keychain items, the config file (`~/.config/blygger/config`), the `BLYGGER_*` environment variables, the `blygger` command and the key-binding contexts are all the same, so nothing needs migrating.
- **Updating from 0.6.0 or earlier:** the in-app update works as usual. The app replaces itself where it is, so it stays `Blygger.app` on disk (Finder and Spotlight show that name) while being Burrow inside. That's expected. To get `Burrow.app`, run the one-line installer, which replaces a `Blygger.app` in the same folder (posts, settings and sign-ins carry over), or rename the app while it isn't running. A Dock shortcut to the old name may need re-adding.
- The updater accepts a release zip named `Burrow-…` or `Blygger-…`, holding `Burrow.app` or `Blygger.app`, and prefers Burrow's. For a transition period, each release also publishes `Blygger-<version>-macos-universal.zip` and `Blygger-macos-universal.zip` (the same signed app in a folder called `Blygger.app`) so 0.6.0 and earlier can still update. The signed `SHA256SUMS` covers them too.

### Fixed

- Selecting text with the mouse in the reading pane and the preview works again: the selection stays until you quote it (⇧⌘D, or the new **Quote in draft** pill beside it), and dragging inside a quote no longer opens the original. ⌘C copies a selection in the reading pane. In the browser, **→ Draft** and **→ Notes** keep the page's selection, and the button reads **❝ Quote → Draft** while text is selected. In the stream, dragging over or double-clicking a post opens it where its text can be selected. The notes drawer's footer says when ⇧⌘D will quote your selection.

## [0.6.0] - 2026-09-29

### Added

- **Universal quoting.** Highlight text in the built-in browser, in notes or in the reading pane, then press ⇧⌘D (or **→ Draft** in the browser, or Post › Quote Selection in Draft). The passage goes into the open draft at the caret, or into a new draft, with its source. A post from a blyg you follow, quoted into a thread, becomes a partial quote of that post. Anything else becomes a blockquote followed by a link to the page.
- **Partial quotes (blygger-studio 0.8).** `![[id]]` followed directly by `>` lines quotes just that passage. A blank line in between makes it a whole quote plus your own blockquote. The preview flags a passage that isn't in the quoted post, and publishing refuses it. In the stream, a partial quote shows only its passage.
- Replying with a passage selected in the reading pane starts the reply as a partial quote of it. Replying to a long post with nothing selected leaves an empty quote line to fill in.
- **Link post:** a new action next to Fork that starts a fragment linking to the post (`[[id]]`) without responding to it. Reply is still the one way to respond.
- **Responses:** each post on the Mentions screen can follow the blyg's site setting, or show or hide its responses itself.
- **Site settings:** a time zone for dates on your pages, and **Show responses by default**, on blygs that report them.

### Changed

- The preview matches blygger-studio 0.8.3.
- Titles never come from quoted text. A reply with nothing of its own yet shows as "In response to `<host>`".

## [0.5.1] - 2026-09-28

### Fixed

- The one-line installer (`scripts/install.sh`) failed on stock macOS with
  `ASSET: unbound variable`. `/bin/bash` 3.2 in a UTF-8 locale reads the first
  byte of the `…` after `$ASSET` as part of the variable name. Braces fix it.

## [0.5.0] - 2026-09-28

### Added

- **`[[id]]` links (protocol 0.3).** An inline `[[id]]` links to one of your posts or one you read, without quoting it. The preview shows it as the published page will: the target's first words in quotes, linked. Links inside code stay as text. A link that can't be resolved is flagged in the preview, the status bar and the publish sheet, as quotes are; publishing refuses it. In the stream, a link reads as the post it points to and opens it.
- Quote boxes in the stream show the citation the quoting blyg recorded (`cited`) when the quoted post isn't held on this Mac.
- Site settings: an **Accept mentions** option, on blygs that report it.

### Changed

- The preview matches the reference client at blygger-studio v0.7.0 (protocol 0.3). The publish error for unresolvable quotes or links is now "one or more references do not resolve".

## [0.4.1] - 2026-09-27

### Changed

- When an update has downloaded and passed its checks, a sheet asks **Restart Now** or **Later**; in `auto-update = notify` mode it offers **Download and Install**. Check for Updates… asks again, and the status bar notice stays after Later.
- Tour: steps that open a pane, popup or mode (the full editor, versions, @ mentions, the stream pane, the original, Reader, notes) show ✓ and wait for Next instead of moving on by themselves. You can now pick someone from the @ popup.

### Fixed

- Opening the profile of some blygs crashed the app (a feed with a multi-byte character just after an `&…;` entity). Profile and original-post fetches now show an error instead of crashing if a page can't be read.
- Right-clicking a misspelled word crashed the app.

## [0.4.0] - 2026-09-27

### Added

- **The stream is the new default reading view.** Reading now opens as one
  scrolling timeline, newest first, drawn natively without a web view per
  post. Fragments show in full; threads show their title, a few lines and
  "Read more". Quotes appear as grey boxes with the quoted text; images and
  videos are small placeholders. Use j/k to move, ⏎ or Space to open the
  whole post in a side pane, and esc to close it; the stream keeps your
  place. A post counts as read once it has been on screen for a second, and
  unread posts get a dot. The Stream | Reader toggle (⌥⌘1 / ⌥⌘2) brings back
  the list-and-post layout and is remembered. Search (⌘F or /) filters the
  stream too.
- **Quotes open the post they quote.** Clicking a quote's text, in the stream
  or the reader, opens the original post beside it, at the quoted version
  when that version is pinned. The footer reads "quoted from `<name>` · v2 ·
  open original", and the name still opens the profile. "↳ stub of …" and
  "⑂ forked from …" work the same way. A post you don't follow is fetched
  from its author's public files, without your token, with a Subscribe
  button.
- **See who responded to a post.** The bottom of the reading pane lists who
  quoted, stubbed or forked the post you're reading, and when, from the posts
  already in your reading list. Your own posts list their verified mentions
  the same way. It's a list, never a count; the stream marks posts that have
  responses with a small ↩.
- **Reader: three panes, smart feeds and folders.** Reader mode (⌥⌘2) is
  laid out like NetNewsWire: sources on the left (smart feeds All unread,
  Today, Thumbed and All, then your folders, then subscriptions in no
  folder), the post list for the source you pick in the middle (⌘F or /
  searches within it), and the post on the right. Folders live on this Mac
  only and are never sent to your blyg: make one with Blyg › New Folder…,
  file a subscription by dragging it onto a folder, with "Move to folder ›",
  or on the Subscriptions screen; rename, reorder and delete them. ←/→ move
  between panes, j/k between posts, Space scrolls the post and then opens
  the next unread one, and ⌥⌘S hides the sources pane (drag its edge to
  resize it). Version stepping in the Reader moved to [ / ].
- **Notes drawer** (⇧⌘N, View › Notes): a scratchpad that slides in over the
  right edge while you read the Stream, the Reader, the browser pane or your
  Posts, and slides away with esc, ⇧⌘N or a click outside. Your notes are a
  local scratch note, "Reading notes", saved as you type and listed in
  Posts; ⌘D makes them a draft and ⋯ › New notes page starts a fresh page.
  **→ Notes** on a post adds it (`![[…]]` for a blyg post, a link for a feed
  post), the browser pane's → Notes adds the page's link, and text selected
  in a post or web page goes in as a quote with its source.
- **In-app browser.** Links in posts open in a browser pane that slides in
  from the right over the reading view: ⌘-click opens it over the whole
  reading area, ⌥-click opens your default browser, esc closes it, and ⇧⌘B
  brings back the last page. It has back, forward and reload, an address
  field that shows the page title, a progress bar, "Open in default
  browser", and "→ Notes", which adds the page's link to your notes. The
  pane has its own cookies (kept across launches on macOS 14 and later) and no bridge
  into the app, so pages never see your token. `open-links = browser`
  restores the old behaviour.
- **Ad and tracker blocking** in the browser pane, with uBlock Origin's
  default filter lists (uBlock filters, EasyList, EasyPrivacy and Peter
  Lowe's list) run as WebKit content blockers. The lists are downloaded about
  once a week and compiled in the background; a small built-in list covers
  you until the first download. 🛡 turns blocking off for one site, and
  `content-blocking = false` turns it off everywhere. Scriptlet filters
  can't run in WebKit content blockers and are skipped.
- **Spellcheck.** Misspelled words get a red wavy underline, using the macOS
  spell checker with your system languages and the words you've taught it.
  Right-click (or ctrl-click) one for suggestions, Learn Spelling and Ignore.
  Code, links, addresses, quotes (`![[…]]`) and TK markers are never checked.
  Checking happens in the background after you pause, only on the lines you
  changed, so typing stays instant. Turn it off with Edit › Spelling › Check
  Spelling While Typing, or `spellcheck = false`.
- **@-mentions.** Type `@` at the start of a word, in the editor or quick
  capture, to pick from the blygs you know: subscriptions, blogroll, authors
  in your reading, and profiles you've opened. ↑/↓ choose, ⏎ or ⇥ insert,
  esc keeps what you typed. Blyg has no handles, so a mention is a plain link
  to the blyg, `[Name](https://…/)`, and it notifies no one.
- **About Blygger** (Blygger menu and Help): version, commit, build date,
  architecture and macOS version; the auto-update setting, last check and
  whether an update is ready, with Check for Updates…; the blyg you're
  connected to (host only) and what its server supports; the data folder and
  config file with Reveal in Finder; links to the repository, this version's
  release notes and the license. **Copy build info** copies a summary for bug
  reports, without your token or file paths.
- **The tutorial covers the new reading features:** the Stream, quotes that
  open the original, who responded, Reader with sources and folders, the
  Notes drawer, links in the browser pane (a blank sample; nothing loads),
  and @-mentions and spellcheck while writing. Its highlight steps aside for
  panes and popups, and the tour puts your reading mode, notes and browser
  page back when it ends.

### Fixed

- **Reading shows each post's own date** (not when your blyg first imported
  it), so a new subscription no longer makes every post look like it arrived
  today. The list is sorted by when the author published or last edited a
  post; edited posts say "edited 3d ago" and still move to the top.

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
