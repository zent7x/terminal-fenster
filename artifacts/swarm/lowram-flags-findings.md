# Low-RAM flag spike — which Chromium switches actually cut the engine's RSS

**Date:** 2026-08-01 · standalone, additive · spike: `apps/engine/spike/lowram-probe.js` ·
measurement tool: `benchmarks/engine-rss.js`. Serves roadmap **P1.2** (low-RAM mode) and the
brief's "runs on low-RAM PCs without lagging."

## Method

Command-line switches are process-global and must be set before app-ready, so each config is a
separate Electron process (same pattern as the B02 spike). Each process opens an offscreen
`BrowserWindow`, loads a page, **proves it still paints**, then samples the whole Chromium
process tree via `/bin/ps` and prints its RSS. A config that renders nothing is disqualified no
matter how low its RSS.

## Result (about:blank, 1280×800, Electron 43.2.0 / Chromium 150, Apple M4)

| config | switches | RSS | Δ vs baseline | renders |
|---|---|---|---|---|
| baseline | none | 353.9 MB | — | yes |
| v8cap | `--js-flags=--max-old-space-size=128` | 354.0 MB | **0** | yes |
| fewproc | `--renderer-process-limit=1 --disable-site-isolation-trials` | 354.0 MB | **0** | yes |
| **nogpu** | `app.disableHardwareAcceleration()` | **330.0 MB** | **−24 MB (−6.8%)** | yes |
| combined | all of the above | 330.1 MB | −24 MB | yes |

## What to do with it

1. **Disabling hardware acceleration is the only lever that moved RSS here — ~7%, and OSR still
   paints.** B02 already showed this machine composites OSR in software, so the GPU process is
   largely dead weight; dropping it is nearly free *here*. Caveat: on a box where hardware OSR
   readback would be faster, `disableHardwareAcceleration()` shifts that work to the CPU. So the
   right shape is an **opt-in low-RAM mode** (wire it to the existing `--profile`/`--fps` surface,
   e.g. a `--low-ram` flag), not an unconditional default — measure the fps cost on a GPU machine
   before making it the default there.
2. **V8 heap cap and process-limit switches do nothing on this config — skip them.** The RSS is
   Chromium's baseline process overhead, not V8 old-space, and a single-tab browser never spawns
   the extra site-isolated renderers `--renderer-process-limit` would cap. Useful negative result:
   don't spend effort wiring these.
3. The absolute baseline here (353.9 MB) is higher than `engine-rss.js`'s ~281 MB because this
   spike uses a bare `BrowserWindow` + a single post-settle sample while `engine-rss.js` drives the
   real `main.js` engine and takes a steady-state median. **Only the intra-spike deltas are
   comparable**; use `engine-rss.js` for the shipping-engine number and to confirm any flag's
   effect on the real engine before landing it.

## Reproduce

```sh
cd apps/engine   # agent sandbox disabled (Chromium children need Mach rendezvous)
for c in baseline v8cap fewproc nogpu combined; do
  ./node_modules/.bin/electron spike/lowram-probe.js --config=$c 2>/dev/null | grep __RESULT__
done
```
