# Third-party notices

Terminal-Fenster is licensed MIT. It depends on the following third-party
components, which carry their own licenses.

| Component | Version | License | Role |
|---|---|---|---|
| Electron | 43.2.0 | MIT (bundles Chromium, BSD-3-Clause + others) | Offscreen browser engine |
| Chromium | 150.0.7871.129 | BSD-3-Clause and others | Web engine, shipped inside Electron |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | termios / ioctl / poll / shared-memory bindings |
| `flate2` | 1.1.9 | MIT OR Apache-2.0 | zlib compression of direct frame payloads |
| `crc32fast` | 1.5.0 | MIT OR Apache-2.0 | `flate2` dependency |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | `crc32fast` dependency |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 | `flate2` dependency |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | `miniz_oxide` dependency |
| `simd-adler32` | 0.3.10 | MIT | `miniz_oxide` dependency |

Electron is **not vendored** into the source repository. Source installs acquire the exact npm
pin. Prebuilt release archives instead fetch Electron's official platform archive and its
free-codecs FFmpeg archive, verify both against `packaging/engine-lock.json`, and ship no npm
downloader dependencies. Each prebuilt archive includes the exact Cargo license files under
`licenses/rust/`, Electron's `dist/LICENSE`, and Chromium's full
`dist/LICENSES.chromium.html`. See ADR-0002 and `tools/package-release.sh`.

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
