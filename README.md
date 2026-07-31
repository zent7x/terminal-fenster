# BlackGlass

A real Chromium browser that renders as **pixels inside your terminal** — not a text-mode
approximation, not a screenshot viewer. Chromium 150 renders offscreen; a Rust core turns
frames into terminal graphics and turns your keyboard and mouse back into browser input.

```
blackglass open news.ycombinator.com
```

## Status: working vertical slice, not shippable yet

This is honest about where it is. What follows is measured, not aspirational.

**Verified working**

- Real pages render in Ghostty via the Kitty graphics protocol, auto-detected.
- Click, hover, ordered typing, and scroll all reach the page — verified by reading actual
  frame pixels, not by trusting an event log (`tests/e2e/input-injection.js`, 9/9).
- The terminal is restored on exit, `ctrl+q`, panic, SIGINT/SIGTERM/SIGHUP.
- 101 unit tests, 9 end-to-end checks.

**Not done** — see [Known gaps](#known-gaps). The adversarial review
(`artifacts/swarm/F10-adversarial-report.md`) returned **NO-SHIP**, and it was right to.

## Requirements

- macOS or Linux. Developed and measured on macOS 26.1 / Apple M4.
- A terminal with the Kitty graphics protocol for full fidelity: **Ghostty**, **kitty**, or
  **WezTerm**. iTerm2 also speaks it. Anything else falls back to low-fidelity Unicode.
- Rust 1.80+, Node 18+.

## Build and run

```bash
git clone <repo> && cd blackglass
cargo build --release                    # builds the terminal core
cd apps/engine && npm install && cd ../.. # downloads Electron (~300 MB)

./target/release/blackglass doctor        # what your terminal can do
./target/release/blackglass open example.com
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
| `ctrl+r` | reload |
| `alt+left` / `alt+right` | back / forward |
| mouse | click, hover, drag, scroll — forwarded to the page |

## How it works

```
your terminal
     │  tty in raw mode, owned by an RAII guard that restores it on every exit path
[blackglass]        Rust: capability detection, compositor, input decoding, Kitty encoding
     │  unix socket, 0600, inside a 0700 dir — no network listener is ever opened
[engine host]       Electron: offscreen Chromium, sandbox ON
```

Frames are raw BGRA from Chromium's `paint` event. The core converts to RGB, deflates, and
transmits with the Kitty graphics protocol. Commands and events are JSON on the same socket;
frames are binary, length-prefixed, because a 5 MB bitmap through JSON would be indefensible.

Architecture decisions with their measurements and their reversal costs live in
[`docs/adr/`](docs/adr/). ADR-0001 records the engine choice *and* the argument it was
originally defended with, which later measurement proved false.

## Measured performance

Ghostty 1.3.1, macOS 26.1, Apple M4 (10 cores, 24 GB), release build.

| Page | Viewport | Frame in | Wire out | Ratio | Encode |
|---|---|---|---|---|---|
| example.com | 2482×814 | 8,081,424 B | 53,999 B | 112× | 2.61 ms |
| example.com | 1513×1073 | 6,493,828 B | 46,282 B | 105× | 8.24 ms |
| example.com | 714×481 | 1,373,768 B | 23,162 B | 44× | 1.89 ms |
| Hacker News | 2482×814 | 8,081,424 B | 292,833 B | 21× | 7.00 ms |

Time to first frame: 600 ms / 802 ms / 1576 ms across three runs on example.com; 3228 ms on
Hacker News including real network load.

Engine frame production sustains **60 fps** (p50 gap 16.65 ms, p99 19.94 ms) on a repainting
page — but note that is measured at the engine, not through the terminal. See gaps.

Reproduce with `benchmarks/`, or directly:

```bash
BLACKGLASS_LOG=/tmp/bg.log BLACKGLASS_EXIT_AFTER_MS=6000 \
  ./target/release/blackglass open https://example.com
```

## Terminal support

| Terminal | Graphics | Kitty keyboard | Pixel mouse | Status |
|---|---|---|---|---|
| Ghostty 1.3.1 | Kitty | yes | yes | **verified end-to-end** |
| iTerm2 3.6.9 | Kitty | yes | **no** (permanently reset) | protocol-verified, app not driven |
| Apple Terminal 465 | none → Unicode | no | no | capability-verified |
| kitty, WezTerm | Kitty | yes | yes | expected, **untested** |

Every "yes" above comes from the terminal answering a protocol query, not from matching
`$TERM`. Detection is in `crates/bg-term/src/caps.rs`.

Note the iTerm2 row: it reports SGR-Pixels mouse mode *permanently reset*, so coordinates
arrive as cells. Treating those as pixels would collapse the whole page into its top-left
corner — the pointer mapping handles both, with tests pinning the difference.

## Testing

```bash
cargo test                          # 101 unit tests, no terminal needed
node tests/e2e/input-injection.js   # 9 end-to-end input checks against real pixels
```

The e2e test speaks the engine wire protocol directly, so it runs without a graphics
terminal and works in CI. It asserts on frame pixels: a click must actually change the
colour under the cursor, and clicking one target must *not* activate another.

## Known gaps

Ordered by how much they matter.

1. **Damage tracking is unproven and unused.** The engine writes a dirty rect into every
   frame header and then sends the entire bitmap anyway. Worse, both original spikes forced
   full-viewport damage by construction, so we have never observed whether Chromium reports
   small rects at all. This is the largest performance gap and it gates the SSH story.
2. **SSH is designed but not implemented or measured.** No adaptive transport exists.
3. **Single tab.** No tab strip, omnibox, history, bookmarks, downloads, or profiles.
4. **No agent interfaces wired up.** CDP broker, MCP server, Playwright attach and the
   automation CLI are specified in `artifacts/swarm/` but not integrated.
5. **Sixel and iTerm2 backends are unimplemented.** If detection picks one, the CLI
   explicitly degrades to Unicode and `doctor` says so rather than silently faking it.
6. **`onPaint` copies each frame three times** (~1.45 GB/s at 60 fps) — B04's finding.
7. **No visual confirmation.** The development machine was locked, so `screencapture`
   returned black. Correctness is established by protocol handshake, byte arithmetic, and
   pixel assertions instead. A human should still look at it.

## Security posture

- Chromium's sandbox stays **on**; web content gets no Node integration and context
  isolation is enforced.
- Page-derived text (titles, URLs) is sanitized before it can reach the terminal — a
  malicious title otherwise smuggles escape sequences through us and can drive OSC 52 to
  overwrite your clipboard. C0, C1, DEL and separators are all stripped.
- The control socket is 0600 inside a 0700 directory. No port is opened. There is no CDP
  endpoint exposed today.
- The frame reader caps message size, so a bad length prefix cannot turn into a 4 GiB
  allocation.

See `artifacts/swarm/A09-threat-model.md` and `F01-security-review.md`.

## Licence

MIT — see `LICENSE-MIT`. Third-party components and prior art studied (but not copied) are
listed in `NOTICE.md`. Notably the benchmark product, `zenbu-labs/terminal-browser`, ships
**no licence file**, so its implementation was treated as unavailable: only its public
behaviour informed this work.
