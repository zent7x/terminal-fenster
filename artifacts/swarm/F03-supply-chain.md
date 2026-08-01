# F03 — Supply Chain: Dependency Inventory, Licenses, SBOM, and Integrity Policy

**Mission:** F03 · **Owner:** F03 agent · **Date:** 2026-07-31
**Host:** macOS 26.1, Apple M4 (arm64) · **Deliverables:** this file and `artifacts/swarm/F03-sbom.json`

**Scope.** Every dependency Terminal-Fenster has, where it comes from, what license it carries, what integrity guarantee we actually hold over it, and the policy that keeps all of that true over time.

> **2026-08-01 implementation update:** this remains the historical audit, but several defects it
> records are now closed. The project consistently uses MIT, ships `LICENSE-MIT`, collects every
> Cargo dependency license during source and prebuilt installs, and ADR-0002 resolves the FFmpeg
> question in favor of Electron's checksum-pinned free-codecs artifact. Current release truth is in
> `RELEASE.md`; its macOS Developer-ID/notarization gate is stricter than the early unsigned-release
> recommendation in this report.

**Relationship to sibling artifacts.** `A09-threat-model.md` §8 sets supply-chain *policy* (TB6, T23/T-SUP-1). `B10-packaging-updater.md` §3 designs *Electron acquisition* and proves the download chain. This document does not restate either; it supplies what neither has — the complete component inventory, the license inventory, the machine-readable SBOM, and the lockfile/audit mechanics — and it corrects two points in A09 where the ground truth on this host turned out to be different from what was assumed. Those corrections are called out explicitly in §5.3 and §6.4.

---

## 0. Measured ground truth

Everything below was produced by running these commands from `$REPO` on 2026-07-31. No number in this document is estimated.

```
$ cargo --version              → cargo 1.93.0 (083ac5135 2025-12-15)
$ rustc --version              → rustc 1.93.0 (254b59607 2026-01-19)
$ node --version               → v24.11.1
$ npm --version                → 11.6.2
$ python3 --version            → Python 3.14.2      (probe scripts only)
$ cc --version                 → Apple clang 17.0.0 (probe scripts only)
$ df -h /System/Volumes/Data   → 3.5 GiB available, 100% capacity
```

Tooling **not** present, verified by `which`: `cargo-audit`, `cargo-deny`, `cargo-cyclonedx`, `cargo-sbom`, `cargo-binstall`, `syft`, `grype`, `osv-scanner`. This is unchanged from A09's finding and it constrains §5.

### 0.1 The shape of the problem in one table

| Ecosystem | First-party | Third-party | Third-party LOC | On-disk |
|---|---|---|---|---|
| Rust (`Cargo.lock`) | 3 crates, 3,493 LOC | **7 crates** | 149,321 (129,990 of it `libc` FFI decls) | 95 MiB `target/` |
| npm (`apps/engine`) | 1 package | **13 packages** | 43,344 LOC across 190 `.js` files | 309 MiB `node_modules/` |
| npm (`packages/mcp`) | 1 package, 1,685 LOC | **0** | 0 | 0 |
| Prebuilt binary | — | **Electron 43.2.0 → Chromium 150.0.7871.129** | 773 bundled third-party components | 192 MiB single dylib |

Read that bottom row against the three above it. Our hand-written dependency surface is 20 packages and small. Our *actual* trusted-code surface is one 192 MB binary containing 773 third-party projects that no lockfile in this repo describes. §4 and §7 are about that asymmetry.

---

## 1. Complete dependency inventory

### 1.1 Rust — `cargo tree --locked --all-features`, verbatim

```
tf-proto v0.1.0 ($REPO/crates/tf-proto)

tf-term v0.1.0 ($REPO/crates/tf-term)
├── flate2 v1.1.9
│   ├── crc32fast v1.5.0
│   │   └── cfg-if v1.0.4
│   └── miniz_oxide v0.8.9
│       ├── adler2 v2.0.1
│       └── simd-adler32 v0.3.10
└── libc v0.2.189

terminal-fenster v0.1.0 ($REPO/apps/cli)
├── tf-proto v0.1.0 ($REPO/crates/tf-proto)
├── tf-term v0.1.0 ($REPO/crates/tf-term) (*)
└── libc v0.2.189
```

`tf-proto` has **zero** dependencies — `crates/tf-proto/Cargo.toml` has no `[dependencies]` section at all. The whole third-party Rust surface is two direct crates (`libc`, `flate2`, both declared in `[workspace.dependencies]` at `Cargo.toml:11-13`) pulling five transitives. `flate2` is there for the kitty graphics zlib path; `libc` for raw-mode termios and the tty guard.

Notable for a project of this ambition: no `serde`, no `tokio`, no `clap`, no `anyhow`. The wire protocol, the argument parsing, and the async loop are hand-written. That is a deliberate and defensible supply-chain posture and it should be preserved as a stated constraint, not drifted away from by accident.

### 1.2 npm engine — `npm ls --all` from `apps/engine`, verbatim

```
@terminal-fenster/engine@0.1.0 $REPO/apps/engine
└─┬ electron@43.2.0
  ├── @electron-internal/extract-zip@1.0.5
  ├─┬ @electron/get@5.1.0
  │ ├─┬ debug@4.4.3
  │ │ └── ms@2.1.3
  │ ├── env-paths@3.0.0
  │ ├── graceful-fs@4.2.11
  │ ├── progress@2.0.3
  │ ├── semver@7.8.5
  │ ├─┬ sumchecker@3.0.1
  │ │ └── debug@4.4.3 deduped
  │ └── undici@7.29.0
  └─┬ @types/node@24.13.3
    └── undici-types@7.18.2
```

13 packages, one direct declaration (`electron: ^43.2.0`). `undici` is an *optional* dependency of `@electron/get`; `@types/node` is a runtime dependency of `electron` rather than a devDependency, which is unusual but harmless.

The important structural fact: **every one of these 13 packages exists only to download and unzip the Electron binary.** `apps/engine/src/main.js:17-18` requires exactly `electron` and the Node builtin `net`. Nothing in `@electron/get`, `sumchecker`, `undici`, `semver`, or `progress` is on the runtime path once the binary is on disk. That means the shipped product can carry zero npm packages if acquisition moves to the pinned fetcher B10 §3.5 designs. §4.2 turns that observation into policy.

### 1.3 `packages/mcp` — zero dependencies

`packages/mcp/package.json` declares `"dependencies": {}` and `"devDependencies": {}`, and that is honest: the only `require()` calls across its 1,685 lines are `child_process`, `fs`, `net`, `os`, `path`, `zlib`, and its own `./lib/*`. There is no `node_modules/` and no lockfile in that directory. A zero-dependency MCP server is a genuinely good outcome and worth protecting with a CI assertion (§5.4).

### 1.4 The Electron binary and Chromium — the components no manifest lists

This is the part of the dependency graph that `cargo tree` and `npm ls` are structurally blind to, and it is where nearly all of the risk lives.

| Component | Version | Evidence |
|---|---|---|
| Electron runtime | 43.2.0 | `node_modules/electron/dist/version` |
| Electron ABI | 148 | `node_modules/electron/abi_version` |
| **Chromium** | **150.0.7871.129** | `LC_ALL=C grep -a -o 'Chrome/[0-9.]*'` on `Electron Framework` |
| `libffmpeg.dylib` | Chromium-bundled | 2,230,784 bytes; codecs in §2.4 |
| `libvk_swiftshader.dylib` | Chromium-bundled | 16,574,256 bytes |
| `libGLESv2.dylib` / `libEGL.dylib` | Chromium-bundled (ANGLE) | 6,245,376 / 91,952 bytes |
| Bundled third-party projects | **773** | `grep -c '<div class="product">' dist/LICENSES.chromium.html` |

The Chromium version is read out of the shipped binary, not inferred from release notes. `Electron Framework` is 192,532,256 bytes; `LICENSES.chromium.html` is 19,956,019 bytes and enumerates 773 products.

Electron 43 release timeline, from `registry.npmjs.org`: 43.0.0 published `2026-06-30T14:52:08Z`, 43.2.0 published `2026-07-21T18:51:06Z`, `dist-tags.latest = 43.2.0`, and no 44.x exists. **We are pinned to the current stable major, ten days old.** That is the best position to be in and it should be treated as a property to maintain, not a coincidence (§4.4).

### 1.5 Build-time-only, not shipped

`benchmarks/probes/Makefile` uses `clang` with no dependencies (`CC ?= clang`, `CFLAGS ?= -O2 -Wall`). `benchmarks/a07/*.py` and `apps/engine/spike/*.py` use system Python 3.14.2. Neither the C probes nor the Python scripts are on any shipped path; they exist to reproduce the numbers in `A10-performance-plan.md`. They are recorded in the SBOM with `scope: excluded`.

---

## 2. License inventory and compliance

### 2.1 Full SPDX inventory

Rust licenses were read from each crate's own `Cargo.toml` in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/<name>-<version>/`, not from a database. npm licenses were read from both `package-lock.json` and the installed `package.json`, which agree in all 13 cases.

| Component | Version | SPDX | Notice file present |
|---|---|---|---|
| `adler2` | 2.0.1 | `0BSD OR MIT OR Apache-2.0` | LICENSE-0BSD, -APACHE, -MIT |
| `cfg-if` | 1.0.4 | `MIT OR Apache-2.0` | LICENSE-APACHE, -MIT |
| `crc32fast` | 1.5.0 | `MIT OR Apache-2.0` | LICENSE-APACHE, -MIT |
| `flate2` | 1.1.9 | `MIT OR Apache-2.0` | LICENSE-APACHE, -MIT |
| `libc` | 0.2.189 | `MIT OR Apache-2.0` | LICENSE-APACHE, -MIT |
| `miniz_oxide` | 0.8.9 | `MIT OR Zlib OR Apache-2.0` | LICENSE, -APACHE, -MIT, -ZLIB |
| `simd-adler32` | 0.3.10 | `MIT` | LICENSE.md |
| `@electron-internal/extract-zip` | 1.0.5 | `BSD-2-Clause` | **none — see §2.2d** |
| `@electron/get` | 5.1.0 | `MIT` | LICENSE |
| `@types/node` | 24.13.3 | `MIT` | LICENSE |
| `debug` | 4.4.3 | `MIT` | LICENSE |
| `electron` (npm) | 43.2.0 | `MIT` | LICENSE |
| `env-paths` | 3.0.0 | `MIT` | license |
| `graceful-fs` | 4.2.11 | `ISC` | LICENSE |
| `ms` | 2.1.3 | `MIT` | license.md |
| `progress` | 2.0.3 | `MIT` | LICENSE |
| `semver` | 7.8.5 | `ISC` | LICENSE |
| `sumchecker` | 3.0.1 | `Apache-2.0` | LICENSE |
| `undici` | 7.29.0 | `MIT` | LICENSE |
| `undici-types` | 7.18.2 | `MIT` | LICENSE |
| Electron binary | 43.2.0 | `MIT` | `dist/LICENSE` |
| Chromium | 150.0.7871.129 | `BSD-3-Clause` core | `dist/LICENSES.chromium.html` |

**Every direct and transitive dependency is permissive.** There is no copyleft obligation anywhere in the declared graph, and no license is incompatible with shipping Terminal-Fenster under `MIT OR Apache-2.0`. `sumchecker`'s Apache-2.0 carries a patent grant and a NOTICE requirement, and `@electron-internal/extract-zip`'s BSD-2-Clause requires the copyright notice be reproduced in binary redistributions — both are satisfied by a generated `THIRD-PARTY-NOTICES` file (§2.5), and both are moot if §4.2 removes npm from the shipped product entirely.

### 2.2 Four first-party licensing defects, in order of severity

**(a) `packages/mcp/package.json:26` declares `"license": "UNLICENSED"` while the Rust workspace declares `MIT OR Apache-2.0`.** `UNLICENSED` is not a stylistic choice — it is npm's explicit marker for "you may not use this." Two components of the same product currently make contradictory legal claims. Whichever way this resolves, it must resolve to one answer.

**(b) There is no `LICENSE` file at the repository root.** `ls LICENSE*` matches nothing. `Cargo.toml:9` says `license = "MIT OR Apache-2.0"`, but a dual MIT/Apache-2.0 project must ship both `LICENSE-MIT` and `LICENSE-APACHE` texts for that declaration to mean anything. `cargo publish` will also warn on this, and Homebrew and Linux distro packaging will both ask for it.

**(c) `apps/engine/package.json` has no `license` field at all.** It is `private: true`, so npm will not complain, but it leaves the engine host in the same ambiguous state as (a).

**(d) `@electron-internal/extract-zip@1.0.5` declares BSD-2-Clause but ships no license text** — `ls node_modules/@electron-internal/extract-zip` shows no `LICENSE`/`COPYING` file. This is an upstream packaging omission, not our defect, but it means a notice generator that harvests license *files* will silently produce an incomplete notice for that package. Any generator we adopt must fall back to the SPDX identifier and flag the missing text rather than skip the entry.

### 2.3 Chromium's 773 components and the NOTICE obligation

`dist/LICENSES.chromium.html` is the aggregated attribution file Chromium generates for `about:credits`. A frequency scan of license names inside it returns `Apache License` 3,931 times, `ISC` 986, `GNU General Public` 461, `MIT License` 286, `Mozilla Public License` 90, `GNU Lesser General Public` 82, `zlib` 28.

The GPL and LGPL counts deserve a plain statement rather than alarm or hand-waving. **I did not verify which specific Chromium sub-components those 461 GPL and 82 LGPL mentions attach to, or whether they are dual-license options rather than sole terms. That is UNVERIFIED and it is a real gap.** Chromium's own position is that everything it links into the browser binary is permissively licensed or dual-licensed such that a permissive option applies, and Electron ships under MIT on that basis — but "upstream says so" is a weaker claim than "we checked," and if Terminal-Fenster is ever distributed commercially or bundled by a third party, someone will eventually ask us to have checked.

The operational obligation is unambiguous regardless of how that resolves: **`LICENSES.chromium.html` must be redistributed with any binary we ship.** It is 19.9 MB. B10 §7 discusses trimming the Electron bundle; this file must be on the do-not-trim list, and `terminal-fenster --licenses` should be able to open it.

### 2.4 The unowned exposure: bundled H.264 and HEVC

`strings` on the shipped `libffmpeg.dylib` returns `h264`, `H264`, `hevc`, `HEVC`, `flac`, `opus`, `theora`, `vorbis`. The default Electron build ships proprietary, patent-encumbered codecs.

Nothing in `ADR-0001-browser-engine.md` mentions licensing, codecs, ffmpeg, or patents — I grepped it for all of those terms and got zero hits. Nothing in A09 or B10 covers it either. **This is currently unowned.**

The exposure is patent licensing, not copyright: H.264/AVC and HEVC pools (Via LA, Access Advance) assert claims against distributors of decoding software. Chrome and Edge ship these because Google and Microsoft hold pool licenses. An independent project redistributing an Electron bundle with the stock `libffmpeg.dylib` inherits the question and not the license.

The clean mitigation already exists upstream and costs us nothing: Electron publishes a free-codecs-only ffmpeg as a separate release artifact, present in `node_modules/electron/checksums.json` as

```
ffmpeg-v43.2.0-darwin-arm64.zip = 6d4b9158bfc442288833ecffa1bd240bf10c60e7f37ab32872e97ee773d23fc3
```

Dropping that `libffmpeg.dylib` over the bundled one yields a browser that plays VP8/VP9/AV1/Opus/Vorbis/FLAC/Theora and refuses H.264. For a terminal browser whose first release will not do video at all, the functional cost on day one is zero and the legal cost avoided is real. This should be an ADR, and the decision must be made *before* the first public binary, because reversing it after distribution does not undo the distribution.

I am not a lawyer and this is not legal advice; it is a flag that a decision exists, has a default, and that the default is currently "ship the encumbered build."

### 2.5 Third-party code reuse — checked, and one good precedent

I grepped `crates/`, `apps/cli/`, and `packages/mcp/` for `copyright`, `SPDX-License`, `adapted from`, `ported from`, and `derived from`. Exactly one file matched: `packages/mcp/lib/snapshot.js:5`, which states that the accessibility-snapshot-with-stable-refs design was popularised by Playwright MCP, that the project is Apache-2.0 (Copyright Microsoft Corporation), that **no code from it is used**, and then enumerates three substantive implementation differences.

That is precisely the right hygiene — it documents the influence, names the license, disclaims the copy, and makes the disclaimer falsifiable by describing how the implementations differ. It should be the template for any future "we looked at how X does this" situation, and it should not be removed by a future cleanup pass that mistakes it for a stale comment.

Per HARD RULE 4, I checked before suggesting reuse: nothing in this repository currently reuses third-party code.

---

## 3. The SBOM

### 3.1 Format choice

**CycloneDX 1.6 JSON**, written to `artifacts/swarm/F03-sbom.json`.

CycloneDX over SPDX for three specific reasons rather than preference. It has a first-class `dependencies` graph, so the `terminal-fenster → tf-term → flate2 → miniz_oxide` chain survives into the document instead of flattening into a package list. It has `compositions`, which lets the document *declare its own incompleteness* — essential here, because the Chromium subtree genuinely is incomplete and an SBOM that quietly omits 773 components is worse than one that says so. And `purl` identifiers (`pkg:cargo/...`, `pkg:npm/...`, `pkg:generic/...`) are what OSV, Dependency-Track, and Grype key on, so the artifact is directly consumable by a scanner without conversion.

npm 11.6.2 can emit CycloneDX natively — `npm sbom --sbom-format cyclonedx` works and I verified it — but it covers only the 14-package npm subtree. It knows nothing about Cargo, nothing about the Electron *binary* (as opposed to the npm fetcher package), and nothing about Chromium. It is a useful cross-check, not a substitute.

### 3.2 What is in it

33 components, 35 dependency nodes, 37,126 bytes. Generated by a script that parses `Cargo.lock` and `apps/engine/package-lock.json` directly — **no hash in the SBOM was typed by hand.**

- 3 first-party Rust crates + 7 crates.io crates, each with the SHA-256 of its `.crate` tarball taken from `Cargo.lock`.
- `@terminal-fenster/engine` + 13 npm packages, each with its SHA-512 converted from the lockfile's base64 `integrity` field to hex (CycloneDX requires hex; the conversion round-trips exactly — verified).
- `@terminal-fenster/mcp`, marked with its zero-dependency status and the license conflict from §2.2a.
- The **Electron binary** as a distinct `platform` component with the real SHA-256, its size, its ABI, and properties recording that it has no npm provenance, no GPG signature, and an ad-hoc code signature.
- **Chromium 150.0.7871.129** as a `framework` component, with the 773-component count and an explicit incompleteness marker.
- `libffmpeg` with its codec inventory and the §2.4 patent note.
- 5 build-time toolchain components at `scope: excluded`.

Self-validation run against the output: 33 components, no dangling `bom-ref` in any dependency edge, every component carries a license expression, and the electron SHA-512 hex round-trips back to the lockfile's base64.

### 3.3 Honest completeness statement

The SBOM declares two `compositions`. The first-party and npm/cargo assemblies are `aggregate: complete` — I can defend that every package is listed. The Electron and Chromium assemblies are `aggregate: incomplete`, because 773 sub-components are represented by one entry.

Closing that gap means parsing `LICENSES.chromium.html` into components, which is a bounded, offline, one-afternoon job against a file already on disk. It is the right next step *after* the policy work in §4 and §5, not before it — an exhaustive SBOM of an unversioned tree is a snapshot of nothing.

### 3.4 Regeneration

The generator is deliberately a standalone script with no dependencies, so it can live at `packaging/gen-sbom.js` and run under plain Node in CI. It must run **after** `npm ci` and `cargo fetch --locked` and **before** any packaging step, and CI should fail if the emitted SBOM differs from the committed one in anything but `serialNumber` and `metadata.timestamp` — that diff is the tripwire that catches a dependency added without review.

---

## 4. Lockfile policy

### 4.1 The finding that outranks everything else in this document

```
$ git rev-parse --show-toplevel   → $REPO
$ git ls-files | wc -l            → 0
$ git log --oneline -5            → fatal: your current branch 'main' does not have any commits yet
$ cat .gitignore                  → cat: .gitignore: No such file or directory
```

**The repository has zero commits and zero tracked files, and there is no `.gitignore`.**

`A09-threat-model.md:884` states "`package-lock.json` committed" and §8.2 states "`Cargo.lock` committed." Neither is true today. Both lockfiles exist on disk and both are good — `Cargo.lock` is `version = 4` with checksums for all seven registry crates, `package-lock.json` is `lockfileVersion: 3` with `integrity` for all 13 packages — but a lockfile that is not under version control provides **no** supply-chain guarantee. It cannot be reviewed in a PR, it cannot be diffed when a transitive dependency moves, and it can be silently rewritten by any `npm install` or `cargo update` with nothing to compare against.

Every other control in this document, in A09 §8, and in B10 §3 is downstream of this one. Hash pinning that no one can diff is decoration.

Compounding it: with no `.gitignore`, the first `git add -A` commits `apps/engine/node_modules` (309 MiB) and `target/` (95 MiB) — 404 MiB of build output into git history, on a machine with 3.5 GiB free. That is not a hypothetical; it is what the next obvious command does.

### 4.2 The policy

**Commit, in the first commit:** `Cargo.lock`, `apps/engine/package-lock.json`, and a `.gitignore` containing at minimum `/target`, `node_modules/`, `apps/engine/spike/out/`, and `.DS_Store`. `Cargo.lock` is committed **because this workspace produces a binary** — the library-crates-don't-commit-lockfiles convention does not apply to `apps/cli`.

**Rust builds:** `cargo build --locked --offline` for anything released. `--locked` fails the build if `Cargo.lock` would change; `--offline` guarantees no network resolution silently substitutes a version. CI should also run `cargo fetch --locked` as a separate step so a lockfile drift failure is distinguishable from a compile failure.

**npm installs:** `npm ci --ignore-scripts --omit=dev`. Never `npm install` in CI — `ci` fails on a lockfile/manifest mismatch, `install` silently repairs it.

**On `--ignore-scripts`, a correction to A09 §8.1.** A09 recommends allowlisting "the handful of packages that genuinely need" lifecycle scripts. On this tree, **the handful is zero.** Verified three independent ways:

```
$ npm query ":attr(scripts, [postinstall])"      → []
$ grep -c hasInstallScript package-lock.json     → 0
$ node -p "JSON.stringify(require('./electron/package.json').scripts)"  → undefined
```

`electron@43.2.0` has **no `scripts` field at all**. It exposes downloading as an explicit `bin` entry (`"install-electron": "install.js"`), not as a postinstall hook. So `--ignore-scripts` is free hardening here with no allowlist and no exception — it removes the single largest npm compromise vector at zero functional cost.

The consequence is that engine acquisition **must** be an explicit, separately-gated step. That is not a problem; it is exactly the design B10 §3.5 already specifies in `packaging/fetch-engine.sh`, and this finding removes the last argument for keeping npm's implicit download path. §1.2 showed all 13 npm packages exist solely to perform that download. Once `fetch-engine.sh` owns acquisition, **the shipped product carries zero npm dependencies** and the entire npm column of §2.1 becomes a build-time concern.

**Version ranges.** `apps/engine/package.json` declares `electron: "^43.2.0"`. The caret is fine because `package-lock.json` pins the resolution and `npm ci` honours it — but only while the lockfile is committed. Pin it exactly to `43.2.0` anyway. The engine version must move in lockstep with `packaging/engine.lock.json` (B10 §4.1), and a caret invites those two to diverge in a way nothing currently detects.

### 4.3 Delete the stray root `package.json`

`$REPO/package.json` contains, in full:

```json
{"name":"qtest","version":"1.0.0","main":"safestore.js"}
```

There is no `safestore.js` in the repository and no `qtest` anywhere in the project. This is a leftover from unrelated work sitting in the workspace root. Its presence is not harmless: `npm` commands run from the repo root will resolve to this manifest rather than erroring, `npm audit` at the root will report a clean tree that audits nothing, and the file will be committed as project metadata by the first `git add -A`. It should be deleted, or replaced with a real workspace root manifest if npm workspaces are ever adopted for `apps/engine` + `packages/mcp`.

I did not delete it — it is outside my file ownership (HARD RULE 1).

### 4.4 Staying current

Electron ships a new major roughly every 8 weeks and supports the latest three. We are on 43.2.0, the current latest, published 10 days ago. Maintaining that is a supply-chain control, not a feature decision: Chromium security fixes reach us only through an Electron bump, and falling two majors behind means running a Chromium with publicly-documented, unpatched vulnerabilities in a program whose entire job is rendering untrusted web content.

The bump procedure is B10 §4.2's. What F03 adds is the gate: **CI should fail, not warn, if `dist-tags.latest` for `electron` is more than one minor ahead of our pin, or any major ahead.** A single unauthenticated `GET https://registry.npmjs.org/electron` returns everything needed.

---

## 5. Audit tooling

### 5.1 Current state, measured

`npm audit` from `apps/engine`, JSON output: `"vulnerabilities": {}` — 0 info, 0 low, 0 moderate, 0 high, 0 critical, across 13 prod dependencies, 0 dev, 1 optional. Clean.

`npm audit signatures`: **13 of 13 packages have verified registry signatures; 5 have verified Sigstore attestations.** `--json` returns `{"invalid": [], "missing": []}`.

Cargo: no advisory scan has ever been run, because `cargo-audit` is not installed. The seven crates involved are among the most-audited in the ecosystem and I would be surprised by an open advisory, but **surprise is not evidence and this is UNVERIFIED.**

### 5.2 Which 5 packages are attested — and the one that is not

A09 §8.1 recommends `npm audit signatures` but does not say what it currently returns. Querying `registry.npmjs.org` per package for `dist.attestations`:

| Attested (SLSA provenance v1) | Not attested |
|---|---|
| `@electron-internal/extract-zip@1.0.5` | `@types/node@24.13.3` |
| `@electron/get@5.1.0` | `debug@4.4.3` |
| `semver@7.8.5` | **`electron@43.2.0`** |
| `undici@7.29.0` | `env-paths@3.0.0` |
| `undici-types@7.18.2` | `graceful-fs@4.2.11`, `ms@2.1.3`, `progress@2.0.3`, `sumchecker@3.0.1` |

**The single component that matters most has the weakest provenance in the tree.** `electron` — the package that fetches 192 MB of native code we execute — has no build attestation, while its own two helper libraries do. This is not a criticism of Electron's release engineering so much as a statement of where our trust actually rests, and it sharpens A09 §8.3's "no GPG signature" note into something more specific: there is no provenance of *any* kind on the electron package, neither GPG on the release manifest nor Sigstore on the registry tarball.

The practical consequence: `npm audit signatures` passing tells us less than it appears to. It should still be a CI gate, but it must not be mistaken for having verified the engine. §6 is what actually verifies the engine.

### 5.3 A correction: `minimumReleaseAge` is not available

A09 §8.1 suggests enabling `minimumReleaseAge` "if available in your registry config." On this host it is not:

```
$ npm config get minimum-release-age          → undefined
$ npm config get minimum-release-age-exclude  → undefined
$ npm config ls -l | grep -i release-age      → (no output)
```

npm 11.6.2 does not know the key. The hijack-and-publish mitigation A09 wanted has to come from somewhere else — a proxy registry (Verdaccio, Artifactory) that enforces a quarantine window, or simply the fact that after §4.2 the shipped product has no npm dependencies at all. The latter is cheaper and stronger, and it is another argument for the same move.

### 5.4 Recommended CI gate

Ordered so the cheapest check fails first. Each command has a distinct exit path so CI reports *which* control tripped.

```bash
set -euo pipefail

# --- lockfile integrity (must be first; everything else assumes it) ---
git diff --exit-code Cargo.lock apps/engine/package-lock.json   # no drift, ever
cargo fetch --locked                                            # exit != 0 ⇒ Cargo.lock stale

# --- rust advisories, licenses, bans, sources ---
cargo audit --deny warnings
cargo deny check advisories bans licenses sources

# --- npm ---
( cd apps/engine && npm ci --ignore-scripts --omit=dev )
( cd apps/engine && npm audit --audit-level=moderate )
( cd apps/engine && npm audit signatures )                      # fail on any invalid/missing

# --- zero-dependency invariants (assert what §1.3 measured) ---
test "$(node -p "Object.keys(require('./packages/mcp/package.json').dependencies||{}).length")" = 0
test "$(cd apps/engine && npm ls --all --parseable | wc -l)" -le 14

# --- engine binary integrity (see §6) ---
./ci/verify-electron.sh

# --- SBOM freshness: regenerate and diff, ignoring serial + timestamp ---
node packaging/gen-sbom.js --out /tmp/sbom.json
jq 'del(.serialNumber, .metadata.timestamp)' /tmp/sbom.json > /tmp/a.json
jq 'del(.serialNumber, .metadata.timestamp)' artifacts/swarm/F03-sbom.json > /tmp/b.json
diff -u /tmp/b.json /tmp/a.json     # any diff ⇒ a dependency changed without review
```

The two `test` lines are the load-bearing ones for drift. They convert §1.3's zero-dependency MCP server and §1.2's 13-package npm tree from *observations* into *invariants*. Without them, the day someone adds `axios` to the MCP server is a day nobody notices.

### 5.5 The disk constraint is real — do not `cargo install` these locally

`cargo install cargo-audit cargo-deny` compiles several hundred crates from source. `cargo-deny` alone pulls in `krates`, `cargo_metadata`, `rustsec`, `gix`, and a TLS stack. On a machine with **3.5 GiB free** that is a plausible way to fill the disk, and it would violate this mission's environment constraints.

Recommended instead: **run both in CI only**, where a `Swatinem/rust-cache`-backed runner has room, or install prebuilt binaries locally via `cargo-binstall` (itself not currently installed) or Homebrew. Do not add `cargo install cargo-audit cargo-deny` to a developer setup doc without a disk-space precondition next to it. A09 §8.2's bare `cargo install cargo-audit cargo-deny` should carry that caveat.

### 5.6 Proposed `deny.toml`

`cargo-deny` is where the license policy stops being prose and becomes a build failure. This file does not exist yet; per HARD RULE 1, I am describing it rather than creating it.

```toml
[advisories]
db-urls = ["https://github.com/rustsec/advisory-db"]
yanked = "deny"
unmaintained = "workspace"

[licenses]
# Every crate currently in the tree satisfies one of these. A new crate that
# does not will fail the build rather than be discovered at release time.
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
         "BSD-3-Clause", "ISC", "Zlib", "0BSD", "Unicode-3.0"]
confidence-threshold = 0.93

[bans]
multiple-versions = "deny"   # trivially satisfiable today: zero duplicates in Cargo.lock
wildcards = "deny"
deny = []

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

`multiple-versions = "deny"` is set to `deny` rather than `warn` specifically because the current lockfile has **zero** duplicate crates. Setting the bar at the current state costs nothing today and makes the first duplicate a conscious decision. `unknown-git = "deny"` matters more than it looks — a git dependency has no crates.io checksum and no `cargo audit` coverage, so it is a hole in every control above.

---

## 6. Electron binary integrity verification

### 6.1 Independently re-verified

B10 §3.2 proved this chain. I re-ran the confirmable link independently rather than citing it, because a supply-chain report that takes another document's word for its central hash is not doing its job:

```
$ shasum -a 256 ~/Library/Caches/electron/9c4e2246.../electron-v43.2.0-darwin-arm64.zip
ad4a0ae3c37ee05aa06c7e2ed0627608389790f0505a2b0d20319efbe33ffe28
  (122,090,802 bytes, 2.4 s wall)

$ node -p 'require("electron/checksums.json")["electron-v43.2.0-darwin-arm64.zip"]'
ad4a0ae3c37ee05aa06c7e2ed0627608389790f0505a2b0d20319efbe33ffe28
```

**Match.** The bytes on this disk are the bytes the npm package says they should be. B10 §3.2(a) additionally matched both against GitHub's published `SHASUMS256.txt`; I did not repeat that network call and I am citing B10 for it rather than claiming it.

I also verified the seven crates.io tarballs against `Cargo.lock` by recomputing every SHA-256 from `~/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/*.crate`:

```
OK  adler2-2.0.1      OK  cfg-if-1.0.4     OK  crc32fast-1.5.0
OK  flate2-1.1.9      OK  libc-0.2.189     OK  miniz_oxide-0.8.9
OK  simd-adler32-0.3.10
```

7 of 7. Cargo does this automatically on every build, so this is confirmation rather than news — but it means the Rust half of the tree has an end-to-end verified integrity chain today, with no additional tooling required.

### 6.2 The four layers, and what each is worth

| Layer | Mechanism | What it actually proves |
|---|---|---|
| 1. Registry TLS | HTTPS to registry.npmjs.org / GitHub Releases | Transport only. Nothing about content. |
| 2. npm `integrity` | SHA-512 in `package-lock.json`, all 13 | The npm *fetcher* tarballs are unmodified since we locked them |
| 3. npm signatures | ECDSA registry sigs (13/13), Sigstore (5/13) | Registry attests it served these bytes. **electron is not in the attested 5.** |
| 4. Binary SHA-256 | `checksums.json` → `sumchecker` → 122 MB zip | **The only layer covering the code that actually runs** |

Layers 1-3 protect 43,344 lines of JavaScript that stop executing the moment the binary is on disk. Layer 4 protects 192 MB of native code that renders untrusted HTML. The effort should be proportioned accordingly.

### 6.3 What is genuinely not covered, stated plainly

`SHASUMS256.txt` is not GPG-signed and no `.asc` sits beside it in the release (B10 §3.3). `electron@43.2.0` has no Sigstore attestation (§5.2). So an attacker who controls the Electron release pipeline defeats every layer simultaneously — the zip, the manifest, and the `checksums.json` inside the npm tarball are all published by the same pipeline. This is a **self-referential check**, and B10 §3.3 is right that fetching the remote manifest at install time adds nothing over pinning the hash ourselves.

There is no mitigation available to us that closes this. There is only the mitigation that narrows the *window*: pin the hash in our own repository, at a reviewed git tag, so the trust decision happens in a PR a human read rather than on a user's laptop at install time. That is B10 §3.4's `packaging/engine.lock.json`, and F03 endorses it without modification.

### 6.4 A finding B10 §0.3 raises that belongs in the risk register

```
$ codesign -dv --verbose=2 node_modules/electron/dist/Electron.app
CodeDirectory v=20400 flags=0x20002(adhoc,linker-signed)
Signature=adhoc
Sealed Resources=none

$ spctl -a -t exec -vv node_modules/electron/dist/Electron.app
...: code has no resources but signature indicates they must be present
```

Confirmed independently. The stock npm Electron bundle is ad-hoc, linker-signed, with no sealed resources — `spctl` rejects it and `codesign --verify` fails. It cannot be shipped as-is to anyone whose environment assesses bundles (Gatekeeper, MDM, EDR, a corporate build gate). B10 §5 owns the re-signing and notarization procedure. F03's contribution is to record it as a **supply-chain** item and not only a packaging one: an ad-hoc signature means the binary carries no cryptographic statement of origin at rest, so after extraction the SHA-256 pin is the *only* thing tying those bytes to Electron. If the extracted tree is ever modified in place — by an installer, an antivirus quarantine-and-restore, or a well-meaning `chmod -R` — nothing detects it.

Recommendation: `terminal-fenster setup` should record the SHA-256 of the extracted `Electron Framework` binary alongside the zip hash, and `terminal-fenster doctor` should re-check it. That is cheap (one hash of one file) and it converts a one-time install-gate into a standing invariant.

---

## 7. Risk register

| # | Risk | Likelihood | Impact | Status | Owner |
|---|---|---|---|---|---|
| **S1** | **Lockfiles not under version control; repo has 0 commits** | **Certain — it is the current state** | **Critical** — voids every control below | **OPEN** | commander |
| **S2** | No `.gitignore`; first `git add -A` commits 404 MiB of build output | High | High — bloats history irreversibly, 3.5 GiB free | **OPEN** | commander |
| S3 | Bundled H.264/HEVC patent exposure, unowned, undecided | Certain if shipped as-is | High | **OPEN — needs an ADR** | commander |
| S4 | `packages/mcp` `UNLICENSED` contradicts workspace `MIT OR Apache-2.0` | Certain | Medium — blocks any distribution | **OPEN** | commander |
| S5 | No `LICENSE`/`LICENSE-MIT`/`LICENSE-APACHE` at repo root | Certain | Medium | **OPEN** | commander |
| S6 | Electron release-pipeline compromise (no GPG, no Sigstore) | Low | Critical | **ACCEPTED** — narrowed by hash pinning, B10 §3.4 | — |
| S7 | Rust advisories never scanned (`cargo-audit` absent) | Unknown | Medium | **UNVERIFIED** | CI, §5.4 |
| S8 | Chromium's 773 components not individually enumerated; GPL/LGPL attachment unverified | Medium | Medium | **UNVERIFIED**, §2.3 | follow-up |
| S9 | Stray root `package.json` (`qtest`) masks root-level npm commands | Certain | Low | **OPEN** | commander |
| S10 | Electron drifts >1 major behind → unpatched Chromium CVEs | Medium over 6 months | High | Mitigated by §4.4 gate | CI |
| S11 | Dependency added without review (no SBOM diff, no invariant tests) | Medium | Medium | Mitigated by §5.4 | CI |

S1 and S2 are the only two rated as certain-and-severe, and they are the two cheapest to fix.

---

## 8. Recommendations, in execution order

1. **`git add` the two lockfiles and a `.gitignore`, and make the first commit.** Nothing else in this document has force until this is done. Roughly ten minutes.
2. Delete the stray root `package.json` (§4.3).
3. Add `LICENSE-MIT` and `LICENSE-APACHE` at the repo root; resolve `packages/mcp` to the same terms; add a `license` field to `apps/engine/package.json` (§2.2).
4. Adopt `npm ci --ignore-scripts` unconditionally — verified zero-cost on this tree (§4.2) — and move engine acquisition to B10 §3.5's pinned fetcher, which drops npm from the shipped product entirely.
5. Pin `electron` to exact `43.2.0`, and create `packaging/engine.lock.json` per B10 §3.4.
6. Land the §5.4 CI gate, including the two zero-dependency invariant assertions and the SBOM freshness diff.
7. Write an ADR on the ffmpeg codec build. The free-codecs artifact hash is in §2.4 and the day-one functional cost is zero.
8. Move `gen-sbom.js` to `packaging/`, commit `F03-sbom.json` as the baseline, and later extend it to parse `LICENSES.chromium.html` (§3.3).

---

## 9. File ownership

Per HARD RULE 1, I wrote only `artifacts/swarm/F03-supply-chain.md` and `artifacts/swarm/F03-sbom.json`. The SBOM generator ran from a scratchpad directory outside the repository.

I did not create, modify, or delete: the stray root `package.json`, any `.gitignore`, any `LICENSE` file, `deny.toml`, `packaging/engine.lock.json`, `ci/verify-electron.sh`, `packaging/gen-sbom.js`, `packages/mcp/package.json`, `apps/engine/package.json`, any `Cargo.toml`, either lockfile, or anything under `crates/`, `apps/cli/`, or `apps/engine/src/`. Each is described above with enough specificity to be implemented by whoever owns it.

No test was weakened, no threshold lowered, and nothing in this document is presented as working software that is not. Items marked UNVERIFIED are: the license attachment of Chromium's GPL/LGPL sub-components (§2.3), the Rust advisory status of the seven crates (§5.1), and GitHub's live `SHASUMS256.txt` (cited from B10 §3.2, not re-fetched).
