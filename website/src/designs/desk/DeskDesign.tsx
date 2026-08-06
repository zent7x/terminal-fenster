import {
  ALPHA_NOTICE,
  BENCH,
  BENCH_ENV,
  BENCH_NOTE,
  DETECTION_NOTE,
  KEYS,
  MCP_TOOLS,
  NAV,
  PILLARS,
  PRODUCT,
  PROOF,
  REQUIREMENTS,
  RUN_LINES,
  SECURITY,
  STATUS_LABEL,
  TERMINALS,
  TESTS,
  TESTS_NOTE,
} from '@/designs/content';
import { useReveal } from '@/designs/use-reveal';
import { Mark } from '@/designs/Mark';
import { CommandBlock } from '@/designs/CommandBlock';
import { ArchStack } from '@/designs/desk/ArchStack';
import { Gaps } from '@/designs/desk/Gaps';
import { Faq } from '@/designs/desk/Faq';
import { AgentArt, EngineArt, InputArt } from '@/designs/art/pillar-art';
import './desk.css';

const PILLAR_ART = {
  chromium: EngineArt,
  input: InputArt,
  mcp: AgentArt,
} as const;

/** Stagger helper — CSS reads `--d` off the element. */
const at = (seconds: number) => ({ '--d': `${seconds}s` }) as React.CSSProperties;

function SectionHead({
  eyebrow,
  title,
  lede,
}: {
  eyebrow: string;
  title: string;
  lede: string;
}) {
  return (
    <div className="dk-shead" data-reveal-item>
      <div>
        <p className="dk-eyebrow">{eyebrow}</p>
        <h2 className="dk-h2">{title}</h2>
      </div>
      <p className="dk-lede">{lede}</p>
    </div>
  );
}

/** Yes/no cell for the capability matrix. */
function Cap({ on }: { on: boolean }) {
  return (
    <span className={`dk-cap ${on ? 'is-yes' : 'is-no'}`} title={on ? 'yes' : 'no'}>
      {on ? (
        <svg viewBox="0 0 14 14" width="13" height="13" aria-hidden>
          <path
            d="M2.5 7.5 L5.5 10.5 L11.5 3.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.9"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      ) : (
        <svg viewBox="0 0 14 14" width="13" height="13" aria-hidden>
          <path
            d="M3.5 7 H10.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.9"
            strokeLinecap="round"
          />
        </svg>
      )}
      <i className="dk-sr">{on ? 'yes' : 'no'}</i>
    </span>
  );
}

export function DeskDesign() {
  const root = useReveal<HTMLDivElement>();

  return (
    <div className="dk" ref={root}>
      <div className="dk-sheet">
        <div className="dk-inner">
          <header className="dk-header">
            <a className="dk-brand" href="/">
              <Mark size={27} />
              <span>Terminal-Fenster</span>
            </a>

            <nav className="dk-nav" aria-label="Primary">
              {NAV.map((l) => (
                <a key={l.href} href={l.href}>
                  {l.label}
                </a>
              ))}
            </nav>

            <a className="dk-ghost" href={PRODUCT.repo}>
              GitHub
            </a>
          </header>

          {/* ---------------------------------------------------- hero ---- */}
          <section className="dk-hero">
            <div data-reveal-item style={at(0.02)}>
              <span className="dk-badge">
                <span className="chip">ALPHA</span>
                <span className="blabel">Chromium 150 · Kitty graphics · MIT</span>
              </span>
            </div>

            <h1 className="dk-h1" data-reveal-item style={at(0.09)}>
              {PRODUCT.headline.lead} <span className="muted">{PRODUCT.headline.trail}</span>
            </h1>

            <p className="dk-sub" data-reveal-item style={at(0.16)}>
              {PRODUCT.blurb}
            </p>

            <div className="dk-cta" data-reveal-item style={at(0.23)}>
              <a className="dk-solid" href="#install">
                Build the alpha
              </a>
              <a className="dk-ghost" href={PRODUCT.docs}>
                Read the docs
              </a>
            </div>

            <p className="dk-req" data-reveal-item style={at(0.29)}>
              Requires{' '}
              {REQUIREMENTS.map((r, i) => (
                <span key={r}>
                  {i > 0 && ' · '}
                  <b>{r}</b>
                </span>
              ))}
            </p>

            <div className="dk-proof" data-reveal-item style={at(0.36)}>
              {PROOF.map((p) => (
                <div className="dk-proof-cell" key={p.label}>
                  <div className="dk-proof-v">{p.value}</div>
                  <div className="dk-proof-l">{p.label}</div>
                  <div className="dk-proof-n">{p.note}</div>
                </div>
              ))}
            </div>
          </section>

          {/* ---------------------------------------------- how it works -- */}
          <section className="dk-section" id="how">
            <SectionHead
              eyebrow="How it works"
              title="Three processes, one private socket."
              lede="One process owns your TTY. Another runs sandboxed Chromium. Nothing between them ever touches the network."
            />
            <div data-reveal-item>
              <ArchStack />
            </div>

            <div className="dk-pillars dk-pillars--spaced">
              {PILLARS.map((p, i) => {
                const Art = PILLAR_ART[p.id];
                return (
                  <div className="dk-pillar" key={p.id} data-reveal-item style={at(i * 0.07)}>
                    <div className="dk-art">
                      <Art />
                    </div>
                    <div className="dk-pillar-body">
                      <span className="dk-pillar-tag">{p.tag}</span>
                      <h3 className="dk-pillar-t">{p.title}</h3>
                      <p className="dk-pillar-b">{p.body}</p>
                    </div>
                  </div>
                );
              })}
            </div>
          </section>

          {/* ------------------------------------------------- terminals -- */}
          <section className="dk-section" id="terminals">
            <SectionHead
              eyebrow="Terminal support"
              title="One is verified. The rest need you."
              lede="Graphics support is only as good as the terminal underneath, so here is the whole matrix — with no optimistic checkmarks."
            />

            <div className="dk-matrix" data-reveal-item>
              <div className="dk-matrix-head" aria-hidden>
                <span>Terminal</span>
                <span>Graphics</span>
                <span>Kitty keys</span>
                <span>Pixel mouse</span>
                <span>Status</span>
              </div>

              {TERMINALS.map((t) => (
                <div className="dk-matrix-row" key={t.id}>
                  <span className="dk-matrix-name">
                    <img src={t.logo} alt="" aria-hidden loading="lazy" />
                    <span>
                      {t.name}
                      <i>{t.version}</i>
                    </span>
                  </span>
                  <span className="dk-matrix-cell" data-label="Graphics">
                    <code>{t.graphics}</code>
                  </span>
                  <span className="dk-matrix-cell" data-label="Kitty keys">
                    <Cap on={t.keyboard} />
                  </span>
                  <span className="dk-matrix-cell" data-label="Pixel mouse">
                    <Cap on={t.pixelMouse} />
                  </span>
                  <span className="dk-matrix-cell">
                    <span className={`dk-tag dk-tag--${t.status}`}>{STATUS_LABEL[t.status]}</span>
                  </span>
                  <p className="dk-matrix-note">{t.note}</p>
                </div>
              ))}
            </div>

            <p className="dk-foot-note" data-reveal-item>
              {DETECTION_NOTE}
            </p>
          </section>

          {/* ------------------------------------------------ automation -- */}
          <section className="dk-section" id="automation">
            <SectionHead
              eyebrow="Automation"
              title="Sixteen tools over stdio."
              lede="Any MCP client that launches subprocess servers can drive it. Snapshots return an accessibility tree with stable refs, so a model clicks by name rather than by pixel."
            />

            <div className="dk-tools" data-reveal-item>
              {MCP_TOOLS.map((t) => (
                <div className="dk-tool" key={t.name}>
                  <code>{t.name}</code>
                  <span>{t.desc}</span>
                </div>
              ))}
            </div>

            <div className="dk-two dk-two--spaced">
              <div data-reveal-item>
                <h3 className="dk-h3">Wire it up</h3>
                <div className="dk-mini">
                  <pre>
                    <span className="c"># JSON for your client’s mcpServers block</span>
                    {'\n'}
                    <span className="p">$ </span>terminal-fenster mcp-config{'\n'}
                    {'\n'}
                    <span className="c"># or run the server directly</span>
                    {'\n'}
                    <span className="p">$ </span>terminal-fenster mcp{'\n'}
                  </pre>
                </div>
              </div>

              <div data-reveal-item style={at(0.06)}>
                <h3 className="dk-h3">Or drive it yourself</h3>
                <dl className="dk-keys">
                  {KEYS.map((row) => (
                    <div key={row.action}>
                      <dt>
                        {row.k.map((key) => (
                          <kbd key={key}>{key}</kbd>
                        ))}
                      </dt>
                      <dd>{row.action}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            </div>
          </section>

          {/* ----------------------------------------------- performance -- */}
          <section className="dk-section" id="performance">
            <SectionHead
              eyebrow="Measured performance"
              title="Both numbers, not the flattering one."
              lede={BENCH_ENV}
            />

            <div className="dk-bench" data-reveal-item>
              <div className="dk-bench-head" aria-hidden>
                <span>Workload</span>
                <span>Viewport</span>
                <span>Steady FPS</span>
                <span>Wire</span>
                <span>Encode</span>
              </div>
              {BENCH.map((b) => (
                <div className="dk-bench-row" key={b.id} data-good={b.good || undefined}>
                  <span className="dk-bench-w">{b.workload}</span>
                  <span data-label="Viewport">
                    <code>{b.viewport}</code>
                  </span>
                  <span data-label="Steady FPS">
                    <b>{b.fps}</b>
                  </span>
                  <span data-label="Wire">
                    <code>{b.wire}</code>
                  </span>
                  <span data-label="Encode">
                    <code>{b.encode}</code>
                  </span>
                </div>
              ))}
            </div>

            <p className="dk-foot-note" data-reveal-item>
              {BENCH_NOTE}
            </p>
          </section>

          {/* ----------------------------------------------------- trust -- */}
          <section className="dk-section" id="trust">
            <SectionHead
              eyebrow="What is actually tested"
              title="It asserts on pixels, not on hope."
              lede={TESTS_NOTE}
            />

            <div className="dk-tests" data-reveal-item>
              {TESTS.map((t) => (
                <div className="dk-test" key={t.cmd}>
                  <span className="dk-test-n">{t.n}</span>
                  <span className="dk-test-w">{t.what}</span>
                  <code>{t.cmd}</code>
                </div>
              ))}
            </div>

            <div className="dk-sec">
              {SECURITY.map((s, i) => (
                <div className="dk-sec-item" key={s.title} data-reveal-item style={at(i * 0.05)}>
                  <h3>{s.title}</h3>
                  <p>{s.body}</p>
                </div>
              ))}
            </div>
          </section>

          {/* ------------------------------------------------ known gaps -- */}
          <section className="dk-section" id="honest">
            <SectionHead
              eyebrow="Known gaps"
              title="Nine things that are not done."
              lede="Ordered by how much they matter. This is the list the README ships with, not a shorter one written for a landing page."
            />
            <div data-reveal-item>
              <Gaps />
            </div>
          </section>

          {/* ------------------------------------------------------- faq -- */}
          <section className="dk-section" id="faq">
            <SectionHead
              eyebrow="Questions"
              title="The ones people actually ask."
              lede="Short answers. Every one of them is expanded on somewhere above, or in the README."
            />
            <div data-reveal-item>
              <Faq />
            </div>
          </section>

          {/* --------------------------------------------------- install -- */}
          <section className="dk-section" id="install">
            <SectionHead
              eyebrow="Install"
              title="Build it from source."
              lede="No signed binaries yet. Expect a few minutes on first run while Rust and Electron compile."
            />

            <div data-reveal-item>
              <CommandBlock prefix="dk" />
            </div>

            <div className="dk-two dk-two--tight">
              <div className="dk-mini" data-reveal-item>
                <pre>
                  {RUN_LINES.map((line, i) =>
                    line.kind === 'comment' ? (
                      <span className="c" key={i}>
                        {line.text}
                        {'\n'}
                      </span>
                    ) : (
                      <span key={i}>
                        <span className="p">$ </span>
                        {line.text}
                        {'\n'}
                      </span>
                    ),
                  )}
                </pre>
              </div>
              <p className="dk-note" data-reveal-item>
                {ALPHA_NOTICE}
              </p>
            </div>
          </section>

          <footer className="dk-footer">
            <p>Terminal-Fenster — {PRODUCT.license} licensed</p>
            <nav>
              <a href={PRODUCT.docs}>Docs</a>
              <a href={PRODUCT.repo}>GitHub</a>
              <a href={`${PRODUCT.repo}/blob/main/SECURITY.md`}>Security</a>
              <a href={`${PRODUCT.repo}/blob/main/RELEASE.md`}>Release gates</a>
              <a href={`${PRODUCT.repo}/tree/main/docs/adr`}>ADRs</a>
            </nav>
          </footer>
        </div>
      </div>
    </div>
  );
}
