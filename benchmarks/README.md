# Terminal-Fenster benchmark harness

Measures the real binary — `target/release/terminal-fenster` — end to end, using the
`TERMINAL_FENSTER_LOG` and `TERMINAL_FENSTER_EXIT_AFTER_MS` hooks that already exist in
`apps/cli/src/main.rs`. No mocks, no simulated frames.

It reports:

| Metric | Source |
| --- | --- |
| cold / warm start to first frame | wall clock from `spawn()` to the `first-frame` log line |
| engine frames received | `bounded-run complete frames=…` |
| presentations and displayed fps | `frame-stats samples=…` plus bounded-run timestamps |
| BGRA conversion p50 / p99 | `frame-stats … convert_ms_p50=… convert_ms_p99=…` |
| encode p50 / p99 | `frame-stats … encode_ms_p50=… encode_ms_p99=…` |
| wire bytes p50 / p99 | `frame-stats … wire_bytes_p50=… wire_bytes_p99=…` |
| presentation-gap p50 / p99 | `frame-stats … gap_ms_p50=… gap_ms_p99=…` |
| process RSS | `ps`, sampled across the whole process tree |

Output is a JSON document plus a readable summary.

Conversion is timed separately from protocol encoding. When multiple engine damage frames are
coalesced before one terminal presentation, the conversion sample is their accumulated cost; the
status row shows conversion plus encoding as renderer CPU time. Legacy logs without conversion
fields remain parseable and report those values as unavailable rather than zero.

For a terminal-independent engine memory baseline, run:

```sh
node benchmarks/engine-rss.js --duration 8000 --json
```

It drives the same private engine socket, sums RSS across the Chromium process tree, and refuses
to pass if any PID it observed survives shutdown. On the reference M4 host, `about:blank` at
1280×800/60 fps measured 280.6 MB peak and steady RSS over a short six-sample smoke run. macOS RSS
double-counts shared pages, so this is explicitly an upper bound rather than physical footprint.

---

## Requirements

**1. A graphics-capable terminal, run interactively.** Ghostty is the verified
one on this machine (kitty graphics, kitty keyboard, `CSI 16 t` cell size all
confirmed). kitty and WezTerm should also work but are **UNVERIFIED** here.

This is not a preference, it is structural:

* `terminal-fenster open` calls `isatty(stdin)` and refuses to run otherwise
  (`apps/cli/src/main.rs:228`).
* Capability detection works by asking the terminal questions. The query bytes
  are written to **stdout** and the replies are read from **stdin**
  (`crates/tf-term/src/caps.rs:117-120`).

So **stdout must not be redirected**. Piping it to a file or another process
means the terminal never sees the queries, never replies, cell size stays
unknown, and `cmd_open` bails at "could not determine terminal pixel size"
(`main.rs:244-251`) before writing a single log line. The harness therefore
inherits stdin and stdout, captures only stderr, and takes every measurement
from `$TERMINAL_FENSTER_LOG` — which is exactly why that hook exists: *"Logging must
never go to stdout while browsing: stdout is the graphics channel"*
(`main.rs:29-31`).

Because nothing is read off the screen, the harness needs no visible display
and is CI-able wherever a real PTY on a graphics-capable terminal is available.

**2. The agent sandbox must be disabled.** Chromium child processes fail under
it with `bootstrap_look_up … Permission denied`. Run the harness from a normal
interactive shell, not from an agent tool call.

**3. Node.js.** Verified against v24.11.1. Zero dependencies.

**4. Not inside tmux or screen.** A multiplexer rewrites or swallows graphics
escapes, which changes what is being measured. Preflight warns.

---

## How to run it

Open **Ghostty**, then:

```sh
cd $REPO
cargo build --release                 # if target/release/terminal-fenster is stale
node benchmarks/bench.mjs
```

That is the whole thing. Defaults are 5 runs of 8000 ms each against a local
repaint page, RSS sampled every 250 ms.

Check everything is wired up without launching a browser:

```sh
node benchmarks/bench.mjs --dry-run     # preflight + the exact plan, executes nothing
node benchmarks/bench.mjs --self-test   # verifies the log parser against fixtures
```

Both are safe to run anywhere, including under the agent sandbox. `--dry-run`
exits non-zero and explains itself when a requirement is missing.

### Useful variations

```sh
# longer, more runs, tagged
node benchmarks/bench.mjs --runs 10 --duration-ms 15000 --label baseline

# no-damage control: frames should stay in the low single digits
node benchmarks/bench.mjs --page static

# dense local fast path (uses Kitty shared memory only when its real-object probe succeeds)
node benchmarks/bench.mjs --page repaint --label shm-dense

# apples-to-apples direct/zlib fallback control
TERMINAL_FENSTER_SHM=0 TERMINAL_FENSTER_TILE_CELLS=1x1 \
  node benchmarks/bench.mjs --page repaint --label direct-control

# exercise the Unicode half-block encoder instead of kitty
node benchmarks/bench.mjs --backend unicode

# a real site (results then depend on the network)
node benchmarks/bench.mjs --url https://example.com

# machine-readable only
node benchmarks/bench.mjs --json-only > run.json
```

`node benchmarks/bench.mjs --help` lists every option.

### Output

```
benchmarks/results/terminal-fenster-bench-<iso-timestamp>[-label].json
benchmarks/results/latest.json        # same content, stable path for CI
benchmarks/results/logs/run-N.log     # raw TERMINAL_FENSTER_LOG per run
```

Exit code is `0` when every run is valid, `1` when any run failed or preflight
blocked, `2` on bad usage.

---

## What the numbers mean

Start is broken into three parts that sum to the headline figure:

```
spawn_to_first_frame_ms
├── tty_and_detect_ms          TtyGuard acquisition + the capability handshake
├── engine_spawn_connect_ms    Electron spawn + Unix socket accept ("engine ready")
└── first_frame_after_run_start_ms   run loop start → first frame drawn
```

The first two are measured across the process boundary by comparing the
harness's `Date.now()` with the log's timestamp prefix. Both read the same Unix
millisecond clock (`main.rs:34-37`), so they are directly comparable.

**`fps_steady_state`** is `(completed terminal presentations - 1) / (first
frame → end of window)`. It excludes load time and does not count multiple
socket frames that the core coalesces into one presentation, so it is the
number to quote. `fps_over_window` includes load time and is pessimistic.
`fps_received_steady_state` and `fps_logged_instantaneous` describe frames
received from the engine; the latter is only the binary's trailing-one-second
sample, not an average.

**Cold vs warm.** `cold` is run 0, the first launch of the series. `warm` is
runs 1..n-1, launched with the Electron framework already in the page cache.
The harness does **not** purge the OS page cache, so "cold" means *first
launch*, not *cold disk*. For a true cold-cache number, run `sudo purge` by hand
immediately before the harness; it is deliberately not automated because it
needs a password prompt and evicts the whole machine's cache.

---

## Known limitations

These are properties of the current binary and the platform, not oversights.
Every one of them is also recorded in the JSON so a reader cannot miss it.

**1. `compression_ratio_approx` is approximate by construction.** The numerator
is the *first* frame's raw size and the denominator the median presented frame's
encoded size (or the legacy final-frame sample when parsing an older log). Damage
size can change during a run, so it remains an order-of-magnitude figure. The harness verifies
`payload_bytes == width * height * 4 + 32` and fails the run if the frame wire
format ever drifts, rather than quietly reporting a meaningless ratio.

**2. RSS over-counts.** `tree_total` sums per-process RSS across the tree.
Chromium's helpers share many pages, so read it as an upper bound. Proportional
set size would be better but macOS does not expose it cheaply.

**3. RSS resolution is bounded by how slow `ps` is.** Measured on this machine
(~700 processes, macOS 26.1, Apple M4):

| command | cost |
| --- | --- |
| `ps -Ao pid=,ppid=,rss=,comm=` | 450–1150 ms |
| `ps -Ao pid=,ppid=,rss=,ucomm=` | 36–223 ms |
| `ps -o pid=,rss= -p <40 pids>` | 317–657 ms |

Targeting specific PIDs is no cheaper, so the cost is `ps` startup, not the
number of processes inspected. The sampler therefore uses `ucomm` and takes one
full-path snapshot per run only to label the breakdown. It always reports
`achieved_interval_ms_p50` alongside `requested_interval_ms`, so the resolution
of the peak is never overstated. A spike shorter than the achieved interval is
invisible.

**4. Leaked helpers are a warning, not a failure.** `shutdown()` gives the
engine 1.5 s before SIGKILL (`main.rs:662-675`) and Chromium unwinds on its own
schedule, so the harness drains for up to 6 s before declaring a leak. In
testing, a tree took 2489 ms to fully exit — a single short probe would have
reported a phantom leak on every healthy run. A surviving helper is a real
defect worth surfacing, but it does not make the timing numbers wrong.

**5. `--url` against a real site measures the network too.** The default page is
local (`file://`), so runs are reproducible and offline.

Per-frame samples are retained only when `TERMINAL_FENSTER_EXIT_AFTER_MS` enables a
bounded run. Normal interactive browsing does not accumulate metric history.
Encode time stops before the stdout write; presentation gaps are timestamped
after each flush, so they include terminal-output backpressure.

For a shared-memory dense frame, `wire_bytes` is intentionally only the Kitty command carried
by the PTY; the raw RGB bytes live in the 0600 POSIX object reported by `t=s`. Compare displayed
FPS and presentation gaps against the `TERMINAL_FENSTER_SHM=0` control, not just wire bytes. A generic
Kitty reply is insufficient: Terminal-Fenster enables this path only after the terminal successfully
opens a real 1×1 shared-memory probe, and disables it over SSH/tmux/screen.

---

## Files

```
benchmarks/bench.mjs           the harness
benchmarks/pages/repaint.html  full-viewport repaint load (default page)
benchmarks/pages/local-damage.html  small animated region (mosaic fast path)
benchmarks/pages/static.html   no-damage control
benchmarks/results/            output (created on first run)
```

`bench.mjs` exports `parseRunLog`, `runOnce` and `summarize`, so the parser can
be imported and tested without launching a browser.
