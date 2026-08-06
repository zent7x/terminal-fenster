# Terminal logos

Real marks, not approximations. Used nominatively on the compatibility table
to identify each terminal — no endorsement is implied or claimed.

| File                  | Source                                                                 |
| --------------------- | ---------------------------------------------------------------------- |
| `ghostty.png`         | Extracted from the installed `Ghostty.app/Contents/Resources/Ghostty.icns` |
| `kitty.png`           | Extracted from the installed `kitty.app/Contents/Resources/kitty.icns`  |
| `apple-terminal.png`  | Extracted from macOS `Utilities/Terminal.app` — Apple's mark            |
| `wezterm.png`         | `github.com/wez/wezterm` → `assets/icon/terminal.png`                   |
| `iterm2.svg`          | Simple Icons (official iTerm2 brand glyph), flat fill baked in          |

Each `.icns` was converted with:

```bash
sips -s format png --resampleHeightWidthMax 256 <in>.icns --out <out>.png
```

Two notes before shipping publicly:

- **Apple Terminal.** The icon is Apple's property. Identifying compatibility is
  ordinary nominative use, but if you would rather not ship Apple artwork at all,
  drop `apple-terminal.png` and the row falls back gracefully — set `logo` to a
  neutral glyph in `src/designs/content.ts`.
- **iTerm2** ships as a monochrome glyph rather than an app icon, so its fill is
  baked to `#8e8e96`. An `<img>` cannot inherit `currentColor` from CSS, which is
  why it is not tokenised like the rest.

The previous `public/icons/*.svg` files are ~300-byte placeholders that imitated
these marks without being them. They are still referenced by the pre-redesign
`Original` design only; nothing in Desk or Chrome uses them.
