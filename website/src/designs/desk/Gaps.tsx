import { useState } from 'react';
import { GAPS } from '@/designs/content';

/**
 * The known-gaps list, as expandable numbered rows on hairline rules.
 *
 * Structure from the `plinth` block in the UI library. Shipping this at all is
 * the point: an alpha that publishes its own nine unresolved problems is more
 * credible than one that publishes a feature grid, so it gets a real component
 * rather than a footnote.
 */
export function Gaps() {
  const [open, setOpen] = useState<number | null>(0);

  return (
    <ul className="dk-gaps">
      {GAPS.map((g, i) => {
        const isOpen = open === i;
        return (
          <li className="dk-gap" key={g.title} data-open={isOpen || undefined}>
            <button
              type="button"
              className="dk-gap-row"
              aria-expanded={isOpen}
              onClick={() => setOpen(isOpen ? null : i)}
            >
              <span className="dk-gap-n">{String(i + 1).padStart(2, '0')}</span>
              <span className="dk-gap-title">{g.title}</span>
              <span className="dk-gap-mark" aria-hidden>
                <svg viewBox="0 0 14 14" width="14" height="14">
                  <path
                    d="M7 2.5 V11.5 M2.5 7 H11.5"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.6"
                    strokeLinecap="round"
                  />
                </svg>
              </span>
            </button>

            {/* Grid-rows animates height without measuring it in JS. */}
            <div className="dk-gap-panel">
              <div>
                <p>{g.body}</p>
              </div>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
