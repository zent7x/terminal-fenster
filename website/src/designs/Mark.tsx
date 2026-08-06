/**
 * The Terminal-Fenster mark, as an inline SVG.
 *
 * `public/assets/logo.svg` is a full lockup — a 240x48 canvas with the
 * wordmark baked in — so rendering it at icon size squashes the type into an
 * illegible smear. This is just the glyph: a window outline holding a grid of
 * cells with the bottom-right one missing (the caret's seat).
 *
 * Colour comes from the surrounding design: the frame takes `currentColor`,
 * the cells take `--mark-cell` and fade along the grid.
 */
export function Mark({ size = 28, className }: { size?: number; className?: string }) {
  const cells: Array<[number, number]> = [
    [8, 12],
    [18, 12],
    [28, 12],
    [8, 22],
    [18, 22],
    [28, 22],
    [8, 32],
    [18, 32],
  ];

  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 40 48"
      fill="none"
      role="img"
      aria-label="Terminal-Fenster"
    >
      <rect
        x="2"
        y="6"
        width="36"
        height="36"
        rx="7"
        stroke="currentColor"
        strokeWidth="2.5"
      />
      {cells.map(([x, y], i) => (
        <rect
          key={`${x}-${y}`}
          x={x}
          y={y}
          width="8"
          height="8"
          rx="1.5"
          fill="var(--mark-cell, currentColor)"
          /* Fade down and to the right so the empty seat reads as intentional. */
          opacity={0.92 - i * 0.075}
        />
      ))}
    </svg>
  );
}
