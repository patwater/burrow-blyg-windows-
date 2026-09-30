#!/usr/bin/env node
// Generate blyg-render parity fixtures by running the reference Worker's own
// TypeScript renderer (markdown.ts + embeds.ts, tk.ts, transclusion.ts,
// pages.ts, attachments.ts) over the neutral inputs in this directory.
//
// Usage (the Worker checkout is NOT part of this repo):
//
//   node crates/blyg-render/tests/fixtures/gen_parity.mjs /path/to/worker
//
// where /path/to/worker is the Worker package directory (the one holding
// package.json, src/ and node_modules/; run `npm install` there first).
// Nothing in the Worker directory is modified: its modules are bundled with
// the Worker's own esbuild into a temporary file and imported from there.
//
// Output: ./parity/<case>.json, one file per corpus case, holding the input
// and the HTML the Worker's studio preview produces for it. The files are
// committed so CI never needs the Worker source.

import { createRequire } from "node:module";
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, rmSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const workerDir = resolve(process.argv[2] ?? process.env.BLYG_WORKER_DIR ?? "");
if (!process.argv[2] && !process.env.BLYG_WORKER_DIR) {
  console.error("usage: gen_parity.mjs <worker-package-dir>   (or set BLYG_WORKER_DIR)");
  process.exit(2);
}

const require = createRequire(join(workerDir, "package.json"));
const esbuild = require("esbuild");

// --- 1. Bundle the Worker's render modules -------------------------------------
const tmp = mkdtempSync(join(tmpdir(), "blyg-parity-"));
const outfile = join(tmp, "worker-render.mjs");
await esbuild.build({
  stdin: {
    contents: `
      export { renderMarkdown } from "./markdown.ts";
      export { parseScopes, previewStrip, annotateGenerated, applyGeneratedWrappers } from "./tk.ts";
      export { previewTransclusions } from "./transclusion.ts";
      export { injectProvenance, transclusionProvenance, mediaHtml } from "./pages.ts";
      export { previewMedia } from "./attachments.ts";
    `,
    resolveDir: join(workerDir, "src"),
    loader: "ts",
    sourcefile: "parity-entry.ts",
  },
  bundle: true,
  format: "esm",
  platform: "node",
  outfile,
  logLevel: "warning",
});
const W = await import(pathToFileURL(outfile).href);
rmSync(tmp, { recursive: true, force: true });

const mdVersion = JSON.parse(readFileSync(require.resolve("markdown-it/package.json"), "utf8")).version;
const linkifyVersion = JSON.parse(readFileSync(require.resolve("linkify-it/package.json"), "utf8")).version;

// --- 2. A fake D1 over fake_store.json ------------------------------------------
// Answers exactly the queries resolveTarget() and transclusionProvenance() issue.
// The Rust FakeResolver (tests/common/mod.rs) implements the same store.
const store = JSON.parse(readFileSync(join(here, "fake_store.json"), "utf8"));

function fakeDb() {
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

// --- 3. The studio preview pipeline -------------------------------------------------
// annotateTkPreview is a private helper in studio.ts; this is its body verbatim
// (parseScopes -> previewStrip -> annotateGenerated with every span highlighted).
function annotateTkPreview(contentMd) {
  const { scopes, errors } = W.parseScopes(contentMd);
  const { text, spans } = W.previewStrip(contentMd, scopes);
  const annotated = W.annotateGenerated(text, spans, spans.map(() => true));
  return {
    scopes,
    errors,
    text: annotated.text,
    inert: annotated.inertRanges, // generated spans: `![[id]]` inside them never transcludes
    finish: (html) => W.applyGeneratedWrappers(html, annotated),
  };
}

async function render(c) {
  const tk = annotateTkPreview(c.md);
  const tkOut = {
    scopes: tk.scopes.map((s) => ({ instruction: s.instruction, generated: s.output !== null, block: s.block, source_ids: s.sourceIds })),
    errors: tk.errors,
  };
  if (c.kind === "fragment") {
    // studio.post("/preview"): fragments never resolve transclusions.
    return { html: tk.finish(W.renderMarkdown(tk.text)), tk: tkOut, transclusions: [], errors: [] };
  }
  // studio.post("/preview-thread") + the public page's injectProvenance().
  const db = fakeDb();
  const resolved = await W.previewTransclusions(db, tk.text, store.self_id, tk.inert);
  const preview = tk.finish(resolved.html);
  const provenance = await W.transclusionProvenance(db, resolved.transclusions, store.mount);
  return {
    html: W.injectProvenance(preview, provenance),
    tk: tkOut,
    transclusions: resolved.transclusions,
    errors: resolved.errors,
  };
}

// --- 4. Write fixtures ------------------------------------------------------------------
const corpus = JSON.parse(readFileSync(join(here, "corpus.json"), "utf8"));
const outDir = join(here, "parity");
mkdirSync(outDir, { recursive: true });
for (const f of readdirSync(outDir)) if (f.endsWith(".json")) rmSync(join(outDir, f));
for (const c of corpus) {
  const out = await render(c);
  const doc = { name: c.name, kind: c.kind, md: c.md, ...out };
  writeFileSync(join(outDir, `${c.name}.json`), JSON.stringify(doc, null, 2) + "\n");
}
// The CommonMark spec examples through the Worker's renderMarkdown (html:false,
// linkify, embeds), as a broad check of the Markdown engine itself.
const spec = JSON.parse(readFileSync(join(here, "commonmark_spec_input.json"), "utf8"));
const specOut = spec.examples.map((e) => ({ example: e.example, section: e.section, md: e.md, html: W.renderMarkdown(e.md) }));
writeFileSync(join(outDir, "_commonmark_spec.json"), JSON.stringify({ source: spec.source, examples: specOut }, null, 1) + "\n");

// linkify-it's and markdown-it's own linkify test vectors, as paragraphs.
const vectors = JSON.parse(readFileSync(join(here, "linkify_vectors_input.json"), "utf8"));
const vecOut = vectors.inputs.map((md) => ({ md, html: W.renderMarkdown(md) }));
writeFileSync(join(outDir, "_linkify_vectors.json"), JSON.stringify({ source: vectors.source, cases: vecOut }, null, 1) + "\n");

// Attachments: the public pages' mediaHtml and the studio preview's strip.
const media = JSON.parse(readFileSync(join(here, "media_input.json"), "utf8"));
const mediaOut = media.cases.map((c) => ({
  ...c,
  media_html: W.mediaHtml(c.media, media.mount),
  preview_media: W.previewMedia(c.media, media.mount),
}));
writeFileSync(join(outDir, "_media.json"), JSON.stringify({ mount: media.mount, cases: mediaOut }, null, 2) + "\n");

writeFileSync(
  join(outDir, "_manifest.json"),
  JSON.stringify(
    {
      generator: "gen_parity.mjs",
      markdown_it: mdVersion,
      linkify_it: linkifyVersion,
      cases: corpus.length,
      commonmark_examples: specOut.length,
      linkify_vectors: vecOut.length,
      media_cases: mediaOut.length,
    },
    null,
    2,
  ) + "\n",
);
console.log(`wrote ${corpus.length} fixtures (markdown-it ${mdVersion}, linkify-it ${linkifyVersion}) to ${outDir}`);
