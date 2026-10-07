# Reading, quoting and responses

⌘R opens reading: the posts from the blygs and feeds your blyg follows. ⌘R
again goes back to your own posts. Subscriptions (⇧⌘S, or Blyg ›
Subscribe…) are kept on your blyg, so every studio sees the same list.

## The stream

Reading opens as **the stream** (⌥⌘1): one timeline of everything you follow,
newest first, drawn natively so it stays fast with hundreds of posts.
Fragments show in full; threads show a few lines and **Read more** (⏎ or
Space) opens the whole post in a side pane. j/k (or ↑/↓) move; esc closes
the pane and the stream keeps its place.

A post counts as read after a second on screen, and unread posts get a dot,
never a count. Posts are dated by when their author published or last edited
them, and an edit updates the post in place rather than adding a new one.
Quotes show as grey boxes with the quoted text, and `[[id]]` links read as
the post they point to.

![The stream](screenshots/stream.png)

## The Reader

**The Reader** (⌥⌘2) has three panes, like NetNewsWire: sources on the left
(All unread, Today, Thumbed, your folders, then the rest), the posts from
that source in the middle, and the post on the right.

- **Folders** stay on your Mac. Drag a subscription onto one to file it, or
  use its context menu; Blyg › New Folder… makes one.
- ←/→ move between the panes, j/k go to the next or previous post from
  anywhere, and Space scrolls the post, then moves to the next unread one.
- ⌥⌘S hides the sources pane, and its edge drags to resize.

![The three-pane Reader](screenshots/reader-three-pane.png)

**Search** (⌘F, or `/` with the list focused) filters whatever you're reading
as you type, over the title, the author, the blyg and the text of the posts
already on your Mac. Nothing is fetched to search.

**Mark read or unread.** ⌘-click and ⇧-click pick several posts, ⌘A picks
every post in the list, and esc lets go. **r** marks the picks (or the open
post) read, **u** unread; the right-click menu says how many (*Mark 3
Unread*).

![Picking posts in the Reader](screenshots/reader-picks.png)

**Read state syncs** between your Macs through your blyg when its server keeps
read state (see [Server requirements](server.md)), and marking unread syncs
when it also supports clearing it. Otherwise it stays on this Mac, and a
post you marked unread stays unread here until you read it again.

**Subscriptions** take their name from the blyg or feed, and follow it when
it changes. Right-click one for **Rename…** (your own name, kept as you
set it), **Use the Blyg's Own Name** to hand it back, and **Check Now**.
**⇧⌘R** (Blyg › Check All Feeds Now) asks your blyg to poll every
subscription at once.

| | |
|---|---|
| ![A subscription's menu](screenshots/reader-sub-menu.png) | ![Renaming a subscription](screenshots/reader-sub-rename.png) |

## Acting on a post

The selected post shows its actions. Each says what it makes:

- **Quote into a thread**: `![[id]]` in a thread of yours.
- **Reply · new stub**: a new thread that responds to the post (a *stub*,
  Blygger's reply). It starts quoting the whole post. A line above the
  editor says what the stub is doing; **quote a passage instead** shows the
  post's text, and the passage you select there becomes the quote (`>` lines
  under `![[id]]`). Once there is one, the next passage goes after the caret
  as its own quote, for a running commentary; **quote whole post** goes back.

  ![Choosing a passage in a stub](screenshots/stub-passage.png)
- **AI reply · new stub**: the same, with a first draft from AI.
- **Link post · new fragment**: a fragment that links to the post (`[[id]]`)
  without responding to it.
- **Fork**: a new thread from a *pinned* version of the post. Forking only
  descends from pins, so on the current version Fork is greyed out and its
  tooltip says where to find a pin.
- **→ Notes**: adds the post to your [notes](notes-and-browser.md).
- **Open on web**, and 👍 / 👎.

![Reading a post](screenshots/reading.png)

### Versions of other people's posts

The version pill in a post's header, `‹ vN ▾ ›`, steps through the current
version and the **pinned** ones (unpinned past versions of other people's
posts are never shown). On a pin you can **Quote this version**, **Fork this
pin**, or **Diff vs now**. When an author edits a post you've read, it shows
"edited", with the author's version notes since you read it.

![Versions of someone else's post](screenshots/versions.png)

## Universal quoting

Highlight text anywhere you read, then press **⇧⌘D** (Post › Quote Selection
in Draft):

- in the reading pane or the preview;
- in the built-in [browser](notes-and-browser.md#the-browser-pane);
- in your [notes](notes-and-browser.md#the-notes-drawer).

The passage goes into the draft open in the editor, at the caret, or into a
new draft if none is open, with where it came from:

- a passage from a post on a blyg you follow, quoted into a thread, becomes a
  **partial quote** of that post, `![[id]]` with the passage under it as `>`
  lines (see [Partial quotes](writing.md#partial-quotes));
- anything else becomes a blockquote followed by a link to the page.

Nothing selected? You get a short message, and nothing is made.

**The "Quote in draft" pill.** While you have text selected in the reading
pane or the preview, a small **Quote in draft** pill appears beside the
selection; clicking it does what ⇧⌘D does. The selection stays until you
quote it, and dragging inside a quote no longer opens the original. ⌘C
copies a selection in the reading pane. In the browser, the **→ Draft**
button reads **❝ Quote → Draft** while text is selected. In the stream,
dragging over or double-clicking a post opens it where its text can be
selected.

## Quotes, stubs and forks you can follow

Click a quote's text to open the post it quotes, at the quoted version, or
its author's name for their profile. The "↳ stub of" and "⑂ forked from"
lines under a post work the same way.

Each post lists who quoted, stubbed or forked it, from the blygs you follow
and your own published posts: a list, never a count. A post that both stubs
and quotes another is one response, listed as the stub, and a reply to a
passage says so.

## Lineage

Beside each post's name, in the stream and in Reader, a small **glyph** shows
its lineage: lines coming in on the left are the kinds of post it draws on,
lines going out on the right the kinds that draw on it. Each kind has its own
place (fork at the top, reply in the middle, quote at the bottom) and colour,
and a dotted line means only a passage. It never says how many; hover for the
kinds in words.

**⌘J** (Post › Lineage…), or a click on the glyph, opens the lineage view. The
post sits in a hexagon, what it draws on above it and what draws on it below.
The arrow keys move between them, ⏎ makes a neighbour the centre so you can
walk a conversation one step at a time, ⌫ walks back, **o** opens the selected
post in the reader, and esc closes. Responses are what this Mac holds: posts
in your reading list, your own posts, and verified mentions of your posts.

![The lineage view](screenshots/lineage.png)

**Space** (or ⏎) on the post in the middle opens the **ring** of the reader's
actions, always in the same places: **F** fork, **R** reply, **Q** quote,
**L** link post, **V** versions, **O** open on the web. A letter previews the
action: a dashed node shows what it would make, and the panel answers the same
four questions for each (is it a response, is the author told, are their
words in yours, does it show under their post). ⏎, or the same letter again,
does it; esc backs out. An action that can't be done (a fork of a post with
no pinned version, say) says why.

![The action ring, previewing Reply](screenshots/lineage-ring.png)

## Mentions and responses

**Mentions** (⇧⌘M) lists the verified responses to your own posts: who
quoted, stubbed or forked which post, and when. You can hide one, and your own
posts list their mentions under them in ⌘Y and in the reading pane.

Your blyg decides whether a post's public page shows its responses. In
**Site settings** (Blyg › Site Settings…), **Show responses by default** sets
it for the whole blyg, on blygs that report it. On the Mentions screen, each
post can **follow site setting** (the default), or **show** or **hide** its
responses itself. Site settings also hold the site's title, author, bio and
links, the time zone for dates on your pages, and **Accept mentions**, on
blygs that report them.

![Mentions](screenshots/mentions.png)

## Profiles

⌘I on a post (or a click on its author's address, its "↳ stub of" /
"⑂ forked from" line, a mention, or a blogroll entry) shows who wrote it:
name, bio, links, their blogroll, recent posts, and the blygs they quote,
stub and fork. **Follow** in one click (private; nobody is told), or **add
them to your own public blogroll**. ⇧⌘O opens any address, and Blyg › My
Profile shows yours as visitors see it.

Profiles are fetched from the blyg's public files without your token, only
when you open one, and kept for offline use. There are no follower counts
anywhere.

| | |
|---|---|
| ![A profile](screenshots/profile.png) | ![Your own profile, with blogroll toggles](screenshots/profile-own.png) |
