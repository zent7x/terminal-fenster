# F09 — Install / Upgrade / Uninstall: validation plan and user documentation

**Agent:** F09  **Date:** 2026-07-31  **Host:** macOS 26.1, Apple M4
**Owns:** `artifacts/swarm/F09-install-validation.md` (this file only)
**Deliverable:** §4 is the user-facing install document, written to be lifted verbatim into `docs/INSTALL.md` by whoever owns `docs/`. §1–§3 are the evidence and defect list behind it. §5 is the validation plan. §6 is what I need from the commander.

---

## 0. What I ran, and one disclosure

Every claim below is a measurement taken on this machine today, or a citation to `file:line`. Nothing is inferred from how Electron or Cargo "usually" behave, because two of the most important findings are cases where they no longer do.

I wrote no file other than this one. **Disclosure:** I ran `touch crates/bg-term/src/tty.rs` to force a rebuild so I could capture the exact compiler warning text. That changed the file's mtime and nothing else; the bytes are untouched. It was still a core file and I should have found another way.

Constraints that shaped the method: the machine is at a lock screen (no GUI observation), Chromium child processes are blocked under the agent sandbox, and `/System/Volumes/Data` is at **100% capacity with 1.4–2.4 GiB free**, which put a hard ceiling on what I could install to test. Every experiment below was built to run inside those limits and was cleaned up afterwards.

---

## 1. Verified repo layout

This is the ground truth the install document is written against. Everything here was read or measured, not assumed.

| Path | What it is | Evidence |
|---|---|---|
| `Cargo.toml` | Workspace root. Members: `crates/bg-term`, `crates/bg-proto`, `apps/cli`. `rust-version = "1.80"`, `license = "MIT OR Apache-2.0"`, `repository = ".../zent7x/blackglass"` | `Cargo.toml:3,8-10` |
| `apps/cli/` | The `blackglass` binary. 1039 lines, `src/main.rs` | `apps/cli/Cargo.toml`, `wc -l` |
| `crates/bg-term/` | tty guard, capability detection, kitty encoder, input decode, unicode fallback | 5 modules, 1897 lines |
| `crates/bg-proto/` | wire protocol | 284 lines |
| `apps/engine/` | Electron host. `package.json` name `@blackglass/engine`, `private: true`, one dependency: `electron ^43.2.0` | `apps/engine/package.json` |
| `apps/engine/src/main.js` | Engine entrypoint, 309 lines | read in full |
| `apps/engine/spike/` | Probe scripts (fps-matrix, osr-probe, term-probe, kitty-proof). Not shipped | `ls` |
| `packages/mcp/` | MCP server, zero runtime deps, `bin: blackglass-mcp`, `engines.node >= 22`, `"license": "UNLICENSED"` | `packages/mcp/package.json` |
| `benchmarks/`, `tests/`, `docs/adr/` | Development-only. `tests/fixtures/assets/` holds two video files | `ls -R` |
| `target/` | 105 MB build output (debug 93 MB, release 12 MB) | `du -sh` |

Things that do **not** exist and that the install document must not pretend do: no `README`, no `LICENSE`, no `.gitignore`, no `rust-toolchain.toml`, no `Makefile`, no install script, no CI config. `git log` reports *"your current branch 'main' does not have any commits yet"*, `git remote -v` is empty, and `git branch -a` is empty. **There is no publishable clone URL yet**, so every "install from git" instruction in §4 is written against a repo state that does not exist on any server. Marked accordingly.

The root `package.json` is `{"name":"qtest","version":"1.0.0","main":"safestore.js"}`. It belongs to some other project, names an entrypoint that does not exist, and makes `npm install` at the repo root a silent no-op that some users will run and believe worked.

Toolchain measured on this host: `rustc 1.93.0`, `cargo 1.93.0`, `node v24.11.1`, `npm 11.6.2`.

---

## 2. The finding that rewrites the install document

### F09-1 (Critical, install/first-run) — `npm install` does not install a browser, and the first `blackglass open` pays a hidden 14-second-to-forever bill with the output discarded

Electron 43 no longer has a postinstall script. Verified from the registry, not from memory:

```
$ npm view electron@43.2.0 --json | python3 -c "...print(d.get('scripts'))"
scripts: None
```

I then ran the real thing in an isolated directory:

```
$ printf '{"name":"t","version":"1.0.0","dependencies":{"electron":"43.2.0"}}' > package.json
$ npm install --no-audit --no-fund
added 13 packages in 29s
$ du -sh node_modules/electron
1.1M    node_modules/electron
$ cat node_modules/electron/path.txt
NO path.txt -> binary NOT installed
$ ls node_modules/electron/     # no dist/
abi_version checksums.json cli.js electron.d.ts index.js install.js LICENSE package.json README.md
```

**A completed `npm install` yields 1.1 MB and no Chromium at all.** The binary is fetched lazily, on first invocation, by `node_modules/electron/index.js:7-17`:

```js
function downloadElectron () {
  console.log('Downloading Electron binary...');
  const result = spawnSync(process.execPath, [path.join(__dirname, 'install.js')], { stdio: 'inherit' });
```

which `cli.js:5` triggers via `require('./')` before it spawns anything. Measured, with the 116 MB zip **already in the local cache** (so this excludes all network time):

```
$ rm -rf node_modules/electron/dist node_modules/electron/path.txt
$ /usr/bin/time -p ./node_modules/.bin/electron --version
Downloading Electron binary...
v43.2.0
real 14.01
```

Now put that against the core. `apps/cli/src/main.rs:402-411` spawns the engine with `.stdout(Stdio::null()).stderr(Stdio::null())`, and `main.rs:415` gives it a hard budget:

```rust
let deadline = Instant::now() + Duration::from_secs(30);
...
"engine did not connect within 30s"
```

So on a first run against an un-materialized Electron, the user sees a frozen terminal, no progress output (it went to `/dev/null`), and after 30 seconds either a page or `blackglass: cannot start engine: engine did not connect within 30s`. **14.01 s of the 30 s budget is consumed before Chromium even starts, on a warm cache.** Cold, with a 116 MB download over anything slower than roughly 10 MB/s, first run cannot succeed.

This is the single highest-value thing the install document can prevent, and it costs one line: run `node_modules/.bin/electron --version` immediately after `npm install` and watch it print `v43.2.0`.

### F09-2 (High, first-run diagnostics) — `doctor` reports the engine as present when there is no browser on disk

`locate_engine()` (`main.rs:319-350`) tests `node_modules/.bin/electron` with `.exists()`. After a bare `npm install`, that path exists: it is a symlink to `cli.js`, created by npm's `bin` handling. Measured on the 1.1 MB install from F09-1:

```
$ ls -la node_modules/.bin/
electron -> ../electron/cli.js
$ BLACKGLASS_ENGINE=<that dir> blackglass doctor
  engine: .../node_modules/.bin/electron
```

Green light, no browser. `doctor` is the one tool whose entire job is to tell a user whether their install works, and on the most common broken install it says yes. The check should be `node_modules/electron/path.txt` plus the existence of the file it names.

### F09-3 (High, install/upgrade) — an installed binary silently prefers the developer's source tree

`locate_engine()` tries three strategies in this order: `$BLACKGLASS_ENGINE`, the **compile-time** `CARGO_MANIFEST_DIR` dev layout (`main.rs:327-334`), then an exe-relative walk (`main.rs:336-348`). The second outranks the third, and it is baked into the binary at build time.

I proved the consequence in the exact form a user will hit it:

```
$ cargo install --path apps/cli --root <scratch>
$ <scratch>/bin/blackglass doctor
  engine: /Users/adeebbashir/projects/blackglass/apps/engine/node_modules/.bin/electron
```

The installed binary reached back into the build tree. I confirmed it is precedence and not absence by planting a valid engine next to the installed binary; it was still ignored:

```
# <T>/bin/blackglass with a real <T>/engine/node_modules/.bin/electron present
$ <T>/bin/blackglass doctor
  engine: /Users/adeebbashir/projects/blackglass/apps/engine/node_modules/.bin/electron
```

Upgrade failure mode: install, then `git pull` and rebuild the engine. The installed binary now drives the *new* engine over the *old* protocol, or the reverse, with no version check anywhere in the handshake. Delete or move the source tree and the same binary abruptly starts working correctly, which is the kind of behaviour that burns an afternoon.

Side effect, verified with `strings`: the release binary embeds `/Users/adeebbashir/projects/blackglass/apps/cli`, so every distributed build leaks the builder's home directory path.

### F09-4 (High) — symlinking the binary onto `PATH` breaks engine discovery

Rust's `current_exe()` does not resolve symlinks on macOS 26.1, so the four-parent walk starts from the link, not the target:

```
# <T>/real/bin/blackglass with engine at <T>/real/engine, symlinked from <T>/linkdir/blackglass
$ <T>/linkdir/blackglass doctor
  engine: NOT FOUND (set BLACKGLASS_ENGINE)
```

This independently reproduces **B10 §6.5**, which found the same thing from the Homebrew direction and works around it with `write_env_script` setting `BLACKGLASS_ENGINE`. Two agents, two methods, same result. The install document must therefore tell users to **copy** the binary or use a wrapper, never `ln -s`.

Install layouts I verified as working (binary copied, not linked):

| Layout | Result |
|---|---|
| `<root>/bin/blackglass` + `<root>/engine/` | FOUND |
| `<root>/blackglass` + `<root>/engine/` (flat) | FOUND |
| `<root>/libexec/bg/bin/blackglass` + `<root>/libexec/bg/engine/` | FOUND |
| symlink from anywhere | NOT FOUND |
| no engine present | `engine: NOT FOUND (set BLACKGLASS_ENGINE)`, exit 1 |

### F09-5 (Medium) — a wrong `BLACKGLASS_ENGINE` is silently ignored

```
$ BLACKGLASS_ENGINE=/nope/nothing blackglass doctor
  engine: /Users/adeebbashir/projects/blackglass/apps/engine/node_modules/.bin/electron
```

The user set the variable, the variable was wrong, and BlackGlass quietly used something else. `main.rs:320-325` only takes the env path if it exists; otherwise it falls through with no diagnostic. If the variable is set at all, a miss should be a loud error naming the path that was tried.

### F09-6 (High, uninstall and privacy) — browsing data goes to a directory shared with every other Electron app on the machine

`apps/engine/src/main.js` never calls `app.setName()` or `app.setPath('userData', …)`, so Chromium uses Electron's default. Measured:

```
$ ls ~/Library/Application\ Support/Electron/
Cookies  Local Storage  Cache  Code Cache  DIPS  TransportSecurity  Trust Tokens
Network Persistent State  Preferences  Session Storage  Shared Dictionary  ...
$ du -sh ~/Library/Application\ Support/Electron/
6.4M
$ sqlite3 .../Electron/Cookies "select count(*), count(distinct host_key) from cookies;"
10|6
$ cat .../Electron/'Local State'
{"uninstall_metrics":{"installation_date2":"1785511280"}}
$ date -r 1785511280
Fri Jul 31 20:51:20 IST 2026
```

That timestamp is BlackGlass's first engine run today, which establishes ownership. Ten cookies across six hosts, a LevelDB `Local Storage` tree, and 4.3 MB of HTTP cache from ordinary browsing are now sitting in a directory named `Electron` that **any other unpackaged Electron app on this machine also uses**. The uninstall consequence is the hard part: we cannot `rm -rf` it, because it is not ours alone; and we cannot leave it, because it contains the user's browsing history.

This converges with **B10 §8.2** and **F01 §230-244**, which reached the same conclusion from packaging and security. Three independent findings on one two-line fix.

The MCP path already does the right thing (`packages/mcp/lib/engine.js:113-117` passes `--user-data-dir=<tmp>/profile`), but only when CDP is enabled. Set `BLACKGLASS_MCP_CDP=0` and the isolation silently disappears, because the `args.push` sits inside the `if (this.useCdp)` block.

### F09-7 (Medium, uninstall) — the residue nobody is tracking

A sweep of `~/Library` for anything created inside today's session window turned up more than the two directories everyone knows about:

| Path | Size | What |
|---|---|---|
| `~/Library/Application Support/Electron/` | 6.4 MB | shared profile, F09-6 |
| `~/Library/Application Support/{cdptest,e01,e09probe,c}/` | 1.1 MB each | four stray Chromium profiles from probe runs, created 20:58–23:16 today. The one named `c` looks like a truncated flag value |
| `~/Library/Caches/electron/` | 116 MB | Electron zip download cache, shared with other projects |
| `~/Library/Caches/chrome_crashpad_handler/` | 80 KB | Chromium crash handler cache |
| `~/Library/Logs/DiagnosticReports/Electron-*.ips` | 11 files, 1.0 MB | macOS crash reports from engine crashes today |
| `$TMPDIR/blackglass-mcp-*/` | 9 dirs, 356 KB | leaked MCP session dirs, each containing a Chromium profile |
| `$TMPDIR/blackglass-mcp-audit.jsonl` | 9,661 bytes | persistent audit log of every URL navigated and element clicked. Nothing ever deletes it |

Two clean results worth recording: `~/Library/LaunchAgents` contains nothing of ours, the keychain contains nothing under `blackglass`, and there were **zero** leftover `blackglass-<pid>-<nanos>` socket directories, which confirms the CLI's own cleanup at `main.rs:676-677` works on the normal shutdown path. The MCP server's `rmSync` (`lib/engine.js:321`) does not survive a hard kill; that is where the nine leftovers came from.

### F09-8 (Medium, macOS first-run) — ad-hoc signature plus an enabled firewall

```
$ codesign -dv --verbose=2 .../Electron.app
Identifier=Electron
Signature=adhoc
TeamIdentifier=not set
$ /usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate
Firewall is enabled. (State = 1)
$ xattr -l .../Electron.app
com.apple.provenance:          # no com.apple.quarantine
```

The npm-delivered Electron carries no quarantine flag, so Gatekeeper does not block it today. That changes the moment anyone ships a zip or tarball. Separately, the MCP path binds a CDP listener (`DevToolsActivePort` observed holding port 9418) from an ad-hoc-signed binary while the application firewall is on, which is the textbook trigger for the *"Do you want the application Electron to accept incoming network connections?"* dialog. **UNVERIFIED** whether that prompt actually fired here: the machine is at a lock screen and I cannot observe GUI dialogs. Every precondition is present, so the document warns about it.

### F09-9 (Medium, legal) — the license story does not close

`Cargo.toml` declares `MIT OR Apache-2.0`, but there is no `LICENSE`, `LICENSE-MIT`, or `LICENSE-APACHE` anywhere in the tree. `packages/mcp/package.json` declares `"license": "UNLICENSED"`, which contradicts it. Electron itself is MIT (`node_modules/electron/LICENSE`, and the registry agrees), but redistributing `Electron.app` also redistributes Chromium and its full third-party notice set, which a binary release must ship. Right now the terms under which anyone may use this are undefined, and `cargo publish` would reject the crate for the missing license file.

### F09-10 (Low, hygiene) — small things that will embarrass a first release

No `.gitignore`, so `git status` on a fresh clone offers to commit `target/` (105 MB) and `node_modules/` (309 MB). The stray root `package.json` from §1. And one build warning, which any `-D warnings` CI gate will turn into a red build on day one:

```
warning: direct cast of function item into an integer
   --> crates/bg-term/src/tty.rs:133:50
    |133|  libc::signal(sig, signal_handler as libc::sighandler_t);
    = note: `#[warn(function_casts_as_integer)]` on by default
```

---

## 3. Measured footprint

Everything a full install puts on disk, measured rather than estimated:

| Item | Size |
|---|---|
| `apps/engine/node_modules/` (295 MB of it `electron/dist/Electron.app`) | 309 MB |
| `~/Library/Caches/electron/` (zip cache) | 116 MB |
| `target/` (debug 93 MB + release 12 MB) | 105 MB |
| `~/Library/Application Support/Electron/` after light browsing | 6.4 MB |
| the `blackglass` binary itself (release) | 619,424 bytes |
| **Steady state** | **~530 MB** |

Peak during a first install is higher, because the 116 MB zip and the 295 MB extracted app exist at once. **Budget 1.2 GB free to install; 530 MB to keep it.** This host currently has 1.4–2.4 GiB free on a volume reporting 100% capacity, which is inside the requirement but not comfortably.

---

## 4. THE INSTALL DOCUMENT

> Ready to become `docs/INSTALL.md`. It is written against what the code does today, including the parts that are awkward; where behaviour is a known defect it says so rather than papering over it. Sections marked NOT YET TRUE must be corrected before publishing.

### Installing BlackGlass

BlackGlass is a real Chromium browser that renders as pixels inside your terminal. It is two pieces that ship together: a Rust binary that owns your terminal, and an Electron host that runs Chromium offscreen and streams frames back. You need both.

There are no prebuilt binaries yet. You build from source.

#### Prerequisites

| Requirement | Minimum | Verified on |
|---|---|---|
| macOS | 26.1 (Apple silicon) | 26.1, M4 |
| Rust | 1.80 per `Cargo.toml` (not tested at that floor) | 1.93.0 |
| Node.js | 22.12.0, required by Electron 43 | 24.11.1 |
| npm | any recent | 11.6.2 |
| Free disk | 1.2 GB to install, 530 MB steady state | measured |
| Network | ~116 MB Electron download on first use | measured |

Linux and Windows are untested. The code is Unix-only (raw-mode tty, Unix domain sockets), so Linux is plausible and Windows is not.

**Your terminal matters more than anything else here.** BlackGlass needs the kitty graphics protocol to draw real pixels. Without it you get a Unicode half-block fallback where layout and colour are visible but body text is not legible.

| Terminal | Result | Verified |
|---|---|---|
| Ghostty 1.3.1 | full pixel rendering, pixel-accurate mouse | yes, end to end |
| kitty, WezTerm | expected to work (kitty graphics) | UNVERIFIED |
| Apple Terminal 465 | Unicode fallback only, no graphics, cell-quantised mouse | yes |
| iTerm2 3.6.9 | UNVERIFIED (automation blocked by macOS TCC) | no |

Inside tmux you must also set `set -g allow-passthrough on`. GNU screen cannot pass graphics through at all.

#### Step 1: get the source

```sh
git clone https://github.com/zent7x/blackglass    # NOT YET TRUE: no commits pushed
cd blackglass
```

> **This repository has no commits and no remote yet.** Until it does, work from the directory you already have.

#### Step 2: build the terminal core

```sh
cargo build --release
```

Produces `target/release/blackglass` (about 620 KB). Expect one compiler warning from `crates/bg-term/src/tty.rs:133`; it is known and harmless.

Verify:

```sh
$ ./target/release/blackglass version
blackglass 0.1.0
```

#### Step 3: install the Electron engine, and then actually download it

This is the step that catches people, so do both halves.

```sh
cd apps/engine
npm install
```

`npm install` finishes in about 30 seconds and gives you 1.1 MB. **It does not download Chromium.** Electron 43 has no postinstall script; the 295 MB browser is fetched lazily the first time you run the `electron` command. If you stop here, BlackGlass will appear to be installed and will hang for 30 seconds on your first page load before failing with `engine did not connect within 30s`, showing you nothing in between, because the download's progress output is discarded by the engine launcher.

So trigger it now, deliberately, where you can watch it:

```sh
$ ./node_modules/.bin/electron --version
Downloading Electron binary...
v43.2.0
```

The first run downloads roughly 116 MB and takes 14 seconds of local work on top of however long your network needs. Confirm it landed:

```sh
$ cat node_modules/electron/path.txt
Electron.app/Contents/MacOS/Electron
$ du -sh node_modules/electron
296M
```

If `path.txt` is missing, the browser is not installed no matter what `npm install` said.

Behind a proxy or an air-gapped mirror, `@electron/get` reads `ELECTRON_MIRROR`, `ELECTRON_CUSTOM_DIR`, `ELECTRON_GET_USE_PROXY`, and the npm config `electron_config_cache` for the cache root.

#### Step 4: run the doctor

From a real terminal, not a pipe:

```sh
cd ../..
./target/release/blackglass doctor
```

You want to see `kitty graphics yes`, a resolved `--> backend kitty`, a `cell px` line with real numbers, and an `electron` path that is not `NOT FOUND`. On Ghostty the geometry block reports a 17x37 px cell.

Two caveats worth knowing before you trust it:

`doctor` exits **1** when stdin is not a TTY, by design, because capability detection works by asking the terminal questions. Do not use it as a health check in CI; use `blackglass version`, which exits 0.

`doctor` currently reports the engine as present whenever `node_modules/.bin/electron` exists, and that symlink exists after a bare `npm install` with no browser behind it. A green engine line is not yet proof that Chromium is on disk. Check `path.txt` as shown in step 3.

#### Step 5: browse

```sh
./target/release/blackglass open example.com
```

`ctrl+q` quits, `ctrl+r` reloads, `alt+left` / `alt+right` go back and forward, and the mouse works. On Ghostty, first frame arrives about 366 ms after launch.

#### Putting it on your PATH

The binary finds its engine by looking for `<dir>/engine/node_modules/.bin/electron` in each of the four directories above itself. So this layout works:

```
~/.local/blackglass/
  bin/blackglass          <- copy of target/release/blackglass
  engine/                 <- copy of apps/engine (package.json, src/, node_modules/)
```

```sh
mkdir -p ~/.local/blackglass/bin
cp target/release/blackglass ~/.local/blackglass/bin/
cp -R apps/engine ~/.local/blackglass/engine
```

**Do not symlink the binary onto your PATH.** `ln -s ~/.local/blackglass/bin/blackglass /usr/local/bin/blackglass` produces a `blackglass` that cannot find its own engine, because the discovery walk starts from the symlink's location rather than the real one. Use a wrapper instead:

```sh
printf '#!/bin/sh\nexec "$HOME/.local/blackglass/bin/blackglass" "$@"\n' > /usr/local/bin/blackglass
chmod +x /usr/local/bin/blackglass
```

Or set `BLACKGLASS_ENGINE` and skip the layout rules entirely:

```sh
export BLACKGLASS_ENGINE="$HOME/.local/blackglass/engine"
```

Note that if `BLACKGLASS_ENGINE` points somewhere wrong, BlackGlass currently ignores it silently and looks elsewhere rather than telling you. Confirm with `blackglass doctor` that the path it reports is the one you meant.

`cargo install --path apps/cli` also works, but be aware of a rough edge: the resulting binary has your source-tree path compiled into it and will prefer that engine over any engine you install alongside it. Rebuild both together, and if you move or delete the source tree, set `BLACKGLASS_ENGINE`.

#### Environment variables

| Variable | Effect |
|---|---|
| `BLACKGLASS_ENGINE` | directory containing `node_modules/.bin/electron` and `src/main.js` |
| `BLACKGLASS_BACKEND` | force `kitty`, `sixel`, or `unicode` |
| `BLACKGLASS_LOG` | append diagnostics to this file. Never logs to stdout, which is the graphics channel |
| `BLACKGLASS_MCP_*` | MCP server: `_LOG`, `_AUDIT`, `_PROFILE`, `_CDP=0`, `_WIDTH`, `_HEIGHT` |

#### What gets created, and where

Inside the repository:

```
target/                                        105 MB   build output
apps/engine/node_modules/                      309 MB   295 MB of it is Electron.app
apps/engine/node_modules/electron/dist/                 created on first electron run
apps/engine/node_modules/electron/path.txt              created on first electron run
```

Outside it, on macOS:

```
~/Library/Caches/electron/                     116 MB   Electron zip cache; SHARED with other
                                                        Electron projects on this machine
~/Library/Application Support/Electron/        6.4 MB   Chromium profile: cookies, localStorage,
                                                        cache, HSTS state. SEE THE WARNING BELOW
~/Library/Caches/chrome_crashpad_handler/       80 KB   Chromium crash handler
~/Library/Logs/DiagnosticReports/Electron-*.ips          one per engine crash
~/.cargo/registry, ~/.cargo/git                          shared Rust caches. Not ours to delete
$TMPDIR/blackglass-<pid>-<nanos>/                        control socket, 0700, deleted on clean exit
$TMPDIR/blackglass-mcp-XXXXXX/                           MCP session dir, includes a Chromium profile
$TMPDIR/blackglass-mcp-audit.jsonl                       MCP audit log of URLs and clicks. Never
                                                        deleted automatically
```

BlackGlass installs no launch agents, no login items, nothing under `/Library`, and no keychain entries. Verified by sweep.

> **Known issue — your browsing data is not in a BlackGlass-specific directory.** The engine does not set an application name, so Chromium writes cookies, localStorage, and cache to Electron's generic default at `~/Library/Application Support/Electron`. Every other unpackaged Electron app on your machine uses that same directory. Two consequences: your BlackGlass browsing state sits alongside other apps' data, and a clean uninstall cannot simply delete that folder. See the uninstall section for how to handle it. This is tracked as a pre-1.0 blocker.

#### Upgrading

```sh
git pull
cargo build --release
cd apps/engine && npm install && ./node_modules/.bin/electron --version
```

Always do all three. The Rust core and the Electron host speak a private binary protocol with no version negotiation in the handshake, so a mismatched pair fails in confusing ways rather than clean ones. If you copied an install to `~/.local/blackglass`, re-copy both the binary and the engine directory; updating one alone is the most likely way to get a mismatch.

If the Electron version in `apps/engine/package.json` changed, delete `apps/engine/node_modules` before reinstalling. `npm install` will replace the metadata but the stale `dist/` and `path.txt` can survive and leave you running the old browser.

#### Uninstalling completely

Everything BlackGlass creates, in order, with the shared-resource cases called out so you can decide.

**1. The project and its build output.**

```sh
rm -rf /path/to/blackglass
```

Removes the binary, `target/` (105 MB), and `apps/engine/node_modules/` (309 MB) together.

**2. The binary, if you copied or cargo-installed it.**

```sh
rm -rf ~/.local/blackglass
rm -f ~/.cargo/bin/blackglass
rm -f /usr/local/bin/blackglass          # the wrapper, if you made one
```

**3. Temporary files.** These usually clean themselves up, but not after a crash or a `kill -9`:

```sh
rm -rf "${TMPDIR:-/tmp}"/blackglass-*
```

That glob covers the CLI's socket directories, the MCP session directories, and `blackglass-mcp-audit.jsonl`. If you pointed `BLACKGLASS_MCP_AUDIT`, `BLACKGLASS_MCP_LOG`, or `BLACKGLASS_LOG` elsewhere, delete those files too. **The audit log records every URL you navigated to and every element you clicked**, so if privacy is the reason you are uninstalling, do not skip it.

**4. Your browsing data.** This is the shared directory from the warning above.

```sh
ls ~/Library/Application\ Support/Electron/
```

If you do not develop with Electron and have never run another unpackaged Electron app, this directory is BlackGlass's and you can remove it:

```sh
rm -rf ~/Library/Application\ Support/Electron
```

If you do, that directory is also some other app's profile. Delete only the browsing state:

```sh
cd ~/Library/Application\ Support/Electron
rm -rf Cookies Cookies-journal "Local Storage" "Session Storage" Cache "Code Cache" \
       "Shared Dictionary" SharedStorage* DIPS TransportSecurity "Network Persistent State"
```

Check `~/Library/Application Support/` for any other Chromium-shaped directories with a `DevToolsActivePort` file in them; development probes create these under arbitrary names.

**5. The Electron download cache.** 116 MB, shared with every other Electron project on the machine. Safe to delete, at the cost of a re-download for whatever else needs it:

```sh
rm -rf ~/Library/Caches/electron
```

**6. Crash artifacts.**

```sh
rm -rf ~/Library/Caches/chrome_crashpad_handler
rm -f ~/Library/Logs/DiagnosticReports/Electron-*.ips
```

**7. Leave these alone.** `~/.cargo/registry` and `~/.cargo/git` are your Rust toolchain's shared caches and are not BlackGlass's to remove. `~/Library/Preferences/com.github.Electron.plist` may predate your BlackGlass install; on the reference machine it did, by three months.

**Verify:**

```sh
ls -d ~/Library/Application\ Support/Electron 2>/dev/null
ls -d "${TMPDIR:-/tmp}"/blackglass-* 2>/dev/null
command -v blackglass
```

Three empty results means it is gone.

#### Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Hangs ~30 s, then `engine did not connect within 30s` | Electron binary was never downloaded; progress output goes to `/dev/null` | `cd apps/engine && ./node_modules/.bin/electron --version` |
| `cannot start engine: electron not found` | no engine in any search location | set `BLACKGLASS_ENGINE` |
| `doctor` says `engine: NOT FOUND` but you installed it | binary is a symlink, or engine is not in a sibling `engine/` dir | copy the binary instead of linking, or set `BLACKGLASS_ENGINE` |
| `doctor` shows an engine path you did not install | build-tree path compiled into the binary wins over your install | set `BLACKGLASS_ENGINE` explicitly |
| Page renders as coloured blocks, text illegible | terminal has no kitty graphics | use Ghostty, kitty, or WezTerm |
| Nothing renders inside tmux | passthrough disabled | `set -g allow-passthrough on` |
| Clicks land in the top-left corner | terminal lacks SGR-Pixels mouse reporting | expected; `doctor` reports it under `input` |
| macOS asks whether "Electron" may accept incoming connections | MCP/CDP mode binds a debug port from an ad-hoc-signed binary | deny it; CDP is local-only. Or `BLACKGLASS_MCP_CDP=0` |
| `doctor` exits 1 in CI | non-TTY, by design | use `blackglass version` |

#### Exit codes

`0` success. `1` runtime failure, including every non-TTY `doctor` run. `2` usage error: unknown command, or `open` with no URL. All measured.

---

## 5. Validation plan

Twenty-one checks. Everything marked RAN was executed today with the result shown. Everything marked BLOCKED names the blocker rather than assuming an outcome.

### 5.1 Build and unit gates

| # | Check | Command | Pass criterion | Status |
|---|---|---|---|---|
| V1 | Workspace builds clean | `cargo build --release` | exit 0 | RAN — PASS (18.8 s) |
| V2 | Test suite | `cargo test --workspace` | 0 failed | RAN — **96 passed, 0 failed** (bg-proto 12, bg-term 70, blackglass 14). The brief's figure of 87 is stale |
| V3 | Zero warnings | `cargo build --release 2>&1 \| grep -c warning:` | 0 | RAN — **FAIL**, 1 warning at `crates/bg-term/src/tty.rs:133` |
| V4 | Binary runs without a TTY | `blackglass version` | prints version, exit 0 | RAN — PASS |
| V5 | MSRV honoured | build under Rust 1.80 | exit 0 | BLOCKED — only 1.93.0 installed; no `rust-toolchain.toml` pins it. Add a CI job |
| V6 | Lockfile pins Electron exactly | `npm ls --depth=0` in `apps/engine` | `electron@43.2.0` | RAN — PASS, but `package.json` still says `^43.2.0`; see B10 §10 item 1 |

### 5.2 Install gates

| # | Check | Command | Pass criterion | Status |
|---|---|---|---|---|
| V7 | `npm install` alone is not sufficient | fresh dir, `npm install electron@43.2.0`, then `ls node_modules/electron/path.txt` | **absent** | RAN — CONFIRMED. 1.1 MB, no `dist/` |
| V8 | Lazy download works and is timed | `time ./node_modules/.bin/electron --version` | prints `v43.2.0` | RAN — 14.01 s **from a warm cache**. Must be re-measured cold |
| V9 | Post-install engine is real | `cat node_modules/electron/path.txt` | `Electron.app/Contents/MacOS/Electron` | RAN — PASS |
| V10 | `doctor` resolves the engine | `blackglass doctor` | engine line, not `NOT FOUND` | RAN — PASS |
| V11 | `doctor` detects a *missing browser* | bare `npm install`, then `doctor` | should report not-ready | RAN — **FAIL**, reports the engine as present (F09-2) |
| V12 | Install layout `<root>/bin` + `<root>/engine` | copy, run `doctor` | engine found under root | RAN — PASS |
| V13 | Install layout flat and nested (`libexec/bg/bin`) | as above | engine found | RAN — PASS both |
| V14 | Symlinked binary on PATH | `ln -s`, run `doctor` | should find engine | RAN — **FAIL**, `NOT FOUND` (F09-4, confirms B10 §6.5) |
| V15 | Installed binary does not reach into the build tree | `cargo install --root <tmp>`, `doctor` | should resolve locally | RAN — **FAIL**, resolves to the source tree (F09-3) |
| V16 | Bad `BLACKGLASS_ENGINE` errors loudly | `BLACKGLASS_ENGINE=/nope doctor` | non-zero, names the path | RAN — **FAIL**, silently falls back (F09-5) |
| V17 | Exit codes | version / doctor / open / bogus | 0 / 1 / 2 / 2 | RAN — PASS, all four |

### 5.3 Runtime and uninstall gates

| # | Check | Command | Pass criterion | Status |
|---|---|---|---|---|
| V18 | End-to-end first frame | `BLACKGLASS_EXIT_AFTER_MS=4000 BLACKGLASS_LOG=/tmp/bg.log blackglass open https://example.com` | log shows `first-frame`, terminal restored | BLOCKED under the agent sandbox (`bootstrap_look_up … Permission denied`). Previously verified in Ghostty 1.3.1: ready 212 ms, first frame 366 ms |
| V19 | Socket directory cleanup | `ls "$TMPDIR"/blackglass-*` after a session | none | RAN — PASS, zero CLI leftovers |
| V20 | MCP temp cleanup | same, `blackglass-mcp-*` | none | RAN — **FAIL**, 9 leftovers (356 KB) |
| V21 | Uninstall leaves nothing | run §4's uninstall, then sweep `~/Library` for files newer than install time | no BlackGlass-attributable residue | BLOCKED as a full test (would delete another agent's live data). The **inventory** it must cover is measured in F09-7 |

### 5.4 Wiring this into CI

Only V18 needs a real terminal and a real Chromium; the other twenty run headless. V1–V4, V6, V7, V9, V12–V17, V19 are pure shell and belong in a pre-merge job today. The install-layout checks (V12–V16) are the highest-value additions, because all three failures they caught are invisible to unit tests and only appear once someone tries to install the thing.

V18 needs a PTY. `BLACKGLASS_EXIT_AFTER_MS` (`main.rs:48-50`) exists precisely for this and takes the same shutdown path as `ctrl+q`, so it is a legitimate harness hook rather than a mock. Drive it under `script(1)` or a pty allocator on a self-hosted macOS runner with the sandbox off. Under the agent sandbox it cannot work: Chromium's child processes fail at `bootstrap_look_up`.

Do not assert on `doctor`'s exit code in CI. It is 1 whenever stdin is not a TTY, which is always, in CI.

---

## 6. What I need from the commander

Three changes to files I do not own, in the order I would do them.

**1. `apps/engine/src/main.js` — name the app and pin the profile.** Two lines before `app.whenReady()`:

```js
app.setName('BlackGlass');
app.setPath('userData', process.env.BLACKGLASS_PROFILE ||
            path.join(app.getPath('appData'), 'BlackGlass', 'engine-profile'));
```

This is the precondition for an honest uninstall. Until it lands, every page a user visits writes cookies into a shared directory called `Electron` that we cannot safely delete, and no uninstall documentation can fix that after the fact. B10 §8.2 and F01 §230 asked for the same change; F09 measured the data already sitting there (10 cookies, 6 hosts, 4.3 MB of cache, directory created 20:51:20 today by our first run).

**2. `apps/cli/src/main.rs:319-350` — fix engine discovery.** Three defects in one function. Canonicalise before the parent walk (`std::fs::canonicalize(exe)`), which fixes the symlink case for Homebrew, `curl | sh`, and manual PATH installs at once. Gate the baked `CARGO_MANIFEST_DIR` probe behind `#[cfg(debug_assertions)]` or move it last, so an installed binary stops preferring the developer's tree and the builder's home path stops appearing in shipped binaries. And make a set-but-wrong `BLACKGLASS_ENGINE` a hard error naming the path tried, instead of a silent fallback.

**3. `apps/cli/src/main.rs` — make `doctor` and first-run honest about the lazy download.** `doctor` should verify `<engine>/node_modules/electron/path.txt` and the file it names, not just the `.bin` symlink, so it stops passing on installs with no browser. And the 30-second connect budget at `main.rs:415` should either not swallow the child's stderr on the first attempt, or print "downloading Chromium, this may take a few minutes" when `path.txt` is absent. A 14-second warm-cache delay against a 30-second deadline, with all output sent to `/dev/null`, is a first-run experience that reads as a hang.

Also worth a moment before any release: add `LICENSE-MIT` and `LICENSE-APACHE` to match `Cargo.toml`, reconcile `packages/mcp`'s `UNLICENSED`, add a `.gitignore` for `target/` and `node_modules/`, and delete the stray `qtest` root `package.json`.
