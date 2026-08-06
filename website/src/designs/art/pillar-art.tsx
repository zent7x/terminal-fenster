/**
 * Pillar illustrations — large measured line-art, one per capability.
 *
 * Approach borrowed from the `cores` design in the UI library: a tall card
 * whose top half is a single big drawing, not a 20px icon in a tinted square.
 * All three are built on one 360×300 grid with a shared stroke language
 * (1.5 structural / 2 emphasis, round caps and joins) so they read as a set,
 * and each fills the frame rather than floating in the middle of it.
 *
 * Colour is inherited, never hardcoded — each design supplies:
 *   --art-line    structural strokes      --art-soft   secondary strokes
 *   --art-fill    filled surfaces         --art-edge   surface outlines
 *   --art-accent  the single accent       --art-accent-soft  its wash
 */

const S = {
  fill: 'none',
  stroke: 'var(--art-line)',
  strokeWidth: 1.5,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
};

const SOFT = { ...S, stroke: 'var(--art-soft)' };
const ACCENT = { ...S, stroke: 'var(--art-accent)', strokeWidth: 2 };

type ArtProps = { className?: string };

/**
 * Real Chromium — one window whose page resolves downward from smooth
 * rendered pixels into discrete terminal cells. The accent rule is the
 * handoff: above it Chromium, below it your TTY. Same content either side.
 */
export function EngineArt({ className }: ArtProps) {
  const COLS = 19;
  const X0 = 48;
  const Y0 = 174;
  const P = 14;

  /* Row densities echo the text block drawn above the rule, so the two halves
     are legibly the same page at two resolutions. */
  const runs: Array<Array<[number, number]>> = [
    [[0, 11]],
    [[0, 17]],
    [[0, 15]],
    [[0, 8]],
    [[9, 17]],
    [[0, 5], [7, 12]],
    [[0, 9]],
  ];

  const cells: React.ReactElement[] = [];
  runs.forEach((row, r) => {
    row.forEach(([a, b]) => {
      for (let c = a; c <= b && c < COLS; c++) {
        const lit = r <= 1;
        cells.push(
          <rect
            key={`${r}-${c}`}
            x={X0 + c * P}
            y={Y0 + r * P}
            width={P - 3.5}
            height={P - 3.5}
            rx={1.6}
            fill={lit ? 'var(--art-accent-soft)' : 'var(--art-fill)'}
            stroke={lit ? 'var(--art-accent)' : 'var(--art-edge)'}
            strokeWidth={1}
          />,
        );
      }
    });
  });

  return (
    <svg viewBox="0 0 360 300" className={className} aria-hidden>
      <rect x={34} y={24} width={292} height={252} rx={15} {...S} fill="var(--art-fill)" />
      <path d="M34 58 H326" {...S} />
      <circle cx={50} cy={41} r={4} {...SOFT} />
      <circle cx={64} cy={41} r={4} {...SOFT} />
      <circle cx={78} cy={41} r={4} {...SOFT} />
      <rect x={100} y={34} width={152} height={14} rx={7} {...SOFT} />

      {/* smooth half — real rendered content */}
      <rect x={48} y={78} width={152} height={11} rx={5.5} fill="var(--art-soft)" opacity={0.5} />
      <rect x={48} y={98} width={238} height={8} rx={4} fill="var(--art-soft)" opacity={0.3} />
      <rect x={48} y={113} width={210} height={8} rx={4} fill="var(--art-soft)" opacity={0.3} />
      <rect x={48} y={128} width={112} height={8} rx={4} fill="var(--art-soft)" opacity={0.3} />
      <rect x={206} y={124} width={80} height={28} rx={7} {...SOFT} />

      {/* the handoff */}
      <path d="M34 162 H326" {...ACCENT} />
      <circle cx={326} cy={162} r={4} fill="var(--art-accent)" />

      {/* quantised half — the same page, as terminal cells */}
      {cells}
    </svg>
  );
}

/**
 * Your input, unchanged — a pointer landing on a live control, and the key
 * events routed back up the right-hand side into the same page.
 */
export function InputArt({ className }: ArtProps) {
  return (
    <svg viewBox="0 0 360 300" className={className} aria-hidden>
      {/* the page receiving the events */}
      <rect x={34} y={22} width={252} height={158} rx={14} {...S} fill="var(--art-fill)" />
      <path d="M34 54 H286" {...SOFT} />
      <circle cx={50} cy={38} r={3.4} {...SOFT} />
      <circle cx={62} cy={38} r={3.4} {...SOFT} />
      <rect x={50} y={72} width={122} height={9} rx={4.5} fill="var(--art-soft)" opacity={0.42} />
      <rect x={50} y={90} width={182} height={7} rx={3.5} fill="var(--art-soft)" opacity={0.26} />

      {/* the target control, lit, with the click landing on it */}
      <rect
        x={50}
        y={118}
        width={116}
        height={36}
        rx={9}
        stroke="var(--art-accent)"
        strokeWidth={2}
        fill="var(--art-accent-soft)"
      />
      <circle cx={108} cy={136} r={21} {...ACCENT} opacity={0.5} />
      <circle cx={108} cy={136} r={33} {...ACCENT} opacity={0.18} />
      <path
        d="M108 136 L108 186 L121 173 L131 194 L142 189 L132 168 L150 166 Z"
        fill="var(--art-fill)"
        stroke="var(--art-line)"
        strokeWidth={2}
        strokeLinejoin="round"
      />

      {/* keycaps */}
      <rect x={34} y={236} width={46} height={46} rx={10} {...S} fill="var(--art-fill)" />
      <rect x={90} y={236} width={46} height={46} rx={10} {...S} fill="var(--art-fill)" />
      <rect x={146} y={236} width={104} height={46} rx={10} {...S} fill="var(--art-fill)" />
      <path d="M50 259 h14 M106 259 h14" {...SOFT} strokeWidth={2} />
      <path d="M180 259 h36" {...SOFT} strokeWidth={2} />

      {/* the events, routed back up into the page — not into a dead end */}
      <path d="M250 259 H306 A16 16 0 0 0 322 243 V196" {...ACCENT} />
      <path d="M314 206 L322 194 L330 206" {...ACCENT} />
    </svg>
  );
}

/**
 * Automatable — the sixteen stdio tools, drawn as sixteen countable chips on
 * a bus that runs back to the agent. A radial burst read as decoration; a
 * grid reads as an inventory, which is what it is.
 */
export function AgentArt({ className }: ArtProps) {
  const COLS = 4;
  const ROWS = 4;
  const W = 48;
  const H = 38;
  const GX = 10;
  const GY = 13;
  /* Right edge lands at 118 + 3*58 + 48 = 340, inside the 360 frame — the
     grid used to run off it. */
  const X0 = 118;
  const Y0 = 58;
  const LIT = 5; // second row, second column

  const chips: React.ReactElement[] = [];
  for (let r = 0; r < ROWS; r++) {
    for (let c = 0; c < COLS; c++) {
      const i = r * COLS + c;
      const x = X0 + c * (W + GX);
      const y = Y0 + r * (H + GY);
      const on = i === LIT;
      chips.push(
        <g key={i}>
          <rect
            x={x}
            y={y}
            width={W}
            height={H}
            rx={9}
            fill={on ? 'var(--art-accent-soft)' : 'var(--art-fill)'}
            stroke={on ? 'var(--art-accent)' : 'var(--art-edge)'}
            strokeWidth={on ? 2 : 1.4}
          />
          {/* each chip carries a two-line "signature", the lit one brighter */}
          <rect
            x={x + 11}
            y={y + 13}
            width={20}
            height={4}
            rx={2}
            fill={on ? 'var(--art-accent)' : 'var(--art-line)'}
            opacity={on ? 0.9 : 0.7}
          />
          <rect
            x={x + 11}
            y={y + 22}
            width={12}
            height={4}
            rx={2}
            fill={on ? 'var(--art-accent)' : 'var(--art-line)'}
            opacity={on ? 0.55 : 0.38}
          />
        </g>,
      );
    }
  }

  const BUS_X = 110;
  const BUS_TOP = Y0 + H / 2;
  const BUS_BOT = Y0 + 3 * (H + GY) + H / 2;

  return (
    <svg viewBox="0 0 360 300" className={className} aria-hidden>
      {/* the bus, and a stub into every row */}
      <path d={`M${BUS_X} ${BUS_TOP} V${BUS_BOT}`} {...SOFT} />
      {[0, 1, 2, 3].map((r) => {
        const y = Y0 + r * (H + GY) + H / 2;
        const on = r === 1;
        return (
          <path
            key={r}
            d={`M${BUS_X} ${y} H${X0}`}
            stroke={on ? 'var(--art-accent)' : 'var(--art-soft)'}
            strokeWidth={on ? 2 : 1.2}
            strokeLinecap="round"
          />
        );
      })}

      {chips}

      {/* the agent, wired into the bus */}
      <path d={`M${BUS_X} ${BUS_TOP} V46 H72`} {...SOFT} />
      <path d={`M${BUS_X} ${BUS_BOT} V254 H72`} {...SOFT} />
      <rect x={14} y={28} width={58} height={36} rx={10} {...S} fill="var(--art-fill)" />
      <rect x={14} y={236} width={58} height={36} rx={10} {...S} fill="var(--art-fill)" />

      {/* stdio: one pipe in, one pipe out */}
      <path d="M29 46 h12 M51 41 l7 5 -7 5" {...ACCENT} />
      <path d="M29 254 h12 M58 249 l-7 5 7 5" {...SOFT} strokeWidth={2} />

      {/* the socket the whole thing speaks over */}
      <circle cx={BUS_X} cy={BUS_TOP} r={4} fill="var(--art-accent)" />
      <circle cx={BUS_X} cy={BUS_BOT} r={3.5} {...SOFT} fill="var(--art-fill)" />
    </svg>
  );
}
