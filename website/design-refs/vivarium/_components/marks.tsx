/* Inline brand glyphs used inside the bio copy. Each one is drawn on a
   24×24 grid and inherits `currentColor`, so they sit on the text baseline
   at whatever size `.viv-mark` gives them. */

export function OpenAIMark() {
  return (
    <span className="viv-mark" aria-hidden>
      <svg viewBox="0 0 24 24" fill="currentColor">
        <path d="M22.282 9.821a5.985 5.985 0 0 0-.516-4.911 6.046 6.046 0 0 0-6.51-2.9A6.065 6.065 0 0 0 4.981 4.182a5.985 5.985 0 0 0-3.998 2.9 6.046 6.046 0 0 0 .743 7.097 5.98 5.98 0 0 0 .51 4.911 6.051 6.051 0 0 0 6.515 2.9A5.985 5.985 0 0 0 13.26 24a6.056 6.056 0 0 0 5.772-4.206 5.99 5.99 0 0 0 3.998-2.9 6.056 6.056 0 0 0-.748-7.073Zm-9.022 12.608a4.476 4.476 0 0 1-2.876-1.04l.142-.08 4.778-2.759a.795.795 0 0 0 .393-.681v-6.737l2.02 1.169a.071.071 0 0 1 .038.052v5.582a4.504 4.504 0 0 1-4.495 4.494ZM3.6 18.304a4.471 4.471 0 0 1-.535-3.014l.142.085 4.783 2.758a.771.771 0 0 0 .78 0l5.843-3.368v2.332a.08.08 0 0 1-.033.062L9.74 19.95a4.499 4.499 0 0 1-6.14-1.646ZM2.34 7.896a4.485 4.485 0 0 1 2.366-1.973v5.677a.766.766 0 0 0 .388.677l5.814 3.354-2.02 1.169a.076.076 0 0 1-.071 0l-4.83-2.787A4.504 4.504 0 0 1 2.34 7.872Zm16.597 3.856-5.833-3.388 2.015-1.164a.076.076 0 0 1 .071 0l4.83 2.791a4.494 4.494 0 0 1-.676 8.105v-5.678a.79.79 0 0 0-.407-.666Zm2.01-3.023-.141-.085-4.774-2.782a.776.776 0 0 0-.785 0L9.41 9.23V6.897a.066.066 0 0 1 .028-.061l4.83-2.787a4.499 4.499 0 0 1 6.68 4.66ZM8.307 12.863l-2.02-1.164a.08.08 0 0 1-.038-.057V6.074a4.499 4.499 0 0 1 7.376-3.454l-.142.08-4.778 2.76a.795.795 0 0 0-.393.68Zm1.097-2.366 2.602-1.5 2.607 1.5v2.999l-2.597 1.5-2.607-1.5Z" />
      </svg>
    </span>
  );
}

export function VercelMark() {
  return (
    <span className="viv-mark" aria-hidden>
      <svg viewBox="0 0 24 24" fill="currentColor">
        <path d="M12 2.4 23.1 21.6H.9Z" />
      </svg>
    </span>
  );
}

export function PalantirMark() {
  // A ring sitting on a short plinth — the mark reads as a lens on a stand
  // at 10px, which is all the reference ever shows of it.
  return (
    <span className="viv-mark" aria-hidden>
      <svg viewBox="0 0 24 24" fill="none">
        <circle
          cx="12"
          cy="10.2"
          r="7.4"
          stroke="currentColor"
          strokeWidth="2.4"
        />
        <rect x="6.2" y="18.5" width="11.6" height="2.6" rx="1.3" fill="currentColor" />
      </svg>
    </span>
  );
}

export function XMark() {
  // Double-struck X: the top-left→bottom-right stroke is hollowed into two
  // hairlines offset along the diagonal, the counter-diagonal stays single.
  // Coordinates traced off the capture's 8×8px of ink.
  return (
    <span className="viv-mark viv-mark--x" aria-hidden>
      <svg viewBox="0 0 24 24" fill="none">
        <g stroke="currentColor" strokeWidth="2.6" strokeLinecap="butt">
          <path d="M3.25 2 15.75 22" />
          <path d="M8.25 2 20.75 22" />
          <path d="M18.25 2 3.25 22" />
        </g>
      </svg>
    </span>
  );
}
