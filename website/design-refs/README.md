# Design references

Source blocks pulled from `ui.satviks.dev` and kept here as reference only —
they are **not** part of the build.

```
npx shadcn@latest add "https://ui.satviks.dev/r/nixole.json?key=…"
npx shadcn@latest add "https://ui.satviks.dev/r/vivarium.json?key=…"
npx shadcn@latest add "https://ui.satviks.dev/r/remix.json?key=…"
```

They are Next.js App Router pages that reproduce three existing landing pages
pixel-for-pixel, so they cannot be dropped into this Vite SPA directly:

- absolutely positioned against fixed canvases (1600×1200, 1088×720, 1042px)
- reference assets this repo does not ship (`/nixole/hero.png`, `/remix/monet.jpg`,
  `/fonts/SF-Pro-*.woff2`)
- carry the original products' copy (a medical-AI SaaS, a personal site, a
  mini-app startup)
- `vivarium.json` declares a dependency on a package called `cutting`, which does
  not exist on npm — `shadcn add` aborts on it, so its files were extracted from
  the registry JSON by hand

What was actually taken from them lives in `src/designs/`, one directory per
design, each documented at the top of its stylesheet:

| Reference  | Design            | What carried over                                              |
| ---------- | ----------------- | -------------------------------------------------------------- |
| `nixole`   | `src/designs/desk`   | Floating-sheet shadow ramp, chip-in-pill badge, masked marquee, blur-rise entrance |
| `vivarium` | `src/designs/glass`  | `--u` reference-pixel scaling, glass frame ring, still-until-pointed-at media |
| `remix`    | `src/designs/chrome` | Four-layer control shadows with a 1px inset highlight, tight display tracking, fanned card stack |
