# Third-party notices

Blygger Desktop is MIT licensed (see `LICENSE`). It includes third-party
software under the licenses below. The app bundle carries these files in
`Blygger.app/Contents/Resources/licenses/`.

## Fonts

The fonts are compiled into the `blygger` binary. Each keeps its own license,
and the full texts are in `crates/blyg-app/assets/fonts/<font>/`.

| Font | License | Copyright |
|---|---|---|
| Literata | SIL Open Font License 1.1 | The Literata Project Authors |
| Inter | SIL Open Font License 1.1 | The Inter Project Authors |
| Source Serif 4 | SIL Open Font License 1.1 | Adobe |
| iA Writer Quattro | SIL Open Font License 1.1 | Information Architects Inc., IBM Corp. (Plex) |
| ET Book | MIT | Dmitry Krasny, Bonnie Scranton, Edward Tufte |

Under the OFL, the fonts can be bundled and redistributed with software, but
they can't be sold on their own, and modified versions can't use the Reserved
Font Names.

## Icons

The toolbar icons (`show-buttons`) are twelve [Lucide](https://lucide.dev) icons,
compiled into the binary from `crates/blyg-app/assets/icons/`. The only change
is a thinner stroke (1.75 instead of 2).

| Icons | License | Copyright |
|---|---|---|
| Lucide (plus, file-up, send, panel-left, columns-2, columns-3, rotate-ccw-clock, sparkles, zap, sticky-note, trash-2, archive-x) | ISC | Lucide Icons and Contributors |

Lucide's icons derived from Feather (`plus` among them) are MIT, Copyright Cole Bemis.
The full text of both licenses is in `crates/blyg-app/assets/icons/LICENSE`.

## Rust crates

The binary statically links about 590 crates (counting the aarch64 and x86_64
builds together). `packaging/rust-dependencies.tsv` lists each one with its
version, license and repository. To regenerate it:

```sh
cargo install cargo-license --locked
scripts/licenses.sh
```

The licenses in use (crate counts):

| License (SPDX) | Crates |
|---|---|
| Apache-2.0 OR MIT | 357 |
| MIT | 108 |
| Apache-2.0 OR MIT OR Zlib | 29 |
| Apache-2.0 (including GPUI) | 24 |
| Unicode-3.0 | 18 |
| BSD-3-Clause | 11 |
| MIT OR Unlicense | 8 |
| Zlib | 7 |
| BSD-2-Clause | 4 |
| MPL-2.0 (`option-ext`, `cssparser`, `dtoa-short`) | 3 |
| Apache-2.0 OR BSD-2-Clause OR MIT | 3 |
| ISC, CDLA-Permissive-2.0, Apache-2.0 OR BSD-3-Clause, 0BSD | 2 each |
| CC0-1.0, bzip2-1.0.6, and single crates under other permissive combinations | 1 each |

None of these are copyleft for the application as a whole. The MPL-2.0
crates (`option-ext`, and `cssparser` and `dtoa-short`, which `ammonia`
uses) are file-level copyleft: their sources are unmodified and available
from crates.io.

The Reading screen sanitizes other people's HTML with
[`ammonia`](https://github.com/rust-ammonia/ammonia) (Apache-2.0 OR MIT),
on `html5ever` (Apache-2.0 OR MIT).

In-app updates check release signatures with
[`ed25519-dalek`](https://github.com/dalek-cryptography/curve25519-dalek)
and `curve25519-dalek` (BSD-3-Clause; Copyright (c) 2016-2021 isis agora
lovecruft, Copyright (c) 2016-2021 Henry de Valence), plus `semver` and
`base64` (Apache-2.0 OR MIT).

SQLite (via `rusqlite`'s bundled build) is in the public domain.

For a full notices file with every license text, use
[`cargo-about`](https://github.com/EmbarkStudios/cargo-about):
`cargo install cargo-about --locked && cargo about init && cargo about generate about.hbs > THIRD_PARTY_LICENSES.html`.
