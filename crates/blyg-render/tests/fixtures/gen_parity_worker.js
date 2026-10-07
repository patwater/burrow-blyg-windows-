// The Worker side of gen_parity.mjs: bundled together with the reference
// Worker's own render modules (`worker-src/*` is an esbuild alias for the
// Worker's src/) and run inside workerd through Miniflare, because the preview
// sanitizer (importer/sanitize.ts) is built on workerd's HTMLRewriter.
//
// POST / with { store, corpus, spec, vectors, media } returns every fixture's
// output. Nothing here re-implements Worker logic: render() is the body of
// read-api.ts's POST /api/preview handler (fragment) and threadPreview()
// (thread), with a fake D1 in place of the database.

import { renderMarkdown } from "worker-src/markdown.ts";
import { parseScopes } from "worker-src/tk.ts";
import { annotateTkPreview } from "worker-src/authoring.ts";
import { applyInternalLinks, previewInternalLinks, previewTransclusions, resolveBlockLinks } from "worker-src/transclusion.ts";
import { injectProvenance, transclusionProvenance, mediaHtml } from "worker-src/pages.ts";

// --- A fake D1 over fake_store.json ----------------------------------------------
// Answers exactly the queries resolveTarget() and transclusionProvenance() issue.
// The Rust FakeResolver (tests/common/mod.rs) implements the same store.
function fakeDb(store) {
  const answer = (sql, args) => {
    const s = sql.replace(/\s+/g, " ").trim();
    if (s === "SELECT * FROM items WHERE id = ?") {
      return { first: store.items.find((i) => i.id === args[0]) ?? null };
    }
    if (s === "SELECT * FROM versions WHERE item_id = ? AND version = ?") {
      const it = store.items.find((i) => i.id === args[0] && i.version === args[1]);
      return { first: it ? { item_id: it.id, version: it.version, content_html: it.content_html } : null };
    }
    if (s.startsWith("SELECT v.transclusions AS t FROM items i JOIN versions v")) {
      return { first: { t: null } }; // the fake store records no nested closures
    }
    if (s.startsWith("SELECT ii.*, s.origin AS sub_origin")) {
      const rows = store.imported.filter((r) => r.remote_id === args[0]).map((r) => ({ ...r, sub_origin: r.origin }));
      return { all: { results: rows } };
    }
    if (s.startsWith("SELECT ii.kind AS kind, ii.page AS page, s.title AS title")) {
      const r = store.imported.find((x) => x.remote_id === args[0] && x.origin === args[1]);
      return { first: r ? { kind: r.kind, page: r.page, title: r.sub_title } : null };
    }
    throw new Error("fake D1: unexpected query: " + s);
  };
  return {
    prepare(sql) {
      let args = [];
      const stmt = {
        bind: (...a) => ((args = a), stmt),
        first: async () => answer(sql, args).first,
        all: async () => answer(sql, args).all,
      };
      return stmt;
    },
  };
}

// --- The preview pipeline (read-api.ts, POST /api/preview) -------------------------
async function render(store, c) {
  const db = fakeDb(store);
  const tk = annotateTkPreview(c.md);
  const tkOut = {
    scopes: tk.scopes.map((s) => ({ instruction: s.instruction, generated: s.output !== null, block: s.block, source_ids: s.sourceIds })),
    errors: parseScopes(c.md).errors,
  };
  // `[[id]]` resolves against siteOrigin; here the mount plus "/", which is
  // what the Rust side derives from `mount`.
  const origin = store.mount + "/";
  const links = await previewInternalLinks(db, tk.text, origin);
  const blocks = await resolveBlockLinks(tk.blocks, (html) => previewInternalLinks(db, html, origin, true));
  const splice = (html) => [links, ...blocks.docs].reduce((acc, doc) => applyInternalLinks(acc, doc), tk.finish(html));
  const linkErrors = [...links.errors, ...blocks.errors];
  if (c.kind === "fragment") {
    // Fragments never resolve transclusions, and are not sanitized.
    return { html: splice(renderMarkdown(links.text)), tk: tkOut, transclusions: [], errors: linkErrors };
  }
  // threadPreview() + the public page's injectProvenance().
  // The Worker never makes this combination: its previews show no provenance,
  // and injectProvenance only ever sees published HTML, which can't hold an
  // unresolved marker. The desktop preview makes it on purpose, so the marker
  // is hidden from injectProvenance here, as it would be absent on the page.
  const resolved = await previewTransclusions(db, links.text, store.self_id);
  const preview = splice(resolved.html);
  const provenance = await transclusionProvenance(db, resolved.transclusions, store.mount);
  const MARKER = 'class="blyg-transclusion unresolved"';
  const HIDDEN = 'class="\u0000unresolved-marker"';
  const injected = injectProvenance(preview.split(MARKER).join(HIDDEN), provenance).split(HIDDEN).join(MARKER);
  return { html: injected, tk: tkOut, transclusions: resolved.transclusions, errors: [...resolved.errors, ...linkErrors] };
}

export default {
  async fetch(req) {
    const { store, corpus, spec, vectors, media } = await req.json();
    const cases = [];
    for (const c of corpus) cases.push({ name: c.name, kind: c.kind, md: c.md, ...(await render(store, c)) });
    return Response.json({
      cases,
      // The CommonMark spec examples through renderMarkdown (html:false, linkify, embeds).
      spec: spec.examples.map((e) => ({ example: e.example, section: e.section, md: e.md, html: renderMarkdown(e.md) })),
      // linkify-it's and markdown-it's own linkify test vectors, as paragraphs.
      vectors: vectors.inputs.map((md) => ({ md, html: renderMarkdown(md) })),
      // Attachments: the public pages' mediaHtml, which appends what the
      // content does not already show.
      media: media.cases.map((c) => ({ ...c, media_html: mediaHtml(c.media, media.mount, c.content_html ?? "") })),
    });
  },
};
