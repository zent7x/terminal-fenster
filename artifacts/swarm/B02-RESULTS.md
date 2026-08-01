# B02 — OSR capability probe: results

Run 2026-08-01. Electron 43.2.0 / Chromium 150.0.7871.129, Node 24.18.0, darwin/arm64. Two modes: `bitmap` (shipping path, full 21-stage run) and `shared` (texture-metadata cross-check). Raw JSON: `apps/engine/spike/out/b02-{bitmap,shared}.json` (gitignored — this file is the committed digest).

## Verdict

**Outcome A — PARTIAL DAMAGE CONFIRMED in the bitmap/shipping path.**

bitmap: _PARTIAL DAMAGE CONFIRMED (bitmap path): min ratio 0.00123, 299 rects on the stimulus box; full-canvas control p50 1_

shared: _bitmap dirtyRect reads full-frame, but shared-texture `captureUpdateRect` reports partial damage (299 partial frames) — the same information via a second path._

## GPU posture (bitmap run)

All software this session (locked/headless): `gpu_compositing: disabled_software`, `2d_canvas: disabled_software`, `webgl: disabled_off`. WebGL still runs via ANGLE/Metal (see webgl stage). This is the pure software bitmap path — i.e. the shipping path.

## Per-stage (bitmap)

| stage | paints | dmg min | dmg p50 | partial frac | rects on box |
|---|---|---|---|---|---|
| `damage-tiny-fps60` | 359 | 0.00123 | 0.00123 | 0.997 | 299 |
| `damage-full-fps60` | 359 | 1 | 1 | 0 | 0 |
| `damage-composited-control` | 359 | 0.0037 | 0.0037 | 0.997 | 299 |
| `damage-caret` | 1 | 1 | 1 | 0 | 0 |
| `damage-hover` | 27 | 0.0037 | 0.0037 | 0.963 | 26 |
| `damage-text-tick` | 80 | 0.00114 | 0.00114 | 0.975 | 78 |
| `damage-tiny-fps30` | 180 | 0.00123 | 0.00123 | 0.994 | 179 |
| `damage-tiny-fps10` | 60 | 0.00123 | 0.00123 | 0.983 | 59 |
| `damage-tiny-720x450` | 360 | 0.00494 | 0.00494 | 0.997 | 299 |
| `idle-static` | 1 | 1 | 1 | 0 | 0 |
| `invalidate-forces-full` | 6 | 1 | 1 | 0 | 0 |
| `paint-control-stop-start` | 240 | 0.00123 | 0.00123 | 0.996 | 239 |
| `dpr-1x` | 300 | 0.00494 | 0.00494 | 0.997 | 299 |
| `dpr-2x` | 300 | 0.00494 | 0.00494 | 0.997 | 299 |
| `zoom-2x` | 300 | 0.00494 | 0.00494 | 0.997 | 299 |
| `webgl` | 360 | 0.06944 | 0.06944 | 0.997 | 299 |
| `video` | 212 | 0.05926 | 0.05926 | 0.991 | 210 |
| `cursor-changed` | 3 | 0.00248 | 0.00248 | 0.667 | 2 |
| `popup-window-open` | 1 | 1 | 1 | 0 | 0 |
| `popup-select-dropdown` | 2 | 0.00355 | 1 | 0.5 | 1 |

## Key facts for the damage encoder (C08)

- **Dirty rect is device-px, same space as the bitmap.** dsf=1 -> box `(600,400,40,40)` on 720x450; dsf=2 -> `(1200,800,80,80)` on 1440x900. Crop the BGRA buffer with the rect as-is; no CSS->device scaling.
- **True 2D boxes, not row strips** (`fullWidthPartialHeight 0` on tiny-fps60).
- **fps-invariant:** ratio 0.00123 at 60/30/10 fps -> capture throttling keeps locality (H5 refuted).
- **idle stops painting** (1 paint) and **invalidate() costs exactly full-damage frames** (6 paints @ 1.0).
- **Caret blink unverified:** headless session gave no OS focus (`document.hasFocus()=false`), so blink timer never fired (1 paint). Text-tick (78 on-box partial paints) proves the typing damage path; reconfirm the 1x20px caret rect interactively in a focused Ghostty window.
- **Side finding — native `<select>` is NOT composited into the OSR frame** (`diffBelowSelect 0`); it is a separate Chromium widget and would be invisible. Needs a product decision (render our own menu). Out of scope for this spike.
- WebGL 2.0 via `ANGLE (Apple, ANGLE Metal Renderer: Apple M4, Unspecified Version)` renders as partial damage; canvas.captureStream->video plays (readyState 4). Popups create an offscreen child webContents but need explicit paint wiring (childPaints 0).
