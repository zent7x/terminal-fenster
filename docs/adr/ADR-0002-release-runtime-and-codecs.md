# ADR-0002: Prebuilt runtime, integrity, and codec policy

- **Status:** Accepted
- **Date:** 2026-08-01
- **Decision owner:** Terminal-Fenster maintainers
- **Supersedes:** the unresolved distribution question in F03 §2.4

## Context

Terminal-Fenster needs Electron at runtime, but Electron's npm dependency tree exists only to download
and unpack the platform binary. Shipping that downloader tree would add packages that are not on
the browsing path, make the installed layout need Node, and still leave binary provenance implicit.

Electron's normal runtime also carries Chromium's stock FFmpeg. The supply-chain review identified
proprietary codec/patent exposure as an explicit release decision. Terminal-Fenster does not need H.264
to establish its terminal-browser release wedge, while Electron publishes a matching
free-codecs-only FFmpeg artifact for every supported target.

Any replacement of a library inside Electron's macOS app bundle invalidates the prior code
signature. Codec selection must therefore happen before Developer-ID signing and notarization,
never after them.

## Decision

Public prebuilt archives will:

1. Build the Rust binary with `--release --locked` for the native host.
2. Fetch Electron's official target archive and separate free-codecs FFmpeg archive.
3. Verify both SHA-256 values against the reviewed `packaging/engine-lock.json`, whose Electron
   version must also equal the exact `apps/engine/package.json` pin.
4. Replace stock FFmpeg in staging, then construct a minimal engine containing the Electron
   runtime, a POSIX launcher, and Terminal-Fenster's two engine source files. npm and its acquisition
   dependencies are not shipped and Node is not a runtime requirement.
5. Include the Terminal-Fenster license, exact license files for all seven linked Cargo dependencies,
   Electron's license, and Chromium's complete generated attribution document.
6. Emit an internal per-file SHA-256 manifest and an external archive SHA-256 sidecar. Both the
   staged tree and a freshly extracted archive must pass structural, version, codec-library, and
   runtime verification.
7. On macOS, apply an ad-hoc signature only for local structural testing. A public artifact remains
   **NO-GO** until it is Developer-ID signed after codec replacement, notarized, stapled, and passes
   Gatekeeper. The release marker records the signing state so an ad-hoc build cannot masquerade as
   a signed one.

Source installs may continue to use the exact npm pin because they are builds performed by the
user, not Terminal-Fenster binary redistributions. The stricter archive policy governs artifacts the
project publishes.

## Evidence

`tools/package-release.sh` produced `terminal-fenster-0.1.0-darwin-arm64.tar.gz` from the reviewed locks.
The 120,704,153-byte archive had SHA-256
`0ff1ba8e03b51fd3c34be7089c342db8810c2d62bd5ee36e84f77ce3225ccc06`. Its payload verified before
and after tar extraction, Electron reported 43.2.0 through the Node-free launcher, the free-codecs
library matched the staged hash, and an isolated install found the sibling engine through a PATH
symlink before uninstalling cleanly.

That hash is evidence for this local candidate, not a published release checksum. Rebuilding after
any source change is expected to produce a different artifact and sidecar.

`tools/package-layout-test.sh` provides the fast regression gate: it tests all four lock entries,
rejects a deliberately modified post-manifest engine file, installs a synthetic native archive,
proves executable-relative engine discovery, and uninstalls it.

## Consequences

- Public binary installs need no Rust, Cargo, Node, npm, source checkout, or network connection.
- H.264 and other proprietary codecs from stock FFmpeg are intentionally unavailable in prebuilt
  Terminal-Fenster archives. That is a product tradeoff, not an accidental build difference.
- The large Chromium runtime remains, but downloader-only npm packages do not.
- A version bump now requires updating one reviewed artifact lock and proving that it still agrees
  with the exact npm pin.
- macOS signing must be performed after packaging. Signing credentials and notarization are still
  external release blockers; this ADR does not pretend otherwise.

## Reversal cost

Low technically, high legally. A future decision to distribute proprietary codecs would replace one
artifact and require a new legal review, lock update, tests, and release note. It must never happen as
an undocumented packager flag.
