# Getting started

## First launch

The first launch opens a short setup: a welcome, **Connect your blyg** (or
*Skip, just try it with sample data*), **AI** (off until you switch a provider
on; installed `claude` / `codex` CLIs are detected), and **Buttons or
keyboard?** (`show-buttons`). esc skips it at any step.

Then comes an optional **interactive tour** of the real window: each step
highlights a part of it and waits for you to press the key (⌘T, ⌘⏎, ⌘3, ⌘G,
⌘Y, ⌘R, ⌘K…). The tour runs on sample data, so your own blyg isn't touched,
and your posts come back when it ends. Replay it from **Help › Burrow
Tutorial** or **Settings (⌘,) › Help**, or set `tutorial-on-launch = true` to
see it every time.

**After an update**, the first launch opens the tour on a **What's new**
card: ⏎ starts at the steps that teach the new things, **Take the whole tour**
starts at the beginning, and esc skips it.

![What's new after an update](screenshots/whats-new.png)

| | |
|---|---|
| ![Onboarding](screenshots/onboarding.png) | ![The tour](screenshots/tutorial.png) |

## Connecting your blyg

Burrow works with any blyg running blygger-studio 0.9 or later (0.32 is current).
A server with the extensions in [Server requirements](server.md) gets a few
more features; without them, Burrow tells you what's limited. No blyg yet? Try
the app on the sample data.

The Connect step asks for your blyg's address (such as
`https://blyg.example.com`, including any path it lives under) and how to sign
in:

- **Sign in with browser** (the default, studio 0.28 or later): your browser
  opens your studio's sign-in and asks you to allow Burrow. Burrow keeps the
  grant in your Keychain and renews it in the background; after 30 days, or
  if you revoke it in the studio (**More › Client access**), it asks you to
  sign in again.
- **API token**: make one in your studio under **More › Client access**
  (REST API, with all four permissions) and paste it. A server with Burrow's
  extensions also takes its `BLYG_OWNER_TOKEN` here.
- **Studio password**, for a studio older than 0.28.

It checks them with the server before saving anything, and says plainly what's
wrong:

- the address can't be reached;
- the password or the token is wrong, or the token lacks a permission (it
  names which);
- there's no studio sign-in at that address;
- the server is older than blygger-studio 0.9.

The sign-in, token or password then goes in your Keychain and the address into the
config file as `blyg-url`, and the app loads your posts. If the server is
older than 0.9, Burrow says so when it starts and pushes nothing until it's
updated. **Burrow › Disconnect…** forgets
both, and can also delete the local copy.

| | |
|---|---|
| ![Connected to a blyg](screenshots/live-list.png) | ![Checking the token](screenshots/connect-check.png) |

## The window

There's one window. At the top is the **omnibar**: type to filter your posts,
↑/↓ to preview each one, ⏎ to open it, and ⏎ with no match to start a new
draft with what you typed. esc goes back to the omnibar. The **status bar**
at the bottom shows the post's kind (`◦ fragment` or `≡ thread`, click to
switch), the character count, the sync state (`synced`, `saved on this Mac`,
`syncing…`, `offline · N changes waiting`) and the version (`draft`,
`public v3`, `public v3 · unpublished edits`).

A quiet **toolbar** in the title bar has New, Make draft, Publish, the three
views (Write, Preview, Full editor), Versions, Generate, Delete or Withdraw,
and Quick capture. Every tooltip shows the shortcut, so the buttons teach the
keys, and a greyed-out button says why ("Too long for a fragment. ⌘T makes it
a thread"). Narrow windows get icons only. `show-buttons = false` (or
Settings, ⌘,) gives the keyboard-only window.

| | |
|---|---|
| ![The toolbar](screenshots/toolbar.png) | ![The toolbar in a narrow window](screenshots/toolbar-narrow.png) |

⌘R switches to [reading](reading.md), and ⌘R again comes back to your posts.
Every shortcut is listed in [Keyboard shortcuts](keys.md).

## Settings

Settings (⌘,) covers the common options: the writing and interface fonts and
size, theme, layout, the quick-capture hotkey, buttons, AI accounts, and the
tutorial (Help). Everything is also in the plain-text
[config file](config.md), which Settings writes back to without touching your
comments.

Burrow comes with a Tufte-style light and dark look that follows macOS (see
[Themes](themes.md)), and switchable writing and interface fonts: Literata,
Inter, Source Serif 4, iA Writer Quattro and ET Book are bundled, and system
fonts such as New York, Charter, SF Pro and Menlo work too
(`blygger +list-fonts` lists them). ⌘+ and ⌘− change the text size.

![Settings](screenshots/settings.png)

## Offline, and conflicts

Nothing waits on the network. Every keystroke is saved on your Mac first;
edits sync shortly after you stop typing, and queue up while you're offline.
The status bar shows how many changes are waiting.

When the server changed under you (say, you edited the same post in the web
studio), a side-by-side sheet shows **On this Mac** and **On the server**:
press 1 to keep yours, 2 to take the server's, or 3 to keep both.

| | |
|---|---|
| ![Offline](screenshots/offline.png) | ![A conflict](screenshots/conflict.png) |
