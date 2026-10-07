# Configuration

Settings live in a plain-text file in the style of Ghostty's config:
`~/.config/blygger/config`, with one `key = value` per line. Every option is
optional. Most options can also be changed in Settings (⌘,), which writes
them back to the file and keeps your comments. **Burrow › Open Config File**
opens it, and ⇧⌘, reloads it after you edit it by hand.

**Secrets never go in this file.** The blyg owner token and AI API keys are
stored in the macOS Keychain, and the app never logs them.

## The file format

```text
# a comment
key = value
key = "a quoted value, with  spaces kept"
repeatable-key = one
repeatable-key = two
config-file = ?optional/extra.config
```

- `#` starts a comment only at the beginning of a line, so `#fff` in a value
  is fine.
- Values may be double-quoted; inside quotes, `\"`, `\\`, `\n` and `\t` are
  escapes.
- An empty value (`key =`) resets a key to its default, and a repeatable key
  to an empty list.
- Unknown keys are warnings; malformed lines and bad values are errors.
  Neither stops the rest of the file from loading.

Files are loaded in this order, later ones overriding earlier ones:

1. `$XDG_CONFIG_HOME/blygger/config` (by default `~/.config/blygger/config`);
2. `~/Library/Application Support/org.blygger.desktop/config`;
3. any file named by `config-file`.

`BLYGGER_CONFIG=<file>` replaces both of the first two, for testing.

## From the command line

The app's executable doubles as a command-line tool, Ghostty-style. From a
standard install, it's `/Applications/Burrow.app/Contents/MacOS/blygger`.

```sh
/Applications/Burrow.app/Contents/MacOS/blygger +show-config --default --docs
```

| Command | What it prints |
|---|---|
| `+show-config` | the effective config (only what you changed) |
| `+show-config --default --docs` | every option with its default and documentation |
| `+show-config --changes-only=false` | every option, not just the ones you set |
| `+validate-config` | problems in the config file(s) |
| `+list-fonts` | fonts for `font-family-writing` / `font-family-ui` |
| `+list-keybinds` | every keyboard shortcut, and the ones reserved |
| `+version` | the version |
| `+help` | this list |

## Every option

This list is generated from the app's own key table
(`crates/blyg-core/src/config/keys.rs`), the same one `+show-config --default
--docs` prints, so it matches the version of Burrow this site was built from.

{{#include generated/config-keys.md}}
