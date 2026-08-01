# Terminal-Fenster release gate

Terminal-Fenster is a release candidate only when every automated check below passes and every
manual box has fresh evidence from the build being tagged. A green unit suite cannot verify
terminal placement, terminal decode throughput, signing, or a clean install.

## Automated gate

With Electron already installed (`cd apps/engine && npm ci`):

```sh
tools/release-check.sh
```

This runs formatting, 202 Rust checks, Clippy with warnings denied, the optimized build, source and
prebuilt install-layout checks (including manifest-tamper rejection), installer/packager syntax,
14 engine frame/tab/security tests, 20 real-pixel engine checks, 14 browser fixtures, the benchmark
parser self-test, 14 MCP compositor/discovery/privacy tests, 26 MCP protocol checks, 28 live Chromium
tool checks, a headless engine RSS/process-cleanup smoke test, the production website build, and
`git diff --check`.

## Real-terminal gate (mandatory)

Run from a normal Ghostty shell, not an agent-launched window. First confirm the exact runtime
path that will be exercised:

```sh
./target/release/terminal-fenster doctor
```

The reference local configuration must report `kitty graphics yes`, `kitty shared memory yes`,
pixel mouse, and synchronized output. Then run:

```sh
node benchmarks/bench.mjs --page local-damage --runs 5 --label rc-local
node benchmarks/bench.mjs --page repaint --runs 5 --label rc-shm-dense
TERMINAL_FENSTER_SHM=0 TERMINAL_FENSTER_TILE_CELLS=1x1 \
  node benchmarks/bench.mjs --page repaint --runs 5 --label rc-direct-control
```

Release acceptance on the reference 2K-class Ghostty viewport:

- [ ] Local-damage displayed steady state is at least 45 fps with no stale or torn tiles.
- [ ] Shared-memory dense repaint is at least 20 displayed fps and at least 2× its direct
      fallback control. If it is not, disable the path rather than shipping an unmeasured default.
- [ ] Resize repeatedly in both directions; no black canvas, stale edge, seam, or status-row bleed.
- [ ] Click, hover, drag, wheel, ordered typing, omnibox, back/forward, reload, focus loss/wake,
      and crash recovery all behave correctly by eye.
- [ ] Quit normally and with SIGINT; the cursor, keyboard, mouse, alternate screen, and Kitty
      images are fully restored/removed.
- [ ] The three benchmark JSON files and raw logs are retained with the release artifacts.

## Distribution gate

- [ ] A clean machine can install and run `terminal-fenster doctor` without the source tree.
- [x] Source install upgrades are staged, runtime-validated, atomically swapped with rollback;
      uninstall preserves profiles by default and refuses unrelated prefixes (automated layout
      and scoped-deletion smoke tests pass).
- [x] A real macOS arm64 archive contains `bin/terminal-fenster` plus a minimal pinned Electron engine;
      it verifies before/after extraction and passes isolated install, discovery, and uninstall.
- [ ] The `Unsigned release candidates` workflow completes on its four native hosted runners:
      macOS arm64/x64 and Linux arm64/x64. Each candidate must pass verification plus an isolated
      install/runtime/uninstall smoke test before its archive and checksum are retained.
- [x] Archives have an external SHA-256 sidecar and an internal per-file SHA-256 manifest whose
      corruption rejection is part of the automated gate.
- [ ] macOS artifacts are Developer-ID signed, notarized, stapled, and pass Gatekeeper. The local
      packager deliberately labels its structural ad-hoc signature rather than calling it signed.
- [x] `LICENSE-MIT`, every Cargo dependency license, Electron's license, and Chromium's complete
      generated third-party notices ship in and are verified by the archive.
- [x] Public archives use Electron's separately published, checksum-pinned free-codecs FFmpeg;
      stock proprietary-codec FFmpeg is never copied into a release artifact (ADR-0002).
- [x] Install, upgrade/rollback mechanics, default profile preservation, explicit scoped profile
      purge, unrelated-prefix refusal, and uninstall are exercised in isolated automated tests.

## Current verdict

**NO-GO** until the real-terminal and remaining distribution boxes above are closed. The code-level
gates and macOS arm64 archive mechanics are strong and repeatable; the remaining blockers are
empirical terminal evidence, a successful four-runner artifact workflow, a clean-machine run, and
macOS Developer-ID signing/notarization—not hidden unit-test failures.
