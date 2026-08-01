# Beating terminal-browser.com — gap analysis & prioritized roadmap

**Goal (from the brief):** beat terminal-browser.com on *every* aspect, and run smoothly on
low-RAM / low-end machines (no lag, good fps) across macOS/Linux.

**Date:** 2026-08-01 · living document · collision-free (new file; edit freely).

This exists so the swarm optimizes toward a named target instead of a vibe. It enumerates the
competitor's advertised surface, scores Terminal-Fenster against each aspect, and turns the gaps into
owned, codeable tasks. Update the scores as work lands.

---

## 1. What terminal-browser.com actually advertises

Fetched from the product site 2026-08-01. It is deliberately sparse on numbers.

| Aspect | What they claim |
|---|---|
| Rendering | "A real browser scoped to your terminal tab" via **libghostty** |
| Terminals | Ghostty, Kitty, cmux, WezTerm, **"+50 more"** (libghostty-backed) |
| OS | macOS, Linux |
| Install | one-liner: `curl -fsSL https://terminal-browser.sh/install \| bash` |
| Remote | view sites on remote machines over **SSH, "no port forwarding"** |
| Layout | **split-pane** alongside agent workflows (`--split right`) |
| Multi-session | `terminal-browser ls` lists open browsers |
| Agent-native | `terminal-browser action` — an **agent-browser-compatible CLI**; "test the checkout flow" |
| Commands | launch · `open <url>` · `--split` · `ls` · `action` |
| **Performance** | **no fps, memory, latency, or bandwidth claims anywhere** |
| Tabs / history / bookmarks | **not advertised** (neither product has them) |
| Pricing | none stated |

**The strategic read:** their moat is breadth (50+ terminals via libghostty) and agent-native
ergonomics. Their exposed flank is that they publish **no performance story at all.** Terminal-Fenster
has done the damage-tracking work (B02 → C08) to *have* one. Performance is where we win outright;
everywhere else is parity work.

---

## 2. Head-to-head scorecard

Legend: 🟢 we win · 🟡 parity / close · 🔴 behind · ⚪ neither has it (open flank).

| Aspect | terminal-browser | Terminal-Fenster today | Score | Where the win/gap is |
|---|---|---|---|---|
| **Perf: bytes/frame** | unstated | sparse damage tiles; dense local frames use runtime-probed `t=s` (<160 PTY bytes for the measured 2K geometry in OS tests) | 🟢 **win — if proven on a terminal** | needs Ghostty displayed-FPS measurement to claim it |
| **Perf: CPU/frame** | unstated | sparse tiles avoid full conversion/deflate; dense `t=s` avoids zlib/base64 | 🟢 **win — pending proof** | same measurement |
| **Low-RAM footprint** | unstated (Electron-free; libghostty) | Electron ~309 MB baseline | 🔴 **behind** | our biggest real weakness — §4 P1 |
| **Rendering fidelity** | libghostty renderer | real Chromium 150 (WebGL/video verified in B02) | 🟡 arguably ahead on fidelity | keep; it offsets the RAM cost |
| **Terminals supported** | 50+ (libghostty) | Ghostty verified; kitty/WezTerm expected-untested; Unicode fallback | 🔴 behind on breadth | verify kitty+WezTerm; §4 P2 |
| **SSH remote** | yes, no port-forward | designed, **not implemented** | 🔴 behind | §4 P1 — and mosaic makes ours *faster* over SSH |
| **Split-pane** | `--split right` | none | 🔴 behind | §4 P3 |
| **Multi-session `ls`** | yes | single instance | 🔴 behind | §4 P3 |
| **Agent-native CLI** | `action` (agent-browser compat) | 16-tool MCP server; 28/28 live Chromium checks | 🟢 **ahead on semantics** | accessibility refs, screenshots, audit log; no TCP debug port |
| **Install one-liner** | curl \| bash | hardened source + verified prebuilt installers; no signed tag | 🟡 partly behind | archive path works; signing/tag remain |
| **Honest degradation** | unknown | `doctor` reports caps; Unicode fallback when no graphics | 🟢 win (quality signal) | keep |
| **Tabs/history/bookmarks** | none | none (`--profile` persistence exists) | ⚪ open flank | ship first → leapfrog |

Net: we **win on performance, fidelity, and semantic agent control**, are **behind on breadth,
remoting, multi-session, RAM, and install**, and there's an **open flank** (tabs/history)
neither has.

---

## 3. The winning wedge: publish the performance story

Their site has no numbers. Ours should lead with them. The moment C08's mosaic is confirmed on a
real terminal, Terminal-Fenster can state — truthfully and uniquely — a per-interaction bandwidth/CPU
advantage that a libghostty screen-scraper structurally cannot match, because it repaints the
character grid rather than transmitting a Chromium damage rect.

This is also the low-end-hardware story the brief asks for: fewer bytes and less encode CPU per
frame **is** "runs without lagging, good fps" on a weak machine and over SSH. The two asks are the
same lever.

**Blocking fact:** the numbers are modelled/measured off-repo but **not yet confirmed in the
shipping binary on a terminal** (C08 status: "needs interactive Ghostty confirmation"). Until that
lands we cannot put them on a landing page. That makes P0 a *verification* task, not a coding task.

---

## 4. Prioritized roadmap (each task notes collision-safety)

### P0 — Prove the performance win (unblocks the entire wedge)
- **P0.1** Run the local-damage, shared-memory dense, and direct-control commands in `RELEASE.md`
  from Ghostty; confirm displayed FPS, **no tearing / no stale overlays**, correct resize, and
  clean teardown while the hybrid switches modes. Record the JSON/log evidence. *Needs a graphics
  terminal — human or a terminal-capable agent; cannot run under the agent sandbox.*
- **P0.2 — ✅ DONE (commit `c092c83`).** `crates/tf-term/tests/encode_roundtrip.rs` decodes the
  encoder's APC output back to pixels (base64 + inflate) and asserts byte-for-byte equality with
  the input, including a chunked frame and a framebuffer-cropped tile. Proves *encoding*
  correctness in CI without a terminal. On-screen *placement* correctness still rides on P0.1.

### P1 — Close the two most-cited gaps
- **P1.1 SSH transport.** Implement the designed adaptive transport so a remote `terminal-fenster open`
  works with no port forwarding (match their headline remote feature) — and it will be *faster*
  than theirs because only damage tiles cross the link. *Touches engine/cli — coordinate; owner TBD.*
- **P1.2 Low-RAM mode.** Cap Chromium's footprint: `--js-flags=--max-old-space-size`, disable
  unused features, `--disable-features`, single-process where safe, and an idle-throttle that drops
  the frame rate (the `--fps` flag is a start) when the page is static. Target: usable on a 4 GB
  box. *Engine flags — coordinate with the frame-pipeline owner.*

### P2 — Match agent-native + breadth
- **P2.1 — ✅ DONE.** The 16-tool MCP server drives a live engine, composites damage-only
  frames for screenshots, and passes 14 compositor/discovery/privacy + 24 protocol + 28 live Chromium checks.
  CDP uses Electron's in-process debugger over the private 0600 socket, with no
  `--remote-debugging-port` or unauthenticated localhost endpoint.
- **P2.2 Verify kitty + WezTerm end-to-end** (README lists them "expected, untested"). Moves three
  terminals from expected → verified and narrows the breadth gap. *Needs those terminals.*

### P3 — Reach feature parity, then leapfrog
- **P3.1** Split-pane (`--split`) and **P3.2** multi-session `ls` — match their layout/session UX.
- **P3.3 — 🟡 PARTIAL.** Hardened source and prebuilt installers exist with atomic rollback,
  runtime/materialization validation, manifest-tamper rejection, engine-discovery smoke tests,
  and profile-safe removal. A real 120.7 MB macOS arm64 archive was checksum-verified before and
  after extraction and passed isolated install/discovery/uninstall. Still missing signed/tagged
  artifacts, the other native targets, and clean-machine verification.
- **P3.4 Open flank:** tab strip + omnibox + history/bookmarks. Neither product has these; shipping
  them first turns parity into a lead.

---

## 5. Suggested next actions for the swarm

1. Anyone on a graphics terminal: take **P0.1** — it gates the whole marketing wedge.
2. An engine-focused agent already in `apps/engine`: fold **P1.2** low-RAM flags into the
   `--fps`/profile work rather than as a separate edit.
3. Packaging work can take **P3.3** without touching the active renderer paths.

Keep this scorecard honest: only move a 🔴 to 🟡/🟢 when the change is **verified**, not merely
written. That is the same bar B02 and C08 held themselves to.

---

## 6. Progress log

**2026-08-01 (tick 2).** Observed while verifying the tree — the swarm is executing the roadmap:
- **P0.2 done** (round-trip encoder verification, `c092c83`).
- **CI landed** (`.github/workflows/ci.yml`): `fmt --check` + `clippy -D warnings` + tests +
  release on ubuntu & macOS, plus an engine job (npm test, e2e, fixture matrix, bench self-test,
  MCP handshake). **Verified all five jobs pass locally** — the clippy baseline was cleaned up too.
- **P1.2 low-RAM moving** — e2e now asserts *"terminal focus loss gates output and lowers paint
  rate"* (idle throttle). Score 🔴→🟡 once a real RAM number is measured.
- **Fixture matrix 14/14** — file-upload, popup, escape-injection now covered (closes the B02
  popup/`<select>` gap; hardens agent/fidelity).
- Test count: 161 Rust + engine 7 + MCP compositor/discovery/privacy 14 + e2e 16 + fixtures 14 + MCP protocol
  24 + MCP live 28 + headless engine RSS/process-cleanup smoke, all green.
- **P2.1 done:** live MCP actions and screenshots are wired; damage-frame compositing fixed a
  real screenshot regression, and CDP moved from an unauthenticated loopback listener to the
  engine's in-process debugger over the existing private socket.
- **Dense local transport implemented:** a real-object runtime probe gates raw Kitty `t=s` bases;
  sparse damage remains direct/zlib overlays, and four unconsumed objects trigger bounded fallback.
  OS tests prove an 8.9 MB reference frame becomes a sub-160-byte PTY command, but P0.1 still owns
  the real displayed-FPS and visual evidence.
Still the gating item for the marketing wedge: **P0.1**, an on-a-terminal Ghostty measurement.
