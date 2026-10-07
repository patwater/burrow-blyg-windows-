# Keyboard shortcuts

Burrow is keyboard-first. Every toolbar button's tooltip shows its key, and
`blygger +list-keybinds` prints this list from the app itself.

## The main table

This part is generated from the app's key table
(`crates/blyg-app/src/keymap.rs`), the one that binds the keys and builds the
menus and toolbar buttons.

{{#include generated/keybindings.md}}

## Keys in context

These belong to one screen or sheet, so they aren't in the table above.

**The omnibar and the editor**

| Key | What it does |
|---|---|
| ↑ / ↓ | Move through the posts, previewing each one |
| ⏎ | Open the selected post, or start a draft when nothing matches |
| esc | Clear the search; from the editor, back to the omnibar |
| ⌃⌥B | Quick capture, from any app (`capture-hotkey`) |
| ⌘K, or `![[` at the start of a line | Quote a post (threads only) |
| `[[` | Link a post |
| @ | Mention a blyg |

**Quick capture and the notes drawer**

| Key | What it does |
|---|---|
| esc, ⌘S | Keep it (a scratch note, or a draft with `capture-default = draft`) |
| ⌘D | Make it a draft on your blyg |
| ⌘⏎ | Publish it (from the notes drawer: open it in the editor to publish) |

**Reading**

| Key | What it does |
|---|---|
| j / k, ↑ / ↓ | Next / previous post |
| ⏎, Space | Open the post (stream); Space pages through it, then the next unread one (Reader) |
| ⌘-click, ⇧-click, ⌘A | Pick posts in the Reader's list (esc lets go) |
| r / u | Mark the picked posts, or the open one, read / unread |
| ← / → | Move between the Reader's panes |
| [ / ] | Step through a post's versions (current and pinned) |
| ⌘F, / | Search what you're reading |
| esc | Close the post or the search |

**Sheets**

| Key | What it does |
|---|---|
| ⏎ | Confirm (publish, delete, accept) |
| esc | Cancel or close |
| 1 / 2 / 3 | In a conflict: keep mine, take the server's, keep both |
| ⇥, F, ← | In a profile: switch tabs, follow the selected entry, go back |
