# A02 — Competitor Matrix: Terminal & Keyboard-First Browsers

**Mission:** A02 · **Date:** 2026-07-31 · **Analyst:** recon agent
**Target env (given, not re-derived):** macOS 26.1, Apple M4 arm64, Ghostty 1.3.1 / iTerm2 3.6.9 / Apple Terminal 465, Rust 1.93.0, Node 24.11.1.
**Verified locally this session:** `TERM=xterm-256color`, `TERM_PROGRAM=Apple_Terminal`, `TERM_PROGRAM_VERSION=465`, `COLORTERM=truecolor`; `/Applications/Ghostty.app` CFBundleShortVersionString = `1.3.1`; `rustc 1.93.0 (254b59607 2026-01-19)`; `node v24.11.1`.

---

## 0. Executive orientation

There are exactly **two** viable ways to put real web pixels in a terminal, and every project below picks one:

| Technique | Fidelity | Terminal support | Representative |
|---|---|---|---|
| **A. Glyph-cell approximation** — Unicode block/quadrant glyphs + SGR color | ≤ 2 colors per cell; ~2×2 "pixels"/cell | **Universal** (any ANSI terminal, incl. Apple Terminal, SSH, tmux) | Carbonyl, Browsh |
| **B. Real raster protocol** — Kitty graphics / Sixel / iTerm2 OSC 1337 | True per-pixel | **Fragmented** — see §5 | terminal-browser, casty, awrit, brow6el, w3m |

The single most consequential environmental fact for Terminal-Fenster: **Ghostty supports the Kitty graphics protocol and explicitly refuses Sixel; iTerm2 supports Sixel and OSC 1337 but not Kitty graphics; Apple Terminal supports neither.** No single raster protocol covers the three target terminals. Details and citations in §5.

---

## 1. Comparison matrix

| Project | Engine | Draw technique | Input | License | Latest / last activity | Install | Platforms | Status |
|---|---|---|---|---|---|---|---|---|
| **Carbonyl** | Chromium **111.0.5511.1** fork (14 patches) | **A** — Unicode quadrant glyphs + SGR 24-bit/256 | Raw termios + SGR mouse (`?1003`/`?1006`) | **BSD-3-Clause** | v0.0.3 (2023-02-18); last commit `ab80a276` **2023-02-26** | npm `carbonyl`, Docker `fathyb/carbonyl`, zip | Linux, macOS, Win11/WSL | ☠️ **Dead ~3.5 yrs** |
| **brow6el** | **CEF** (Chromium Embedded) | **B** — Sixel (tiled or monolithic) **+** Kitty, auto-detect | Vim modal (4 modes) + grid-jump | **MIT** (CEF BSD-3, libsixel MIT) | **v0.3.5, ~2026-07-27** | `download_cef.sh` (~670 MB) + `build.sh`; AUR/Debian pkgs | Linux (Ubuntu 25.10, Debian 13, Arch) | ✅ **Active**, self-described "POC code quality" |
| **terminal-browser** (zenbu-labs) | **Electron** offscreen rendering (GPU readback) | **B** — Kitty graphics only | Terminal events + **Swift OS-level helper** for trackpad/momentum | ⛔ **NONE** (all rights reserved) | created 2026-07-06, pushed **2026-07-30**, 400★ | `curl -fsSl https://terminal-browser.sh/install \| bash` | **macOS only** (Linux PR open) | ✅ **Very active** — closest competitor |
| **casty** | `chrome-headless-shell` over raw **CDP** | **B** — Kitty graphics | CDP `Input.*` synthesis; `CSI 14t` for pixel dims | **MIT** | pushed 2026-03-25, 23★ | `npm i -g @sanohiro/casty` | Node ≥18; Ghostty/kitty | ✅ Active-ish |
| **herdr-browser** | Chromium over **CDP** in a "Herdr pane" | **B** (CDP-driven) | CDP | **MIT** | created 2026-07-26, pushed 2026-07-28, 218★ | npm/TS | TypeScript | 🆕 **Days old** |
| **awrit** | **CEF** | **B** — Kitty graphics (kitty ≥ 0.31) | kitty mouse + keyboard | **BSD-3-Clause** | **ARCHIVED 2026-04-25**, 1383★ | `awrit [url]` | Linux (macOS → "use cmux") | ☠️ **Archived w/ security warning** |
| **Browsh** | **Headless Firefox** via Marionette :2828 | **A** — `▄` U+2584 half-block, `tcell` 24-bit | tcell events → webextension | **LGPL-2.1** | v1.8.3 2024-01-29; last commits 2025-07 (CI/docs only) | binary ~11 MB + Firefox; Docker `browsh/browsh` ~230 MB | Linux, macOS, Windows | 🟡 **Dormant** |
| **w3m** | **Own** HTML renderer (no JS/CSS) | **B** — Sixel via `img2sixel`, or `w3mimgdisplay` X11 blit | Pager keybinds | w3m license (GitHub: `NOASSERTION`) | pushed 2024-08-19 (Debian tree) | `apt install w3m w3m-img` | POSIX | 🟡 Maintenance-only |
| **Lynx** | **Own** text renderer | **None** (text only) | Pager keybinds | **GPL-2.0** | **2.9.3, 2026-05-26** | `brew install lynx` | POSIX | ✅ Actively maintained |
| **Nyxt** | **Electron** (`cl-electron`); GTK/WebKit renderers still present | **N/A — GUI window** | Emacs/vi/CUA, keyboard-driven | **BSD-3-Clause** | **4.0.0, 2026-01-18** | Flathub, distro pkgs | Linux, macOS, FreeBSD | ✅ Active |
| **Vieb** | **Electron 43.1.1** | **N/A — GUI window** | Vim bindings | **GPL-3.0-or-later** | **12.10.0, 2026-07-19** | npm / releases | Linux, macOS, Windows | ✅ Very active |

> **Nyxt and Vieb are not terminal browsers.** They are keyboard-first GUI browsers. They are in the matrix because they define the *interaction* bar (modal editing, hinting, command palette) that Terminal-Fenster will be judged against, not the *rendering* bar.

---

## 2. Carbonyl — deep dive (the critical target)

Repo: `github.com/fathyb/carbonyl`. Author: Fathy Boundjadj. 19,309★.

### 2.1 License — verified carefully

`license.md` is verbatim **BSD-3-Clause**, `Copyright © 2023, Fathy Boundjadj`. `package.json` declares `"license": "BSD-3-Clause"`. GitHub API returns `spdx_id: "BSD-3-Clause"`.

**Caveat that matters:** the BSD-3 grant covers only *Carbonyl's own* Rust + C++ glue (`src/`) and the patch files. The shipped **runtime is a patched Chromium 111**, which carries Chromium's own BSD-3-Clause plus its enormous third-party license set — and `chromium/src/browser/args.gn` sets `ffmpeg_branding = "Chrome"` and `proprietary_codecs = true`, which pulls in **H.264/AAC patent-encumbered decoders**. Redistributing a Carbonyl-derived binary is a codec-licensing question, not just a copyright one. BSD-3 also carries a **no-endorsement clause** — we may not use "Carbonyl" or the author's name to promote Terminal-Fenster.

**Verdict:** Carbonyl's own source is permissively licensed and *legally* copyable with attribution. We should still not copy it — see §6.

### 2.2 Maintenance status — it is dead, and the GitHub metadata lies

- `pushed_at` = `2024-07-01T14:31:50Z` — **misleading**. A `pushed_at` bump can come from tag/branch metadata.
- **Branch list: exactly one branch, `main`, at `ab80a276`.**
- `ab80a276` = `fix(build): link to Chromium sysroot libs on Linux (#134)`, **2023-02-26T21:31:10Z**.
- Latest (only recent) release: **v0.0.3, 2023-02-18**. Assets: `carbonyl.{linux,macos}-{amd64,arm64}.zip`, ~68–80 MB each, 47,191 downloads on linux-amd64.
- **89 open issues**, zero commits in ~3.5 years. No `.github/workflows` remain reachable.

**Why it is unmaintained (assessment, flagged as inference):** the repo carries **14 Chromium patches + 2 Skia patches + 1 WebRTC patch**, each pinned to exact upstream SHAs in `scripts/patches.sh`:

```bash
chromium_upstream="92da8189788b1b373cbd3348f73d695dfdc521b6"   # 111.0.5511.1
skia_upstream="486deb23bc2a4d3d09c66fef52c2ad64d8b4f761"
webrtc_upstream="727080cbacd58a2f303ed8a03f0264fe1493e47a"
```

The patches (`0001-Add-Carbonyl-library` … `0014-Move-Skia-text-rendering-control-to-bridge`) touch Viz, Skia's device layer, Blink's `StyleResolver`, and `display::Display`. Chromium ships a major version roughly every 4 weeks; every roll risks breaking all 17 patches. The README concedes the runtime "takes more than an hour to build from scratch," needs "~100 GB of disk space," and "Building Chromium for arm64 on Linux requires an amd64 processor." That is an unfundable maintenance burden for one person. Chromium 111 shipped stable in March 2023; **Carbonyl is ~35 major Chromium versions behind and carries every browser CVE since.**

**This is the single most important strategic fact in this report: the graveyard is full of Chromium *forks*. Every actively-maintained competitor (brow6el, terminal-browser, casty, herdr) uses an *embedding* API — CEF, Electron OSR, or CDP — precisely so upstream can roll without them.**

### 2.3 How Carbonyl actually draws — VERIFIED FROM SOURCE

**It uses no graphics protocol at all.** No Sixel, no Kitty, no iTerm2 OSC. It is pure Unicode glyphs + SGR color, which is why it works over SSH into a plain `xterm-256color`.

> ⚠️ **Correction to the widely-cited blog post.** `fathy.fr/carbonyl` describes half-block `U+2584` rendering. That describes an *older* build. Commit `67474982 feat(browser): introduce quadrant rendering (#120)` (2023-02-14) replaced it. **Current `main` uses quadrant glyphs.** Trusting the blog post here would cost hours.

**Pipeline (from `src/output/renderer.rs::draw_background`):**

1. Chromium rasterizes into a shared-memory RGBA8888 framebuffer of `(cols × 2) × (rows × 4)` pixels. Sanity check in source: `pixels.len() < viewport.width * viewport.height * 8 * 4` (8 = 2×4 px/cell, 4 = bytes/px).
2. Vertical 2:1 box average → a **2×2 quadrant** per cell:
   ```rust
   let pair = |x, y| pixel(x, y).avg_with(pixel(x, y + 1));
   cell.quadrant = (pair(x+0, y+0), pair(x+1, y+0), pair(x+1, y+2), pair(x+0, y+2));
   ```
3. `src/output/quad.rs::binarize_quandrant` collapses 4 colors → 1 glyph + fg + bg:
   - Luma weights `Color::new(0.299, 0.587, 0.114)` (Rec.601).
   - Threshold = `min + (max - min) / 2.0` (midpoint, **not** mean).
   - 16-case match on the 4-bit mask → glyph ∈ `▄ ▖ ▗ ▝ ▘ ▞ ▐ ▌ ▚`, with the two color groups averaged.
4. **Text is not rasterized** — it is captured as real glyphs and re-emitted (see §2.5), so a `Cell` carries `Option<Rc<Grapheme>>` that overrides the quadrant.

**Exact escape sequences (byte-accurate, from `src/output/painter.rs`, `src/input/tty.rs`, `src/output/renderer.rs`):**

| Purpose | Sequence | Bytes |
|---|---|---|
| Frame begin (hide cursor + disable blink) | `\x1b[?25l\x1b[?12l` | `1B 5B 3F 32 35 6C` … |
| Cursor position (1-based) | `\x1b[{row};{col}H` | CSI … `48` |
| FG truecolor | `\x1b[38;2;{r};{g};{b}m` | |
| BG truecolor | `\x1b[48;2;{r};{g};{b}m` | |
| FG 256 | `\x1b[38;5;{code}m` | |
| BG 256 | `\x1b[48;5;{code}m` | |
| Frame end (restore cursor) | `\x1b[{y};{x}H\x1b[?25h\x1b[?12h` | |
| Window title (all three) | `\x1b]0;{t}\x07`, `\x1b]1;{t}\x07`, `\x1b]2;{t}\x07` | OSC + BEL `07` |

**Startup / teardown** — `SEQUENCES: [(1049, true), (1003, true), (1006, true), (25, false)]`, emitted as `\x1b[?{n}h` when `true` and `\x1b[?{n}l` when `false`; teardown emits the inverse:

```
\x1b[?1049h   # enter alternate screen buffer
\x1b[?1003h   # xterm ANY-EVENT mouse tracking (incl. bare motion)
\x1b[?1006h   # SGR extended mouse coordinates (>223 cols safe)
\x1b[?25l     # hide cursor
```

**Capability detection** — two DCS queries at startup:

```
\x1b[48;2;0;0;0m     # set BG to a known truecolor value
\x1bP$qm\x1b\\        # DECRQSS(SGR): if reply echoes 48;2;… → truecolor confirmed
\x1bP+q544e\x1b\\     # XTGETTCAP for hex 544E == "TN" == terminal name
```
Fallback: env `COLORTERM ∈ {"truecolor", "24bit"}` (`src/output/painter.rs`).

**256-color quantization** (`src/output/xterm.rs::to_xterm`) — grayscale ramp when `max_val() - min_val() < 8`, else the 6×6×6 cube via a fused multiply-add and dot product with `(36.0, 6.0, 1.0)`:
```rust
let scale = 5.0 / 200.0;
(16.0 + self.cast::<f32>().mul_add(scale, scale * -55.0).max(0.0).round().dot((36.0, 6.0, 1.0))) as u8
```

**Mouse input** (`src/input/mouse.rs`) — parses SGR form `\x1b[<{btn};{col};{row}M` (press/motion) and `…m` (release). Button-mask bits: `MouseMove = 0x20`; scroll up/down decoded from higher mask bits before the button branch. Coordinates are converted 1-based → 0-based.

**TTY setup** (`src/input/tty.rs`) — `libc::cfmakeraw()` but **`c_oflag` is restored afterward** ("ensures carriage returns are consistent"); falls back to opening `/dev/tty` when `isatty(STDIN_FILENO) != 1`; restores via `tcsetattr(…, TCSANOW, …)` in `Drop`.

### 2.4 Damage tracking & frame pacing

`Renderer::render` keeps `cells: Vec<(Cell, Cell)>` — a **(previous, current) double buffer** — and skips any cell where `current == previous`. `Painter` additionally suppresses redundant `cursor`, `background`, and `foreground` SGR writes, and buffers the whole frame into a `Vec<u8>` flushed **once** per frame. `src/output/frame_sync.rs` computes `frame_duration = Duration::from_micros((1_000_000.0 / fps) as u64)` and returns a deadline of `render_start + frame_duration`, so an over-budget frame yields a past deadline and renders immediately (no death spiral).

### 2.5 Chromium integration (from the author's writeup, `fathy.fr/carbonyl`)

- **Framebuffer:** custom `carbonyl::HostDisplayClient` swapped in at `VizProcessTransportFactory::OnEstablishedGpuChannel()`; implements `HostDisplayClient` + `SoftwareOutputDevice` + a `LayeredWindowUpdater` receiving pixels via `OnAllocatedSharedMemory()`. Author reports this took steady-state CPU from **~400% to ~15%** during scroll by eliminating IPC copies.
- **Text capture:** a Skia device `TextCaptureDevice : SkClipStackDevice` overrides `onDrawGlyphRunList()`, converts glyph IDs with `SkFontPriv::GlyphsToUnichars()`, and overrides `drawRect()`/`drawRRect()` to clear occluded text. Skia's own text raster is patched off.
- **Transport:** Mojo interface `CarbonylRenderService::DrawText()` carrying `TextData{utf8, bounds, color}`; renderer side reaches `blink::WebViewImpl::GetPaintRecord()` via a private cast and replays the record.
- **The 49× trick:** forced device scale factor **`1.0 / 7.0`** via `Display::GetForcedDeviceScaleFactor()` / `HasForceDeviceScaleFactor()` — 7×7 = 49 CSS px rasterize to one terminal block. This is the actual source of the performance win, not clever encoding.
- **Forced monospace:** `StyleResolver::ResolveStyle()` patched to apply `SetComputedSize(11.75 / 7.0)`, `SetGenericFamily(FontDescription::kMonospaceFamily)`, `SetLineHeight(Length::Fixed(14.0 / 7.0))` to **all** elements.

### 2.6 Performance & limitations

**Claims (README):** "starts in less than a second, runs at 60 FPS, and idles at 0% CPU usage"; works without a window server and over SSH. vs Browsh: "50x more CPU power is needed for the same content in average … Carbonyl does not downscale or copy the window framebuffer, it natively renders to the terminal resolution." Marked **UNVERIFIED** — not benchmarked this session (v0.0.3 arm64 macOS binary is 3.5 yrs old and Chromium-111-era).

**Limitations:** no fullscreen mode (README); ≤2 colors per cell (inherent to technique A); forced-monospace + 1/7 DSF means pages are **laid out differently from a real browser** — this is not a faithful rendering; `--no-sandbox` in the Docker entrypoint; Chromium 111 = unpatched CVEs; build requires ~100 GB and ~1 h; arm64 Linux cross-build requires an amd64 host.

---

## 3. brow6el — the most technically relevant *active* project

Moved **Codeberg → Tangled** (`tangled.org/janantos.tngl.sh/brow6el`); Codeberg mirror currently returns **HTTP 503**. v0.3.5, 156 commits, 7 tags, 7★ — small but exactly on-target.

- **License: MIT.** Uses CEF (BSD-3-Clause) and libsixel (MIT). All permissive.
- **Engine:** CEF. `./download_cef.sh` pulls ~670 MB of prebuilt binaries — **no Chromium build, no patches.** This is why it can stay current.
- **Sixel, tiled (the good idea):** tiles dynamically sized to ~30–40 per screen, **aligned to terminal cell boundaries** to avoid artifacts, and **only tiles intersecting CEF's dirty rects are redrawn**. Runtime toggle `z` (tiled↔monolithic), `Z` (force full redraw). Reported to reduce flicker significantly and be faster for typing-heavy workflows.
- **Kitty path:** always monolithic + **double buffering**; whole screen encoded RGBA → base64; **30 FPS**; images placed at **`z=-1`** so text dialogs composite on top.
- **Protocol selection:** `browser.conf` → `auto` (default) | `sixel` | `kitty`.
- **Modal input:** STANDARD `[S]` / INSERT `[I]` / VISUAL `[V]` / MOUSE `[M]`, mode shown in status bar. `i`→insert, `e`→mouse, `v`→visual, `f`→link hints, `u`→URL, `+`/`-`/`0`→zoom.
- **Grid Jump:** recursive adaptive **3×3** grid, default label keys `qweasdzxc`, configurable via `grid_keys`. **3–4 keystrokes to almost any element.** Green grid = more zoom levels available, red = max.
- **Extras:** inspect mode (cyan border + tag/id/class/attr/dims panel), user scripts (`view-source.js`, `reader-mode.js`, `adblock.js`, `force-light-mode.js`, `frameset-redirect.js`), DoH (`secure`/`automatic`/`off`; Cloudflare/Google/Quad9), proxy HTTP/HTTPS/SOCKS4/SOCKS5.
- **Auto-zoom formula:** adaptive blend of terminal cell size (60% weight) and window resolution (40%).
- **Limitations (author-stated):** "POC code quality"; **Kitty over SSH is "barely usable"** — "responsiveness significantly affected due to massive data transfers on each frame rendered"; Ubuntu 24.04's kitty v0.32.2 mishandles the newer image protocol and **prints base64 instead of rendering**.
- Tested on Ubuntu 25.10 / Debian 13 / Arch; terminals foot, wezterm, kitty, ghostty, yaft, Windows Terminal via WSL2.

---

## 4. terminal-browser (zenbu-labs) — the closest architectural competitor

`github.com/zenbu-labs/terminal-browser` · `terminal-browser.com` · Rust · 400★ · created **2026-07-06** · pushed **2026-07-30**.

### ⛔ LICENSE: NONE

The repository has **no license file and GitHub reports `license: null`**. Under default copyright, **all rights are reserved**. Despite being publicly readable, this is the *most* restrictive project in the matrix — we may not copy any of it, and should not even read it closely while writing equivalent code. Architectural facts below come from its README (§ "How it works") and its public file listing only.

### Architecture (from README + public tree)

- **Rendering:** **Electron's offscreen rendering API** to "read pixels generated by chromium **directly from the GPU**," pushed to the terminal via the **Kitty graphics protocol**. No fork, no CEF build — just Electron. This is the lowest-effort path to a current Chromium.
- **Compositor:** a Rust engine crate `pixel-core` (`canvas.rs`, `compositor.rs`, `paint.rs`, `surfaces.rs`, `kitty.rs`, `ghostty.rs`, `tmux.rs`, `image_cache.rs`, `throttle.rs`) plus `pixel-node` NAPI bindings (`iosurface.rs` ⇒ macOS **IOSurface** zero-copy GPU handoff, `diff.rs`, `mend.rs`).
- **Chrome UI:** defined in **React via a custom React reconciler** (`packages/pixel-react`, `reconciler-config.ts`) that renders into the *same* Rust canvas as the page content — so browser chrome and web content composite as layers in one image. It ships its own in-terminal devtools (`devtools/fiber-hook.ts`, `profiler.tsx`).
- **Input — the standout idea:** terminal-sourced mouse clicks/position/keys are converted to synthetic Chromium events; **but for what the terminal cannot express (smooth scrolling, trackpad momentum/pixel-delta), a background Swift app reads OS input events non-intrusively** (`native-scroll-helper.swift`, `native_pairing.rs`, `scroll/profiles/{glide,smooth,tui}.rs`). README: "websites with infinite canvases work great inside terminal-browser."
- **Platform:** **macOS only**; Linux "in progress" via open PR. Terminals: Ghostty, kitty, cmux, VSCode, "many more."
- **Install:** `curl -fsSl https://terminal-browser.sh/install | bash`. CLI: `terminal-browser open <url>`, `--split right`, and an **agent-compatible `terminal-browser action`** subcommand.
- Bundles `assets/fonts/JetBrainsMono-Regular.ttf`; repo contains `CLAUDE.md`/`AGENTS.md` (AI-assisted build).

---

## 5. Protocol reality check for our three target terminals

This is the constraint that should drive Terminal-Fenster's rendering architecture.

| Terminal | Kitty graphics | Sixel | iTerm2 OSC 1337 | Kitty keyboard |
|---|---|---|---|---|
| **Ghostty 1.3.1** | ✅ **Yes** | ❌ **No — explicit non-goal** | ❌ | ✅ Yes |
| **iTerm2 3.6.9** | ❌ | ✅ **Yes, since 3.3.0** | ✅ **Native** | ✅ Yes |
| **Apple Terminal 465** | ❌ | ❌ ("could not find any sixel references in documentation") | ❌ | ❌ |
| kitty | ✅ | ❌ (deliberate) | ❌ | ✅ |
| WezTerm | ✅ | ✅ (since `20200620-160318-e00b076c`) | ✅ | ✅ |
| foot | ✅ | ✅ (since 1.2.0) | ❌ | ✅ |
| xterm | ❌ | ✅ (default since patch #359) | ❌ | ❌ |

**Consequence:** Terminal-Fenster needs **three** backends to cover its own stated test matrix — Kitty graphics (Ghostty), Sixel *or* OSC 1337 (iTerm2), and a glyph-cell fallback (Apple Terminal, plain SSH, tmux). There is no shortcut.

### 5.1 Kitty graphics protocol — exact wire format

Envelope is APC: `ESC _ G <control kv-pairs> ; <base64 payload> ESC \` → bytes `1B 5F 47` … `1B 5C`.

Key control keys: `a` action (`T` transmit+display, `p` place, `d` delete, `f` frame, `q` query) · `f` format (`24` RGB, `32` RGBA default, `100` PNG) · `s`/`v` pixel width/height · `t` medium (`d` direct, `f` file, `t` temp file, `s` **shared memory**) · `m` chunking (`1` more, `0` last) · `i` image id · `p` placement id · `c`/`r` display cols/rows · `x,y,w,h` source rect · `X,Y` intra-cell offset · `z` z-index (**negative = below text**) · `C=1` don't move cursor · `o=z` zlib · `q=1|2` suppress responses · `U=1` unicode placeholder.

Support detection (send, then read before the DA reply):
```
\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c
```
Success reply `\x1b_Gi=31;OK\x1b\\`; **no reply before the `\x1b[c` Device Attributes response ⇒ unsupported.** Errors come back as `ENOENT:`/`EINVAL:`.

Delete: `\x1b_Ga=d\x1b\\` (all visible), `\x1b_Ga=d,d=i,i=10\x1b\\` (by id), `\x1b_Ga=d,d=Z,z=-1\x1b\\` (by z). **Uppercase delete letters free the image data; lowercase keep it cached** — critical for a 60 FPS loop.

### 5.2 Kitty keyboard protocol — exact wire format

- Query: `CSI ? u` → reply `CSI ? <flags> u`
- Set: `CSI = <flags> ; <mode> u` (mode 1 = set-and-reset-others, 2 = set-only, 3 = reset-only)
- **Push/pop (use these — they're restorable on crash):** `CSI > <flags> u` / `CSI < <n> u`
- Flags: `1` disambiguate escapes · `2` report event types · `4` report alternate keys · `8` report all keys as escapes · `16` report associated text
- Event encoding: `CSI <unicode-key>:<alt-keys> ; <modifiers>:<event-type> ; <text-codepoints> u`
- Modifiers are **`1 + bitmask`**: shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32, caps 64, num 128
- Event types: press 1 (default/omitted), **repeat 2, release 3** ← the only way to get keyup in a terminal

### 5.3 iTerm2 inline images — exact wire format

```
ESC ] 1337 ; File = [key=value;…] : <base64 file bytes> BEL
```
`BEL` = `0x07`, or ST (`ESC \`). Args: `name` (base64 filename), `size` (bytes), `width`/`height` (`N` cells | `Npx` | `N%` | `auto`), `preserveAspectRatio` (0|1, default 1), `inline` (0|1, **default 0 — must set `inline=1`**). tmux-safe multipart since 3.5: `OSC 1337 ; MultipartFile=…BEL`, then repeated `OSC 1337 ; FilePart=<b64> BEL`, then `OSC 1337 ; FileEnd BEL`.

---

## 6. What we should steal — conceptually, not as code

**Legal preamble.** Copy-safe if attributed: Carbonyl, awrit, Nyxt (**BSD-3-Clause**); brow6el, casty, herdr, tcell (**MIT/Apache-2.0**). Do **not** copy: Browsh (**LGPL-2.1** — dynamic linking only, no source lifting), Vieb (**GPL-3.0**), **terminal-browser (no license at all — most restrictive despite being public)**. Recommendation: **copy nothing.** Every item below is an architectural idea, independently reimplementable, and several are simply "read the spec."

**1. Do not fork Chromium. Embed it.** The strongest signal in this entire matrix: the one project that forked (Carbonyl) is dead at Chromium 111 with 89 open issues, and every living competitor embeds instead — CEF (brow6el, awrit), **Electron offscreen rendering** (terminal-browser), or CDP screencast (casty, herdr). With Electron already installing in our environment, **Electron OSR + `webContents.setFrameRate()`** gives us a current, auto-patched Chromium and a `paint` event with dirty rects for free. Carbonyl's 3.5-year death is the cost of the alternative.

**2. Ship three rendering backends with runtime probing, ranked Kitty → Sixel/OSC1337 → glyph-cell.** Non-negotiable given §5: Ghostty has no Sixel, iTerm2 has no Kitty, Apple Terminal has neither. Probe in this order at startup — Kitty query + `CSI c` race (§5.1), then `DA1`/`XTGETTCAP` for Sixel, then `TERM_PROGRAM` for iTerm2 OSC 1337, then fall back. **The glyph-cell fallback is not a toy** — it is the only thing that works over plain SSH, in tmux, and in Apple Terminal, and it is what made Carbonyl famous.

**3. Steal brow6el's tiled-Sixel dirty-rect scheme, and generalize it to Kitty.** Tiles sized to ~30–40 per screen, **snapped to terminal cell boundaries**, redrawing only tiles that intersect the engine's dirty rects. Then go further than brow6el, which admits its Kitty path is always monolithic and "barely usable over SSH … massive data transfers on each frame": apply the same tiling to Kitty using per-tile image IDs with `a=T` + `z`, and use lowercase deletes to keep tiles cached. **Full-frame base64 per frame is the #1 performance failure mode in this space — three separate projects hit it.**

**4. Use Kitty's `t=s` shared-memory transmission, not `t=d` base64.** Base64 inflates every frame by 33% and forces a full memcpy through the PTY. `t=s` hands the terminal a POSIX shm name. This is the single highest-leverage optimization available and no competitor in this matrix is documented as using it.

**5. Carbonyl's real speed trick was resolution, not encoding — but its cost was fidelity.** The `1.0/7.0` forced device scale factor plus forced-monospace `StyleResolver` patching bought ~49× rasterization savings, and that is why it hits 60 FPS. But it means Carbonyl **does not render the web faithfully** — layout differs from a real browser. Terminal-Fenster's differentiator should be the opposite bet: real pixels at real DPI via a raster protocol, with `zoom`/DSF as a *user-facing* control rather than a hidden 1/7 hack.

**6. Steal terminal-browser's OS-level input side-channel.** Terminal mouse protocols cannot express trackpad momentum or pixel-delta scroll — `\x1b[?1003h`/`?1006` give discrete button-4/5 clicks only. A non-intrusive native helper (they use Swift on macOS; a `CGEventTap` at `.listenOnly`) reading scroll phase and delta is the difference between "terminal browser" and "browser that happens to be in a terminal." Combine with `CSI > 5 u` (kitty keyboard, all flags) for **key-release events** — otherwise no web game, drag gesture, or modifier-held interaction works.

**7. Adopt Carbonyl's frame discipline verbatim in spirit.** Three cheap, high-value patterns, all independently obvious once seen: (a) `(previous, current)` cell double-buffer, skip unchanged; (b) suppress redundant SGR/cursor writes by tracking last-emitted state; (c) buffer the entire frame into one `Vec<u8>` and issue exactly **one** `write` + `flush` per frame. And `FrameSync`'s deadline-in-the-past behavior — when a frame overruns, render immediately rather than sleeping — avoids the classic pacing death-spiral.

**8. Steal brow6el's Grid Jump for mouse-free pointing.** Recursive 3×3 grid with `qweasdzxc` labels reaching any pixel in 3–4 keystrokes, with color signalling remaining zoom depth. This solves the problem link-hinting (Vieb/Nyxt style) cannot: positioning a *cursor* on a canvas, map, or drag target. Ship **both** — hints for links, grid-jump for arbitrary points.

**9. Steal the modal model from brow6el/Vieb/Nyxt, with an always-visible mode indicator.** STANDARD/INSERT/VISUAL/MOUSE with `ESC` to return, mode rendered in the status bar. This is the settled convention in keyboard-first browsing; deviating from it costs users more than any novelty gains.

**10. Steal casty's two-tier capture and CDP hygiene.** Low-resolution screencast purely as a *change detector*, then a high-resolution grab of only what changed; adaptive codec (fast/lossy while scrolling or playing video, lossless once idle). Also its two hard-won operational notes: **do not send `Runtime.enable`** (it breaks Google login), and **tmux requires `allow-passthrough on`** or every graphics escape is swallowed.

**11. Ship a first-class agent CLI from day one.** terminal-browser exposes `terminal-browser action`; brow6el exposes a user-script injection system. A browser living in a terminal is where coding agents already are — this is a differentiator competitors are only just discovering, and it costs little to design in early.

**12. Learn from awrit's obituary.** Archived 2026-04-25 with the maintainer citing "the rising number of security issues." A CEF/Chromium embedder that falls behind becomes a liability, not just stale software. **Whatever embedding we choose must have an automated upgrade path in CI from week one**, or Terminal-Fenster joins Carbonyl and awrit.

---

## 7. Unverified / open items

- **UNVERIFIED:** Carbonyl's "60 FPS / 0% idle CPU / 50× faster than Browsh" claims — not benchmarked here; the only binary is a 3.5-year-old Chromium-111 build.
- **UNVERIFIED:** brow6el's exact CEF/Chromium version — README does not state it; would require running `download_cef.sh`.
- **UNVERIFIED:** Ghostty 1.3.1's Kitty-graphics *completeness* (specifically Unicode-placeholder `U=1` support and `t=s` shared-memory support). Ghostty's own docs confirm only "supports the Kitty graphics protocol." **This must be probed empirically before committing to `t=s` — it is the load-bearing assumption behind recommendation #4.**
- **UNVERIFIED:** whether Browsh's headless Firefox path still works against current Firefox (last functional commit 2023-12; `firefoxMarionette()` dials `127.0.0.1:2828` after `--marionette`).
- **UNVERIFIED:** herdr-browser's internals — repo is days old; only its description ("Render a real Chromium view inside a Herdr pane and drive it over CDP") and MIT license were confirmed.
- **Note:** Codeberg (`codeberg.org/janantos/brow6el`) returned HTTP 503 throughout this session; brow6el data comes from the Tangled mirror the author migrated to, plus LinuxLinks coverage dated 2026-03-04.

---

## 8. Sources (primary unless noted)

**Carbonyl** — `github.com/fathyb/carbonyl` · source read directly via `raw.githubusercontent.com/fathyb/carbonyl/main/`: `src/output/{painter,renderer,cell,quad,xterm,frame_sync}.rs`, `src/input/{tty,mouse}.rs`, `license.md`, `package.json`, `chromium/.gclient`, `scripts/patches.sh`, `src/browser/args.gn`, `readme.md` · GitHub REST API (`/repos`, `/branches`, `/commits`, `/releases/latest`, `/git/trees`) · author's writeup `fathy.fr/carbonyl`
**brow6el** — `tangled.org/janantos.tngl.sh/brow6el` · `codeberg.org/janantos/brow6el` (503) · `linuxlinks.com/brow6el-terminal-web-browser-graphics-support/` (secondary, 2026-03-04)
**terminal-browser** — `github.com/zenbu-labs/terminal-browser` README + `/git/trees/main?recursive=1` + `/repos` · `terminal-browser.com`
**casty** — `github.com/sanohiro/casty` · npm `@sanohiro/casty`
**awrit** — `github.com/chase/awrit` (archived)
**herdr-browser** — `github.com/ogulcancelik/herdr-browser`
**Browsh** — `github.com/browsh-org/browsh` · source: `interfacer/src/browsh/{frame_builder,tty,firefox}.go`, `interfacer/go.mod` · `brow.sh`
**w3m** — `github.com/tats/w3m/blob/master/doc/README.sixel` · libsixel `saitoha.github.io/libsixel/`
**Lynx** — `lynx.invisible-island.net/current/CHANGES` (2.9.3, 2026-05-26)
**Nyxt** — `github.com/atlas-engineer/nyxt` · `nyxt.asd`, `source/renderer/electron.lisp` (SPDX headers)
**Vieb** — `github.com/Jelmerro/vieb` · `package.json`
**Protocols** — `sw.kovidgoyal.net/kitty/graphics-protocol/` · `sw.kovidgoyal.net/kitty/keyboard-protocol/` · `iterm2.com/documentation-images.html` · `arewesixelyet.com` · `ghostty.org/docs/about` · `github.com/ghostty-org/ghostty/discussions/{2496,5886}`
