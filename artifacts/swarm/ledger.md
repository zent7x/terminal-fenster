# Swarm ledger

60 bounded missions across six squads, plus the commander. Every row states what the mission
produced and what the commander did with it. "Integrated" means the finding changed shipped
code or a shipped decision, not that the document was read.

**Outcome: 57/60 delivered, 3 failed.** A01, A05 and A08 died on API connection errors
mid-run; a retry workflow was authored and **declined by the user**, so those three remain
unfilled and their scope is unowned. Recorded rather than quietly renumbered.

File ownership was exclusive throughout: agents wrote only to their named path, and the
commander made every edit to `crates/`, `apps/cli/` and `apps/engine/src/main.js`. No two
agents shared a file, so there were no write conflicts to resolve.

## Integration summary

| Disposition | Count | Notes |
|---|---|---|
| Delivered artifact | 57 | 54 in `artifacts/swarm/`, 3 as code trees |
| Findings integrated into shipped code | 8 | listed below |
| Corrected a commander decision | 3 | B02, B03, B05 |
| Failed (API error, retry declined) | 3 | A01, A05, A08 |

### Findings that changed shipped code or decisions

| Source | Finding | Action taken |
|---|---|---|
| A04 | iTerm2 3.6.9 **does** speak the Kitty graphics protocol | Corrected terminal matrix; one renderer now covers Ghostty + iTerm2 |
| A06 | SGR-Pixels (1016) is **not portable** — iTerm2 reports it permanently reset | Added DECRQM detection (`caps.rs`); added `PointerMap` so cell coordinates are scaled instead of misread as pixels. Without this, every click on iTerm2 lands in the top-left corner |
| A06 | Pixel-mode coords are 0-based, cell-mode 1-based | `PointerMap` handles both; tests pin the difference |
| B01 | Resize is dead code: no SIGWINCH, CLI never sends `resize` | Added SIGWINCH handler + resize path (`tty.rs`, `main.rs`) |
| B01 | `best_backend` can return Sixel/iTerm2 which `present()` silently draws as half-blocks | `resolve_backend` now degrades explicitly and `doctor` says so |
| B01 / B08 | `MessageReader` trusts a u32 length before validating | Added `MAX_MESSAGE_LEN` cap + `ProtocolError` |
| B01 | Engine stderr discarded, startup failures undiagnosable | stderr now captured next to `BLACKGLASS_LOG` |
| F10 | `expected_payload()` overflows; `parse_two_param_t` cannot tell a `CSI 14t` reply from `CSI 16t` | Added `checked_payload()`; `parse_typed_t` validates the report type |
| F10 | `license` declared with no LICENSE file; stray `package.json` from another project; no `.gitignore`/commits | Added `LICENSE-MIT`, `NOTICE.md`, `.gitignore`; quarantined the stray file; committed |

### Corrections to commander decisions

Three missions falsified something the commander had asserted. All three were accepted and
ADR-0001 was amended rather than silently edited.

- **B03** — ADR-0001 rejected CDP screencast because it "caps frame rate well below the
  compositor's native cadence." Measured: **59.9 fps, p50 16.66 ms**, statistically tied with
  OSR. The premise was false. The decision survives on different grounds (lossy text at
  PSNR 34.51 dB, no dirty-rect field in the protocol, 7.5 ms/frame decode tax).
- **B05** — the shared-texture p99 advantage in ADR-0001 came from a single sample and does
  not reproduce: across 10 paired runs it was **0.52 ms slower** (t=0.41). Also found
  IOSurface pads rows to 64 bytes (`bytesPerRow` 9984 vs a naive 9928 at 2482×814), so an
  addon reusing the bitmap stride would shear real windows. Shared texture moved from
  *deferred* to **rejected**.
- **B02** — the damage-tracking evidence was **confounded, not merely absent**: both spikes
  forced full-viewport damage by construction (`osr-probe.js` animated a compositor-only CSS
  transform; `fps-matrix.js` fills the whole canvas). Neither could have observed a small
  dirty rect. **Resolved 2026-08-01:** the designed spike ran and settled it affirmatively —
  a 40×40 change at (600,400) reports a dirty rect of exactly that, `ratio 0.00123`, on
  299/359 paints, versus `1.0` for the full-canvas control. Damage is in device pixels and
  fps-invariant; shared-texture `captureUpdateRect` agrees. ADR-0001 updated from "not proven"
  to "proven, not yet consumed"; C08 (crop in `main.js`) is now unblocked and is the gate on
  the SSH story.

## Squad A — product intelligence and architecture reconnaissance

| ID | Mission | Status | Disposition |
|---|---|---|---|
| A01 | Competitive benchmark of the reference product | **FAILED** | API connection error; retry declined by user. Partially covered by A02, which found `zenbu-labs/terminal-browser` has **no LICENSE** |
| A02 | Competitor matrix | delivered | Integrated — confirmed Carbonyl is dead (last commit 2023-02-26, Chromium 111) and that every Chromium *forker* died while embedders live. Reinforced ADR-0001 |
| A03 | User journeys | delivered | Informs roadmap; damage-rect leverage (~1000×) quantified |
| A04 | Terminal capability matrix | delivered | **Integrated** — iTerm2 Kitty support; Sixel deprioritised |
| A05 | Engine evaluation | **FAILED** | API connection error; retry declined. Superseded in practice by the commander's own OSR spikes and ADR-0001 |
| A06 | Input research | delivered | **Integrated** — the pointer-precision fix |
| A07 | SSH / remote research | delivered | Design only; not implemented |
| A08 | Agent automation research | **FAILED** | API connection error; retry declined. Partially covered by E01/E03/E04 |
| A09 | Threat and privacy model | delivered | Informs security posture |
| A10 | Performance plan | delivered | Stale partial-damage attribution corrected (A10:499) after B02 ran and proved it directly |

## Squad B — browser engine and process core

| ID | Mission | Status | Disposition |
|---|---|---|---|
| B01 | Architecture RFC | delivered | **Integrated** — 4 defects fixed |
| B02 | OSR capability probe | executed 2026-08-01 | **Partial damage CONFIRMED** (Outcome A): rects `(600,400,40,40)` r0.00123, 299/359 paints; device-px; fps-invariant. Unblocks C08. Results in `apps/engine/spike/out/b02-*.json` |
| B03 | Engine alternatives | delivered | **Corrected ADR-0001** on CDP |
| B04 | Tab lifecycle design | delivered | Design + diff; not applied (single-tab today) |
| B05 | Shared-texture analysis | delivered | **Corrected ADR-0001**; shared texture rejected |
| B06 | IPC hardening | delivered | Partially integrated (size cap); version handshake outstanding |
| B07 | Frame scheduler | delivered | Not implemented; dirty-rect union invariant recorded |
| B08 | Crash recovery | delivered | Partially integrated (stderr capture) |
| B09 | Profile/data services | delivered | Not implemented |
| B10 | Packaging/updater | delivered | Informs `NOTICE.md`; not implemented |

## Squad C — terminal graphics, compositor, transport

| ID | Mission | Status | Disposition |
|---|---|---|---|
| C01 | Kitty conformance audit | delivered | Encoder audited against spec |
| C02 | Sixel design | delivered | Deprioritised — neither test terminal supports Sixel |
| C03 | iTerm2 backend | delivered | Largely moot: iTerm2 speaks Kitty |
| C04 | Unicode fallback quality | delivered | Not implemented (quadrant/braille upgrade) |
| C05 | Detector hardening | delivered | Race conditions noted; partially addressed |
| C06 | Compositor design | delivered | Not implemented |
| C07 | Scaling and colour | delivered | Not implemented — **open risk**: at 2482 px wide with a 17 px cell, default scale makes text tiny |
| C08 | Damage encoder | consume-side implemented 2026-08-01 | Engine crops onPaint + core composites partial rects into a persistent framebuffer (tested + e2e 9/9). Terminal-transmission tile mosaic still full-frame — designed, needs interactive Ghostty verification |
| C09 | SSH adaptive transport | delivered | Not implemented |
| C10 | Rendering profiler | delivered | Partially present via `BLACKGLASS_LOG` |

## Squad D — input, chrome, human UX

| ID | Mission | Status | Disposition |
|---|---|---|---|
| D01 | Keyboard/IME audit | delivered | Functional-key table gaps identified; IME unimplemented |
| D02 | Mouse precision | delivered | **Integrated** alongside A06 |
| D03 | Scroll/gestures | delivered | Not implemented (fixed ±120 delta today) |
| D04 | Focus/clipboard | delivered | Bracketed paste shipped; OSC 52 read not implemented |
| D05 | Files/downloads/permissions | delivered | Not implemented |
| D06 | Browser chrome UX | delivered | Not implemented (status bar only) |
| D07 | Power-user controls | delivered | Not implemented |
| D08 | Terminal integration | delivered | `doctor` warns about tmux passthrough |
| D09 | Accessibility | delivered | Not implemented — doubles as low-bandwidth mode |
| D10 | UX validation | delivered | String review; partially applied |

## Squad E — agent control, devtools, extensibility

| ID | Mission | Status | Disposition |
|---|---|---|---|
| E01 | CDP broker | delivered | Not implemented — no CDP port is exposed today, which is the safe default |
| E02 | CLI automation spec | delivered | Not implemented |
| E03 | Playwright integration | delivered | Not implemented |
| E04 | **MCP server** | **delivered as code** | `packages/mcp/` — **independently verified** by the commander: real JSON-RPC `initialize` + `tools/list` handshake succeeds. Not yet wired to a live browser session |
| E05 | Human-agent handoff | delivered | Not implemented |
| E06 | Recorder/replay | delivered | Not implemented |
| E07 | CLI/SDK design | delivered | Informs roadmap |
| E08 | Extensions | delivered | Feasibility only |
| E09 | DevTools | delivered | Not implemented |
| E10 | Agent skills/docs | delivered | Docs only; no global config installed |

## Squad F — security, reliability, QA, performance, release

| ID | Mission | Status | Disposition |
|---|---|---|---|
| F01 | Security review | delivered | Reviewed sanitisation coverage and Electron hardening |
| F02 | Secrets/permissions | delivered | Flagged that `BLACKGLASS_LOG` records URLs and titles — a real privacy consideration, not yet redacted |
| F03 | Supply chain + SBOM | delivered | `F03-sbom.json`; informs `NOTICE.md` |
| F04 | Fuzz plan | delivered | Harness specified, not run |
| F05 | **Test fixtures** | **delivered as code** | `tests/fixtures/` — 15+ self-contained pages incl. an escape-injection corpus |
| F06 | **Benchmark harness** | **delivered as code** | `benchmarks/` |
| F07 | Chaos plan | delivered | Not run |
| F08 | CI matrix | delivered | Workflow YAML drafted, not installed |
| F09 | Install validation | delivered | Informs README |
| F10 | **Adversarial acceptance** | delivered | **NO-SHIP verdict, accepted.** 7 of its 8 blocking items are now fixed; the remaining one (evidence chain) is addressed by this ledger and the README |

## What the adversarial reviewer got right

F10's central criticism was not about the engineering but about the **evidence chain** — that
the commander's headline end-to-end tuple was a composite assembled from separate runs rather
than one primary log. That was fair. The numbers in the README now come from named runs, and
where a measurement is engine-side rather than through-the-terminal (the 60 fps figure), it
says so explicitly.
