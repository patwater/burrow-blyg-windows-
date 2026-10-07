#!/usr/bin/env node
// Generate blyg-render parity fixtures by running the reference Worker's own
// TypeScript renderer (markdown.ts + embeds.ts, tk.ts, authoring.ts,
// transclusion.ts, importer/sanitize.ts, pages.ts) over the neutral inputs in
// this directory.
//
// Usage (the Worker checkout is NOT part of this repo):
//
//   node crates/blyg-render/tests/fixtures/gen_parity.mjs /path/to/worker
//
// where /path/to/worker is the Worker package directory (the one holding
// package.json, src/ and node_modules/; run `npm ci` there first). Nothing in
// the Worker directory is modified: gen_parity_worker.js and the Worker's
// modules are bundled with the Worker's own esbuild into a temporary file,
// which runs inside workerd through the Worker's own Miniflare. (workerd,
// not Node: the thread preview's sanitizer is built on HTMLRewriter.)
//
// Output: ./parity/<case>.json, one file per corpus case, holding the input
// and the HTML the Worker's studio preview produces for it. The files are
// committed so CI never needs the Worker source. Nothing is written unless
// every case renders, so a failed run leaves the old fixtures in place.

import { createRequire } from "node:module";
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, rmSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
if (!process.argv[2] && !process.env.BLYG_WORKER_DIR) {
  console.error("usage: gen_parity.mjs <worker-package-dir>   (or set BLYG_WORKER_DIR)");
  process.exit(2);
}
const workerDir = resolve(process.argv[2] ?? process.env.BLYG_WORKER_DIR);

const require = createRequire(join(workerDir, "package.json"));
const esbuild = require("esbuild");
const { Miniflare } = require("miniflare");

// --- 1. Bundle the harness with the Worker's render modules ----------------------
const tmp = mkdtempSync(join(tmpdir(), "blyg-parity-"));
const outfile = join(tmp, "worker-render.mjs");
await esbuild.build({
  entryPoints: [join(here, "gen_parity_worker.js")],
  alias: { "worker-src": join(workerDir, "src") },
  nodePaths: [join(workerDir, "node_modules")],
  bundle: true,
  format: "esm",
  platform: "browser",
  conditions: ["workerd", "worker", "browser"],
  external: ["node:*", "cloudflare:*"],
  outfile,
  logLevel: "warning",
  plugins: [
    {
      // pages.ts keeps mediaHtml private since studio 0.10; expose it to
      // this bundle only (the Worker's source is read, never changed).
      name: "expose-media-html",
      setup(b) {
        b.onLoad({ filter: /[\\/]src[\\/]pages\.ts$/ }, (a) => {
          const src = readFileSync(a.path, "utf8");
          const exported = /export\s+(async\s+)?function\s+mediaHtml\b/.test(src);
          return { contents: exported ? src : src + "\nexport { mediaHtml };\n", loader: "ts" };
        });
      },
    },
  ],
});
const script = readFileSync(outfile, "utf8");
rmSync(tmp, { recursive: true, force: true });

const pkg = (name) => JSON.parse(readFileSync(require.resolve(`${name}/package.json`), "utf8")).version;
const studioVersion = JSON.parse(readFileSync(join(workerDir, "package.json"), "utf8")).version;

// --- 2. Render everything inside workerd ------------------------------------------
const read = (f) => JSON.parse(readFileSync(join(here, f), "utf8"));
const input = {
  store: read("fake_store.json"),
  corpus: read("corpus.json"),
  spec: read("commonmark_spec_input.json"),
  vectors: read("linkify_vectors_input.json"),
  media: read("media_input.json"),
};
const mf = new Miniflare({
  modules: true,
  script,
  compatibilityDate: "2025-01-01",
  compatibilityFlags: ["nodejs_compat"],
});
let out;
try {
  const res = await mf.dispatchFetch("http://parity.invalid/", { method: "POST", body: JSON.stringify(input) });
  if (!res.ok) throw new Error(`harness failed: ${res.status} ${await res.text()}`);
  out = await res.json();
} finally {
  await mf.dispose();
}

// --- 3. Write fixtures --------------------------------------------------------------
const outDir = join(here, "parity");
mkdirSync(outDir, { recursive: true });
for (const f of readdirSync(outDir)) if (f.endsWith(".json")) rmSync(join(outDir, f));
const write = (name, value, indent) => writeFileSync(join(outDir, name), JSON.stringify(value, null, indent) + "\n");
for (const doc of out.cases) write(`${doc.name}.json`, doc, 2);
write("_commonmark_spec.json", { source: input.spec.source, examples: out.spec }, 1);
write("_linkify_vectors.json", { source: input.vectors.source, cases: out.vectors }, 1);
write("_media.json", { mount: input.media.mount, cases: out.media }, 2);
write(
  "_manifest.json",
  {
    generator: "gen_parity.mjs",
    studio: studioVersion,
    markdown_it: pkg("markdown-it"),
    linkify_it: pkg("linkify-it"),
    cases: out.cases.length,
    commonmark_examples: out.spec.length,
    linkify_vectors: out.vectors.length,
    media_cases: out.media.length,
  },
  2,
);
console.log(`wrote ${out.cases.length} fixtures (studio ${studioVersion}, markdown-it ${pkg("markdown-it")}) to ${outDir}`);
