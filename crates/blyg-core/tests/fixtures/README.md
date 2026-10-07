# Test fixtures

`openapi.json` is the owner-API contract from upstream
[blygger-studio](https://github.com/blygger/blygger-studio) v0.32.1
(commit a3d2cd6; MIT License, Copyright (c) 2026 Venkatesh Rao).

`extensions.json` describes, in the same form, the routes and replies the
app uses that upstream lacks (the Worker fork's extensions, docs/SERVER.md).
An upstream operation is checked against `openapi.json`; anything else, and a
reply status upstream doesn't declare (`POST /api/media` → 200 `duplicate`),
against `extensions.json`. A route in neither fails the test. The mock
server in `tests/common/` validates every request the app sends, and every
response the mock gives, against them (`tests/common/contract.rs`).

To update it, copy `openapi.json` from a newer upstream checkout, then run
`cargo test -p blyg-core`. Any contract violation fails the test that caused it.
