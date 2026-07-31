# BlackGlass benchmark harness

Measures the real binary — `target/release/blackglass` — end to end, using the
`BLACKGLASS_LOG` and `BLACKGLASS_EXIT_AFTER_MS` hooks that already exist in
`apps/cli/src/main.rs`. No mocks, no simulated frames.

It reports:

| Metric | Source |
| --- | --- |
| cold / warm start to first frame | wall clock from `spawn()` to the `first-frame` log line |
| frames and fps | `bounded-run complete frames=…` |
| encode milliseconds | `bounded-run complete … encode_ms=…` |
| wire bytes per frame | `bounded-run complete … last_wire_bytes=…` |
| process RSS | `ps`, sampled across the whole process tree |

Output is a JSON document plus a readable summary.

---

## Requirements

**1. A graphics-capable terminal, run interactively.** Ghostty is the verified
one on this machine (kitty graphics, kitty keyboard, `CSI 16 t` cell size all
confirmed). kitty and WezTerm should also work but are **UNVERIFIED** here.

This is not a preference, it is structural:

* `blackglass open` calls `isatty(stdin)` and refuses to run otherwise
  (`apps/cli/src/main.rs:228`).
* Capability detection works by asking the terminal questions. The query bytes
  are written to **stdout** and the replies are read from **stdin**
  (`crates/bg-term/src/caps.rs:117-120`).

So **stdout must not be redirected**. Piping it to a file or another process
means the terminal never sees the queries, never replies, cell size stays
unknown, and `cmd_open` bails at "could not determine terminal pixel size"
(`main.rs:244-251`) before writing a single log line. The harness therefore
inherits stdin and stdout, captures only stderr, and takes every measurement
from `$BLACKGLASS_LOG` — which is exactly why that hook exists: *"Logging must
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
cd /Users/adeebbashir/projects/blackglass
cargo build --release                 # if target/release/blackglass is stale
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
benchmarks/results/blackglass-bench-<iso-timestamp>[-label].json
benchmarks/results/latest.json        # same content, stable path for CI
benchmarks/results/logs/run-N.log     # raw BLACKGLASS_LOG per run
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

**`fps_steady_state`** is `(frames - 1) / (first frame → end of window)`. It
excludes load time, so it describes the pipeline rather than the page. It is the
number to quote. `fps_over_window` includes load time and is pessimistic.
`fps_logged_instantaneous` is the binary's own trailing-one-second count
(`main.rs:842-844`) — a sample, not an average.

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

**1. Encode time and wire bytes are single samples, not distributions.**
The CLI logs only the *final* frame's `encode_ms` and `last_wire_bytes`
(`main.rs:467-470`). Nothing emits a per-frame series, so no p50/p99 over frames
can be computed from the current binary — only one value per run, aggregated
across runs. Fixing this needs a small change to the CLI's logging (see below);
it cannot be fixed in the harness.

**2. `compression_ratio_approx` is approximate by construction.** The numerator
is the *first* frame's raw size and the denominator the *last* frame's encoded
size. It is an order-of-magnitude figure. The harness verifies
`payload_bytes == width * height * 4 + 32` and fails the run if the frame wire
format ever drifts, rather than quietly reporting a meaningless ratio.

**3. RSS over-counts.** `tree_total` sums per-process RSS across the tree.
Chromium's helpers share many pages, so read it as an upper bound. Proportional
set size would be better but macOS does not expose it cheaply.

**4. RSS resolution is bounded by how slow `ps` is.** Measured on this machine
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

**5. Leaked helpers are a warning, not a failure.** `shutdown()` gives the
engine 1.5 s before SIGKILL (`main.rs:662-675`) and Chromium unwinds on its own
schedule, so the harness drains for up to 6 s before declaring a leak. In
testing, a tree took 2489 ms to fully exit — a single short probe would have
reported a phantom leak on every healthy run. A surviving helper is a real
defect worth surfacing, but it does not make the timing numbers wrong.

**6. `--url` against a real site measures the network too.** The default page is
local (`file://`), so runs are reproducible and offline.

---

## Recommended change to the CLI (not made here — core files are owned elsewhere)

Limitation 1 is the only one worth fixing in code, and it is small and
log-only. `Renderer::present` already computes `last_encode_ms` and
`wire_bytes` for every frame (`main.rs:857-881`); they are simply overwritten
each time. Accumulating them into two `Vec<f64>` / `Vec<usize>` on `Renderer`
and emitting one extra line at the end of the bounded run —

```
frame-stats encode_ms_p50=… encode_ms_p99=… wire_bytes_p50=… wire_bytes_p99=… gap_ms_p50=… gap_ms_p99=…
```

— would turn every "one sample per run" figure above into a real distribution,
and would let the harness report frame-gap percentiles directly comparable to
the p50 16.65 ms / p99 19.94 ms already measured at the engine. This changes no
product behaviour: it is one more `log_line` on the existing bounded-run path,
which is already env-gated and off by default.

The harness parser is ready for it — add the line and it can be consumed
without restructuring anything.

---

## Files

```
benchmarks/bench.mjs           the harness
benchmarks/pages/repaint.html  full-viewport repaint load (default page)
benchmarks/pages/static.html   no-damage control
benchmarks/results/            output (created on first run)
```

`bench.mjs` exports `parseRunLog`, `runOnce` and `summarize`, so the parser can
be imported and tested without launching a browser.
