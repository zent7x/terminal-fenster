# Hero capture

The hero shows `demo.gif` (still poster: `demo.png`) — a genuine Ghostty
recording of Terminal-Fenster, 960×726, 64 frames.

## Replacing it

The hero reads these two paths and nothing else, so a better recording is a
drop-in swap — no code change:

```
public/assets/demo.png    still poster, shown until you hover
public/assets/demo.gif    the recording, attached only on hover
```

Keep the transparent padding around the window chrome if you can. The hero casts
its shadow through the alpha channel (`filter: drop-shadow`) rather than drawing
a box, which is what lets the capture sit on the page as an object.

## Capturing a better one

The current recording is of Hacker News, which is a weak argument for a product
whose whole pitch is "real pixels, not a text-mode approximation" — an orange
list of links is exactly what a text-mode browser can already do. Something
visual makes the case far better.

On a machine with the toolchain (Rust 1.80+, Node 22.12+), in Ghostty:

```bash
terminal-fenster open <a visually rich page>
```

Then record the window. Good candidates are pages with photography, real
typography and layout — anything where "this is actually Chromium" is obvious at
a glance and impossible in Lynx.

Render at 2× and downsample if you want it crisp above ~1120px, which is the
width the hero caps at today (`.ht-frame` in `src/designs/hero/hero-terminal.css`).
Raise that cap once the source resolution supports it.
