import { useCallback, useId, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';
import { INSTALL_PATHS, type InstallPathId } from '@/designs/content';

/**
 * Install command block with a path switcher.
 *
 * Tablist mechanics come from the `incant` design: the active pill is a
 * shared-layout element so it glides between tabs, and the panel swaps on a
 * short blur rather than a hard crossfade (at 13px two overlapping monospace
 * runs read as a smudge).
 *
 * The layout is a two-column split — numbered code on the left, what the path
 * actually does on the right. A single wide code slab left more than half the
 * width empty and pushed its explanation into a grey footer line that read as
 * a disclaimer.
 *
 * Presentation is left to the host design via `prefix`, which namespaces every
 * class, so Desk and Chrome style the same component to their own surface.
 */

const IN = { duration: 0.22, ease: [0.22, 1, 0.36, 1] as const };
const OUT = { duration: 0.16, ease: [0.4, 0, 1, 1] as const };

export function CommandBlock({ prefix }: { prefix: string }) {
  const reduce = usePrefersReducedMotion();
  const uid = useId();
  const tabsRef = useRef<HTMLDivElement>(null);
  const [activeId, setActiveId] = useState<InstallPathId>(INSTALL_PATHS[0].id);
  const [copied, setCopied] = useState(false);

  const active = INSTALL_PATHS.find((p) => p.id === activeId) ?? INSTALL_PATHS[0];
  const panelId = `${uid}-panel`;

  /* Roving focus: arrows move and activate, Home/End jump to the ends. */
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      const keys = ['ArrowLeft', 'ArrowRight', 'Home', 'End'];
      if (!keys.includes(e.key)) return;
      e.preventDefault();
      const i = INSTALL_PATHS.findIndex((p) => p.id === activeId);
      const last = INSTALL_PATHS.length - 1;
      const next =
        e.key === 'Home'
          ? 0
          : e.key === 'End'
            ? last
            : e.key === 'ArrowLeft'
              ? (i - 1 + INSTALL_PATHS.length) % INSTALL_PATHS.length
              : (i + 1) % INSTALL_PATHS.length;
      setActiveId(INSTALL_PATHS[next].id);
      tabsRef.current?.querySelectorAll<HTMLButtonElement>("[role='tab']")[next]?.focus();
    },
    [activeId],
  );

  async function copy() {
    /* Only the commands — copying the comments would break a paste. */
    const text = active.lines
      .filter((l) => l.kind === 'cmd')
      .map((l) => l.text)
      .join('\n');
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      window.prompt('Copy the install commands:', text);
    }
  }

  return (
    <div className={`${prefix}-cmd`}>
      <div className={`${prefix}-cmd-bar`}>
        <div
          className={`${prefix}-tabs`}
          ref={tabsRef}
          role="tablist"
          aria-label="Install path"
          onKeyDown={onKeyDown}
        >
          {INSTALL_PATHS.map((p) => {
            const on = p.id === active.id;
            return (
              <button
                key={p.id}
                type="button"
                id={`${uid}-${p.id}`}
                role="tab"
                aria-selected={on}
                aria-controls={panelId}
                tabIndex={on ? 0 : -1}
                className={`${prefix}-tab`}
                data-active={on || undefined}
                onClick={() => setActiveId(p.id)}
              >
                {on && (
                  <motion.span
                    layoutId={`${uid}-pill`}
                    className={`${prefix}-tab-pill`}
                    transition={
                      reduce
                        ? { duration: 0 }
                        : { type: 'spring', stiffness: 420, damping: 38, mass: 1 }
                    }
                  />
                )}
                <span className={`${prefix}-tab-label`}>{p.label}</span>
              </button>
            );
          })}
        </div>

        <button
          className={`${prefix}-copy`}
          type="button"
          onClick={copy}
          data-copied={copied || undefined}
          aria-label={`Copy the ${active.label} commands`}
        >
          <span className={`${prefix}-copy-mark`}>
            {copied ? (
              <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
                <path
                  d="M3.5 8.5 L6.5 11.5 L12.5 4.5"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.9"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            ) : (
              <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
                <rect
                  x="5.5"
                  y="5.5"
                  width="8"
                  height="8"
                  rx="2"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                />
                <path
                  d="M10.5 3.5 h-7 a1 1 0 0 0 -1 1 v7"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                />
              </svg>
            )}
          </span>
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>

      <div className={`${prefix}-cmd-body`} id={panelId} role="tabpanel">
        <AnimatePresence initial={false} mode="wait">
          <motion.div
            key={active.id}
            className={`${prefix}-cmd-split`}
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 5, filter: 'blur(3px)' }}
            animate={{
              opacity: 1,
              y: 0,
              filter: 'blur(0px)',
              transition: reduce ? { duration: 0 } : IN,
            }}
            exit={
              reduce
                ? { opacity: 0, transition: { duration: 0 } }
                : { opacity: 0, y: -5, filter: 'blur(3px)', transition: OUT }
            }
          >
            <ol className={`${prefix}-code`}>
              {active.lines.map((line, i) => (
                <li key={i} data-kind={line.kind}>
                  <span className={`${prefix}-gutter`} aria-hidden>
                    {i + 1}
                  </span>
                  <code>
                    {line.kind === 'cmd' && (
                      <span className={`${prefix}-prompt`} aria-hidden>
                        ${' '}
                      </span>
                    )}
                    {line.text}
                  </code>
                </li>
              ))}
            </ol>

            <aside className={`${prefix}-facts`}>
              <p className={`${prefix}-facts-blurb`}>{active.blurb}</p>
              <dl>
                {active.facts.map((f) => (
                  <div key={f.k}>
                    <dt>{f.k}</dt>
                    <dd>{f.v}</dd>
                  </div>
                ))}
              </dl>
            </aside>
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
