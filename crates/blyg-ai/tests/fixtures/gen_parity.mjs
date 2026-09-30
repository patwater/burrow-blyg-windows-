// Regenerates provider_ts_parity.json from the blyg server's own source, so
// the Rust prompt builder is checked against the real TypeScript.
//
//   node crates/blyg-ai/tests/fixtures/gen_parity.mjs <path/to/apps/blyg/src>
//
// (or set BLYG_SRC to that directory).
//
// It slices SYSTEM_PROMPT, buildUserContent, markDocument and the scope
// markers out of src/ai/provider.ts, parseScopes out of src/tk.ts and
// ID_ALPHABET out of src/util.ts, then builds prompts the way
// src/tk-generate.ts::runGenerateScope does. Needs Node >= 23 (type stripping).

import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { join, dirname } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = process.argv[2] ?? process.env.BLYG_SRC;
if (!src) {
  console.error("usage: node gen_parity.mjs <path/to/apps/blyg/src>  (or set BLYG_SRC)");
  process.exit(2);
}
const lines = (f) => readFileSync(join(src, f), "utf8").split("\n");
const slice = (ls, from, to) => {
  const a = ls.findIndex((l) => l.startsWith(from));
  const b = ls.findIndex((l, i) => i > a && l.startsWith(to));
  if (a < 0 || b < 0) throw new Error(`markers not found: ${from} .. ${to}`);
  return ls.slice(a, b).join("\n");
};

const provider = lines("ai/provider.ts");
const tk = lines("tk.ts");
const util = lines("util.ts");
const code = [
  util.find((l) => l.startsWith("export const ID_ALPHABET")),
  "interface GenerateRequest { instruction: string; currentText: string | null; sources: { id: string; content_md: string }[]; documentContext: string; stylePrompt: string | null; }",
  slice(provider, "export const SCOPE_MARK_START", "/** Thrown on any provider failure"),
  slice(provider, "export function markDocument", "/**"),
  slice(provider, "const SYSTEM_PROMPT =", "interface AnthropicContentBlock"),
  "export { SYSTEM_PROMPT, buildUserContent };",
  slice(tk, "const SOURCE_REF", "/** Scopes with no output yet"),
].join("\n\n");

const dir = mkdtempSync(join(tmpdir(), "blyg-parity-"));
const file = join(dir, "parity.ts");
writeFileSync(file, code);
const m = await import(file);

const ID1 = "0123456789abcdefghjkmnpqrs";
const ID2 = "zyxwvtsrqpnmkjhgfedcba9876";
const cases = [
  {
    name: "first generation, no sources, no style",
    content_md: "On friction: every step is a place the thought can die. [TK]say why, briefly[/TK] So the tool should disappear.",
    scope: 0,
    sources: {},
    style: null,
  },
  {
    name: "regenerate with sources and a style prompt, block scope",
    content_md: `Why I built this\n\n[TK] open with the idea from ![[${ID1}]] and ![[${ID2}]] [=]An old draft.[/TK]\n\nMore text, and a second [TK]later[/TK].`,
    scope: 0,
    sources: { [ID1]: "Every step between a thought and a post is a place it can die.", [ID2]: "Tools should disappear." },
    style: "Plain words. Short sentences. No hype.",
  },
  {
    name: "second scope, multibyte text",
    content_md: "Café ☕ notes — naïve 🙂 [TK]a[=]x[/TK] then [TK]sum it up ![[" + ID1 + "]][/TK] fin ✓",
    scope: 1,
    sources: { [ID1]: "Ünïcode source — 日本語 🙂" },
    style: "",
  },
];

const out = cases.map((c) => {
  const { scopes, errors } = m.parseScopes(c.content_md);
  if (errors.length) throw new Error(JSON.stringify(errors));
  const s = scopes[c.scope];
  const req = {
    instruction: s.instruction,
    currentText: s.output,
    sources: s.sourceIds.map((id) => ({ id, content_md: c.sources[id] })),
    documentContext: m.markDocument(c.content_md, s.start, s.end),
    stylePrompt: c.style || null,
  };
  const system = req.stylePrompt ? `${m.SYSTEM_PROMPT}\n\n${req.stylePrompt}` : m.SYSTEM_PROMPT;
  return { ...c, expected_system: system, expected_user: m.buildUserContent(req), expected_source_ids: s.sourceIds };
});

writeFileSync(join(here, "provider_ts_parity.json"), JSON.stringify(out, null, 2) + "\n");
console.log(`wrote ${out.length} cases`);
