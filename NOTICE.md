# Third-party notices

BlackGlass is licensed MIT OR Apache-2.0. It depends on the following third-party
components, which carry their own licenses.

| Component | Version | License | Role |
|---|---|---|---|
| Electron | 43.2.0 | MIT (bundles Chromium, BSD-3-Clause + others) | Offscreen browser engine |
| Chromium | 150.0.7871.129 | BSD-3-Clause and others | Web engine, shipped inside Electron |
| `libc` (crates.io) | 0.2.x | MIT OR Apache-2.0 | termios / ioctl / poll bindings |
| `flate2` (crates.io) | 1.x | MIT OR Apache-2.0 | zlib compression of frame payloads |

Electron is **not vendored** into this repository. It is installed via npm into
`apps/engine/node_modules` and must be verified against upstream `SHASUMS256.txt` before
distribution (see `artifacts/swarm/B10-packaging-updater.md`).

## Studied but not copied

The following projects were studied as prior art. No code was taken from any of them.

- **zenbu-labs/terminal-browser** — the benchmark product for this effort. Recon (A02)
  found **no LICENSE file**, which means all rights reserved. Its implementation is
  therefore treated as unavailable: only publicly observable behaviour and documentation
  informed this work.
- **Carbonyl** (BSD-3-Clause) — a Chromium fork, last real commit 2023-02-26, pinned to
  Chromium 111. Studied for its glyph-cell rendering approach; its architecture was
  deliberately not followed (see ADR-0001).
- **Browsh, w3m, Lynx, Nyxt, Vieb** — studied for UX conventions only.
- **microsoft/playwright-mcp** — its accessibility-snapshot-with-refs *concept* informed our
  MCP design. No code copied; check its license before any reuse.
