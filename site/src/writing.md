# Writing and publishing

## Search is the interface

The omnibar filters your posts as you type. ↑/↓ previews each post, ⏎ opens
it, and ⏎ with no match starts a new draft seeded with what you typed. ⌘L
comes back to the omnibar from anywhere, and so does esc from the editor.

Posts are **plain Markdown**, and there's no save button. Every keystroke is
saved on your Mac and synced shortly after you stop typing. A post's title is
its first line of text (a leading heading is the title), never a quote it
starts with.

## Fragments and threads

A blyg holds two kinds of post:

- a **fragment** is short: up to 1000 characters. A live counter in the
  status bar turns amber past 900 and red past 1000;
- a **thread** is long, with no limit, and can quote other posts.

⌘T switches a post between them (or click `◦ fragment` / `≡ thread` in the
status bar). An over-long fragment can't be published: the status bar says
"Too long for a fragment · ⌘T makes it a thread". The AI helper **Shorten to
fit 1000** (⇧⌘G) is the other way out; see [AI helpers](ai.md).

![A fragment over the limit](screenshots/over-limit.png)

## Publishing and versions

⌘⏎ publishes. A sheet drops from the title bar, "Publish … as v2", with an
optional **version note**; ⏎ publishes and esc cancels. A toast shows the
result and its address, and ⌘O opens the post on the web.

| | |
|---|---|
| ![The publish sheet](screenshots/publish-sheet.png) | ![Published](screenshots/published-toast.png) |

Every publish makes a new public version (v1, v2, …); editing a published
post leaves it `public v3 · unpublished edits` until you publish again.
**Versions** (⌘Y) lists every version of your post with its note. Open any of
them and **Restore** it into the editor; publishing it makes a new version,
since versions never go backwards.

A version can be **pinned**: a pin is served unchanged forever, and it's what
other blygs can fork. Pinning can't be undone, so it asks you to type to
confirm.

![Versions of your own post](screenshots/versions-own.png)

### Delete and withdraw

Drafts and scratch notes can be deleted: ⇧⌘⌫ (Post › Delete Draft…) asks
first. **Published work is never deleted.** Post › Withdraw… withdraws a
published post instead, with an optional note. Withdrawn posts stay listed as
withdrawn, and you can't undo it, so Withdraw has no key on purpose.

## Quick capture and scratch notes

A global hotkey (⌃⌥B by default, `capture-hotkey`) opens a small panel over
any app. esc, ⌘S or clicking away keeps what you wrote as a **scratch note**:
it stays on this Mac, is never synced and never published, until you decide.
⌘D makes it a draft on your blyg, and ⌘⏎ publishes it (as a fragment if it
fits, otherwise as a thread).

Scratch notes show in the main list with a `scratch` pill, are searchable and
editable, and ⌘D or ⌘⏎ there promotes them too. Images pasted into a scratch
note stay on your Mac until it's promoted, when they're uploaded.

- `capture-default = draft` makes esc save a draft instead.
- `new-note = scratch` makes the omnibar create scratch notes.

| | |
|---|---|
| ![Quick capture](screenshots/quick-capture.png) | ![Quick capture's button row](screenshots/quick-capture-buttons.png) |

## Links, mentions, images and spelling

- **Paste a link over text.** Select some text and paste a web address to link
  it, as in WordPress: the text becomes `[text](address)`.
- **Mentions.** Type `@` to pick from the blygs you know (subscriptions,
  blogroll, authors you read). It inserts a plain link to that blyg, since
  Blygger has no handles.
- **Images.** Paste or drop an image to upload it; it goes into the text
  where you pasted it, and the preview shows it. Until a published post shows
  it, an upload is private on studio 0.28 or later: only you (and Burrow,
  signed in as you) can see it.
- **Spellcheck** with the macOS spell checker, your languages and learned
  words: a red wavy underline, and right-click for suggestions, Learn Spelling
  and Ignore. Code, links, quotes and TK markers are skipped, and checking
  runs in the background after you pause, so typing stays instant.
  `spellcheck = false` turns it off.

| | |
|---|---|
| ![Picking a blyg after @](screenshots/mention-picker.png) | ![Spelling suggestions](screenshots/spellcheck.png) |

## The preview and the full editor

A live preview beside the editor shows the post exactly as your blyg will
publish it, in your blyg's own theme: quotes, links, AI-written spans, images
and video. Click a paragraph in the preview to jump to it.

- ⌘1 **Write**: the list and the editor.
- ⌘2 **Preview**: the list, the editor and the preview.
- ⌘3 **Full editor**: the editor and the preview, with the list hidden.
- ⌘E toggles the preview in place.

![An image in the preview](screenshots/image-preview.png)

## Quoting other posts

In a thread, `![[id]]` alone on a line **quotes** a post: yours, or one from a
blyg you follow. The quote is a snapshot taken when you publish; later edits
to the original never rewrite it. ⌘K (or typing `![[` at the start of a line)
opens a picker of the posts you hold, and ⏎ inserts the line. The picker
searches every word of your published posts and the blyg posts you read, on
your Mac, and filters by source (all, mine, imported) and blyg, newest or
oldest first. Where you type the search is the blyg's **picker typing**
setting: after the brackets in the editor (the default on a Mac), or in the
picker's own box. Quotes only go
in threads: in a fragment, Burrow says so and leaves the text alone.

### Partial quotes

`![[id]]` followed directly by `>` lines quotes **just that passage** of the
post:

```markdown
![[01J9ZK3Q7T5V8X2B4N6M0PQRST]]
> The passage you want, exactly as it is in the post.

What you have to say about it.
```

A blank line between `![[id]]` and the `>` lines makes it a whole quote
followed by your own blockquote instead. The preview flags a passage that
isn't in the quoted post, and publishing refuses it. In the stream, a partial
quote shows only its passage.

The easy way to write one is to select the passage while reading and press
⇧⌘D, or to reply and choose **quote a passage instead** in the stub; see
[Reading, quoting and responses](reading.md#universal-quoting).

### `[[id]]` links

An inline `[[id]]` **links** to one of your posts or one you read, without
quoting it:

```markdown
As I said in [[01J9ZK3Q7T5V8X2B4N6M0PQRST]], the lighthouse is the point.
```

The preview shows it as the published page will: the target's first words in
quotes, linked. Links inside code stay as text. A link that can't be resolved
is flagged in the preview, the status bar and the publish sheet, as quotes
are; publishing refuses it ("one or more references do not resolve").

Typing `[[` opens the same picker, and ⏎ inserts the link.

| | |
|---|---|
| ![The [[ link picker](screenshots/picker-link.png) | ![The ![[ quote picker](screenshots/picker-quote.png) |

**Link post** (next to Fork on a post you're reading) starts a new fragment
holding `[[id]]`: a plain link, not a response. Reply is the one way to
respond.

## TK: gaps for later

Write `[TK]instruction[/TK]` where something is still to come:

```markdown
The harbour was quiet. [TK]one sentence on why, in my voice[/TK]
```

With the caret inside it, ⌘G fills it with AI (if you've switched a provider
on), written in place as `[TK]instruction[=]output[/TK]`. ⌘G inside it again
regenerates. The preview tints generated text, and it's always disclosed as
generated when you publish; an unfilled gap shows as `⚠ ungenerated` in the
preview. See [AI helpers](ai.md).

**Highlighting generated text** (studio 0.27 or later): the blyg's public
pages can tint generated passages and mark them with a small robot that says
what you disclosed. Your blyg's settings (in the Reader's site settings) set
the default; the ⌘G palette sets it per post (default, on or off; **H** cycles
it), and the preview shows the post as its page will.
