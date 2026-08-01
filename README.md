# Terminal-Fenster

A real Chromium browser that renders as **pixels inside your terminal** — not a text-mode
approximation, not a screenshot viewer. Chromium 150 renders offscreen; a Rust core turns
frames into terminal graphics and turns your keyboard and mouse back into browser input.

```
terminal-fenster open news.ycombinator.com
```

Website: [terminal-fenster.com](https://terminal-fenster.com) (source in [`website/`](website/))

## Open source

Licensed under [MIT](LICENSE). Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).
Source: [github.com/zent7x/terminal-fenster](https://github.com/zent7x/terminal-fenster) ·
[commit history](https://github.com/zent7x/terminal-fenster/commits/main)

This is experimental software: see [Known gaps](#known-gaps) and [RELEASE.md](RELEASE.md) before
treating it as production-ready.

## Status: working vertical slice, not shippable yet

This is honest about where it is. What follows is measured, not aspirational.

**Verified working**

- Real pages render in Ghostty via the Kitty graphics protocol, auto-detected.
- Click, hover, ordered typing, scroll, resize, focus pacing, and idle wake-up all work —
  verified with real frame pixels, page state, and scheduler state
  (`tests/e2e/input-injection.js`, 20/20).
- The terminal is restored on exit, `ctrl+q`, panic, SIGINT/SIGTERM/SIGHUP.
- The headless engine probe now asserts process-tree cleanup; its reference `about:blank`
  upper-bound RSS is about 281 MB with no surviving Chromium helpers.
- Renderer crashes produce a visible, escape-sanitized reload banner instead of a silent freeze.
- A 16-tool MCP server drives an isolated Terminal-Fenster engine through accessibility refs and
  the private engine socket; its real-Chromium suite passes 28/28.
- 161 Rust tests, 21 JS frame/security/compositor/discovery/privacy tests, 20 engine E2E checks, 14 browser fixtures,
  24 MCP protocol checks, and 28 live MCP checks.

**Not done** — see [Known gaps](#known-gaps) and [RELEASE.md](RELEASE.md).

## Requirements

- macOS or Linux. Developed and measured on macOS 26.1 / Apple M4.
- A terminal with the Kitty graphics protocol for full fidelity: **Ghostty**, **kitty**, or
  **WezTerm**. iTerm2 also speaks it. A terminal with no graphics protocol at all (e.g.
  Apple Terminal) is headless-only — interactive `open` refuses it and points you to
  `--headless`; sixel- or iTerm2-only terminals render interactively through a low-fidelity
  Unicode half-block fallback.
- Rust 1.80+, Node 22.12+ (required by the pinned Electron runtime).

## Install from a checkout

```bash
./install.sh
```

The source installer builds with the lockfile, stages and materializes Electron before touching
the active install, validates the pinned runtime, then atomically swaps
`~/.local/share/terminal-fenster` and links `~/.local/bin/terminal-fenster`. A failed upgrade restores the
previous prefix. Set `TERMINAL_FENSTER_REUSE_ENGINE=1` to reuse this checkout's already-materialized
engine for an offline/same-machine install. Until a signed release tag exists, do not present the
`curl | bash` form as a reproducible binary release.

```bash
./uninstall.sh                 # preserve browser profiles
./uninstall.sh --purge-profile # also permanently remove Terminal-Fenster profiles
```

The uninstaller refuses unmarked/unrelated prefixes and never removes the shared Electron cache.

## Build a prebuilt release archive

```bash
tools/package-release.sh
```

This produces a host-native `.tar.gz` and SHA-256 sidecar under `dist/`. The archive contains a
minimal engine launcher and the pinned Electron runtime, so installing it needs neither Rust,
Node, npm, a source checkout, nor network access. The packager independently verifies Electron
and its free-codecs FFmpeg artifact, includes all Cargo/Electron/Chromium license material,
checks every extracted payload file, then installs and launches from the archive layout. On
macOS the local artifact is ad-hoc signed for structural verification only; it is not a public
release until Developer-ID signing and notarization pass the gate in `RELEASE.md`.

The `Unsigned release candidates` workflow runs the same native packager on macOS arm64/x64 and
Linux arm64/x64 for an exact version tag or manual dispatch. It retains archives and sidecars as
short-lived CI artifacts only; it deliberately cannot publish a GitHub release or turn an ad-hoc
macOS signature into a production signature.

## Quick start

```bash
./install.sh                 # one-shot install (Rust + Node required once)
terminal-fenster setup             # verify engine + terminal, get next steps
terminal-fenster open example.com  # interactive (Ghostty / kitty / WezTerm / iTerm2)
terminal-fenster open example.com --headless   # scripts, CI, agents — any terminal
```

**macOS Terminal.app** cannot render pages (no graphics protocol). Use a Kitty-capable
terminal for interactive browsing, or `--headless` everywhere else.

## Build and run

```bash
git clone <repo> && cd terminal-fenster
cargo build --release                    # builds the terminal core
cd apps/engine && npm ci                  # installs the pinned Electron package
./node_modules/.bin/electron --version    # materializes Chromium (~300 MB); required once
cd ../..

./target/release/terminal-fenster doctor        # what your terminal can do
./target/release/terminal-fenster open example.com
```

`doctor` first. It tells you which backend you get and, more usefully, *why*, including the
raw bytes your terminal replied with:

```
  graphics
    kitty graphics      yes
    sixel               no
    --> backend         kitty
  input
    kitty keyboard      yes
    sgr-pixels mouse    yes
  geometry
    cells               146x23
    cell px             17x37
    page viewport       2482x851
```

## Keys

| Key | Action |
|---|---|
| `ctrl+q` | quit |
| `ctrl+c` | copy selection |
| `ctrl+r` | reload |
| `ctrl+l` | open URL (omnibox on the status row) |
| `ctrl+f` | find in page (`ctrl+n` / `ctrl+p` next/prev) |
| `ctrl+=` / `ctrl+-` / `ctrl+0` | zoom in / out / reset |
| `ctrl+left` / `ctrl+right` | back / forward (`alt+arrow` where the terminal delivers it) |
| mouse | click, hover, drag, scroll — forwarded to the page |

## MCP automation

Terminal-Fenster ships a **stdio MCP server** with 16 browser tools (navigate, snapshot, click,
type, screenshot, …). Any MCP client that launches subprocess servers over stdio can use it.

```bash
./install.sh
terminal-fenster mcp-config    # JSON for your client's mcpServers block
terminal-fenster mcp           # run the server directly
```

Paste the JSON into your editor or harness MCP settings. The `command` must be the installed
`terminal-fenster` binary; `args` is `["mcp"]`.

Full tool list, security model, and env vars: [`packages/mcp/README.md`](packages/mcp/README.md).

## Automation details

[`packages/mcp/`](packages/mcp/) starts an isolated engine and exposes 16 browser tools over
stdio: navigate, accessibility snapshot/find, click/type/key/scroll, screenshots, history,
resize, wait, status, and close.
Semantic inspection uses Electron's in-process debugger API proxied through the existing 0600
Unix socket; it opens no DevTools TCP port. See [`packages/mcp/README.md`](packages/mcp/README.md)
for client configuration.

## How it works

```
your terminal
     │  tty in raw mode, owned by an RAII guard that restores it on every exit path
[terminal-fenster]        Rust: capability detection, compositor, input decoding, Kitty encoding
     │  unix socket, 0600, inside a 0700 dir — no network listener is ever opened
[engine host]       Electron: offscreen Chromium, sandbox ON
```

Frames are raw BGRA from Chromium's `paint` event. The core retains an RGB canvas and uses a
hybrid Kitty compositor: a runtime-probed POSIX shared-memory base for dense local repaints,
and zlib-compressed position-bound tile overlays for sparse damage. SSH, multiplexers, a failed
probe, or four unconsumed shared objects fall back to direct/zlib transmission. Commands and
events are JSON on the private socket; frames are binary and length-prefixed because a 5 MB
bitmap through JSON would be indefensible.

Architecture decisions with their measurements and their reversal costs live in
[`docs/adr/`](docs/adr/). ADR-0001 records the engine choice *and* the argument it was
originally defended with, which later measurement proved false.

## Measured performance

Ghostty 1.3.1, macOS 26.1, Apple M4 (10 cores, 24 GB), release build. These are real
core-to-terminal runs, not engine-only paint rates:

| Workload | Viewport | Steady received FPS | Trailing FPS | Final image wire | Encode |
|---|---:|---:|---:|---:|---:|
| Local 80×80 animation, default 4×4-cell mosaic | 2108×1332 | 46.5 | 54 | 2,540 B | 0.18 ms |
| Full-viewport repaint, direct/zlib monolithic baseline | 2108×1406 | 7.2 | 7 | 82,086 B | 2.68 ms |

The localized workload is the intended fast path: Chromium sends damage, the core updates a
retained RGB canvas, and only intersecting Kitty tiles cross the terminal boundary. The
direct full-viewport baseline shows the old bottleneck honestly: encoding is only 2.68 ms, but
base64/APC terminal presentation held throughput near 7 fps. The current build runtime-probes
Kitty shared memory and keeps an 8,891,544-byte 2108×1406 RGB frame behind a sub-160-byte PTY
command in OS-level tests; its real Ghostty FPS is deliberately not claimed until the release
gate is rerun. The older logs above count engine frames received; benchmark schema v2 now also
records completed presentations plus BGRA conversion, encode, wire, and presentation-gap p50/p99
so coalescing or hidden conversion work cannot inflate future results.

Reproduce with `benchmarks/`, or directly:

```bash
node benchmarks/bench.mjs --page local-damage
TERMINAL_FENSTER_SHM=0 TERMINAL_FENSTER_TILE_CELLS=1x1 \
  node benchmarks/bench.mjs --page repaint --label direct-control
```

## Terminal support

| Terminal | Graphics | Kitty keyboard | Pixel mouse | Status |
|---|---|---|---|---|
| Ghostty 1.3.1 | Kitty | yes | yes | **verified end-to-end** |
| iTerm2 3.6.9 | Kitty | yes | **no** (permanently reset) | protocol-verified, app not driven |
| Apple Terminal 465 | none → headless-only | no | no | capability-verified |
| kitty, WezTerm | Kitty | yes | yes | expected, **untested** |

Every "yes" above comes from the terminal answering a protocol query, not from matching
`$TERM`. Detection is in `crates/tf-term/src/caps.rs`.

Note the iTerm2 row: it reports SGR-Pixels mouse mode *permanently reset*, so coordinates
arrive as cells. Treating those as pixels would collapse the whole page into its top-left
corner — the pointer mapping handles both, with tests pinning the difference.

## Testing

```bash
cargo test                          # 161 Rust tests, no terminal needed
cd apps/engine && npm test          # 7 frame-scheduler / security-policy unit tests
cd ../.. && node tests/e2e/input-injection.js   # 20 real-pixel / page / security checks
apps/engine/node_modules/.bin/electron tests/fixtures/verify-fixtures.js  # 14 fixtures
cd packages/mcp && npm test         # 14 compositor/discovery/privacy + 24 protocol checks
npm run test:live                   # 28 tools against real Chromium
tools/package-layout-test.sh        # manifest tamper + prebuilt install/layout smoke test
```

The e2e test speaks the engine wire protocol directly, so it runs without a graphics
terminal and works in CI. It asserts on frame pixels: a click must actually change the
colour under the cursor, and clicking one target must *not* activate another.

## Known gaps

Ordered by how much they matter.

1. **The hybrid compositor still needs eyes on a graphics terminal.** Capture-side damage,
   retained RGB, the dense-base/sparse-overlay switch, exact encoder round trips, real POSIX
   shared-memory objects, stale-overlay deletion, and bounded fallback are tested. On-screen
   placement and teardown after switching between shared bases and tile overlays still need a
   Ghostty run before release.
2. **Full-viewport motion needs a fresh measurement.** The direct fallback measured about
   7 fps at 2108×1406. The new local `t=s` path removes pixels, compression, base64, and APC
   chunking from the PTY, but no honest displayed-FPS number exists yet. `RELEASE.md` makes
   20 displayed fps and a 2× improvement over direct control the go/no-go threshold.
3. **SSH adaptive transport: MBDT, byte-credit, fps ladder, and OSR scale downs are wired**
   on the direct Kitty path (`{"t":"fps"}` + resize + pointer remap + Kitty `c`/`r` stretch).
   WAN measurement still open; `TERMINAL_FENSTER_LAG_BUDGET_MS` (default 100) tunes the credit
   window.
4. **Single tab.** No tab strip, history UI, or bookmarks. Bottom-row omnibox (`ctrl+l`),
   find-in-page (`ctrl+f`), and `ctrl+left`/`right` history exist; named profiles via
   `--profile`.
5. **Sixel and iTerm2 backends are unimplemented.** If detection picks one, the CLI
   explicitly degrades to Unicode and `doctor` says so rather than silently faking it.
6. **`onPaint` still copies via `toBitmap()`** — B04's finding; shared-texture path is not
   shipped.
7. **Public artifacts are not signed or notarized.** The macOS arm64 archive pipeline is proven
   locally, but other target archives and a genuinely clean-machine install remain release gates.
8. **The Electron memory floor is still high.** A short 1280×800 `about:blank` probe measured
   280.6 MB peak/steady summed RSS (an upper bound because shared pages are double-counted). The
   idle frame throttle works, but a separately verified low-memory mode is not shipped.

## Security posture

- Chromium's sandbox stays **on**; web content gets no Node integration and context
  isolation is enforced.
- Camera, microphone, location, clipboard, device, and every other privileged page permission
  are denied by explicit session handlers. External-application URL schemes are blocked before
  navigation; denied permissions, schemes, and popups produce a sanitized terminal notice.
- Page-derived text (titles, URLs) is sanitized before it can reach the terminal — a
  malicious title otherwise smuggles escape sequences through us and can drive OSC 52 to
  overwrite your clipboard. C0/C1 controls, bidi overrides/isolates, and invisible formatting
  characters are stripped; the final chrome line is column-clipped before the autowrap cell.
- The control socket is 0600 inside a 0700 directory. MCP CDP calls use Electron's
  in-process debugger through that socket; no DevTools TCP endpoint is exposed.
- Initial URLs travel over that socket rather than process arguments. Diagnostic and MCP audit
  logs are 0600, refuse symlinks, and structurally redact URL secrets; agent snapshots redact
  every editable value and agent navigation cannot open local files, blobs, or custom schemes.
- Shared-memory frame objects are 0600, carry unique collision-checked names, are unlinked by
  the terminal or our guard, and are capped at four outstanding objects before direct fallback.
- The frame reader caps message size, so a bad length prefix cannot turn into a 4 GiB
  allocation.

Run [`tools/release-check.sh`](tools/release-check.sh), then close the manual terminal and
distribution gates in [`RELEASE.md`](RELEASE.md) before tagging anything.

See [`SECURITY.md`](SECURITY.md) for reporting vulnerabilities.

## Licence

MIT — see `LICENSE-MIT`. Third-party components and prior art studied (but not copied) are
listed in `NOTICE.md`. Notably the benchmark product, `zenbu-labs/terminal-browser`, ships
**no licence file**, so its implementation was treated as unavailable: only its public
behaviour informed this work.
