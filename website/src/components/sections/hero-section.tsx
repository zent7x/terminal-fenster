import { animate, stagger } from 'animejs';
import { motion, useScroll, useTransform } from 'motion/react';
import { useEffect, useRef } from 'react';
import { TerminalDock } from '@/components/terminal-dock';
import { HeroCanvas } from '@/components/scene/hero-canvas';
import { DemoTiltCard } from '@/components/sections/demo-tilt-card';
import { Button3D } from '@/components/ui/button-3d';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';

const badges = ['Experimental alpha', 'Ghostty + macOS verified', 'Source only · unsigned'];

export function HeroSection() {
  const reduced = usePrefersReducedMotion();
  const badgesRef = useRef<HTMLDivElement>(null);
  const { scrollY } = useScroll();
  const heroY = useTransform(scrollY, [0, 500], [0, reduced ? 0 : 80]);
  const heroOpacity = useTransform(scrollY, [0, 400], [1, reduced ? 1 : 0.3]);

  useEffect(() => {
    if (reduced || !badgesRef.current) return;
    const badges = badgesRef.current.querySelectorAll('[data-badge]');
    animate(badges, {
      translateY: [14, 0],
      opacity: [0, 1],
      delay: stagger(90, { start: 700 }),
      duration: 650,
      ease: 'outExpo',
    });
  }, [reduced]);

  return (
    <section className="relative min-h-[92vh] overflow-hidden border-b border-border">
      <HeroCanvas />

      <motion.div style={{ y: heroY, opacity: heroOpacity }} className="relative z-10">
        <div className="mx-auto grid w-full max-w-6xl gap-12 px-4 py-16 sm:px-6 lg:grid-cols-[1.05fr_0.95fr] lg:items-center lg:py-24">
          <div>
            <motion.p
              className="inline-flex items-center gap-2 rounded-full border border-accent/30 bg-accent/10 px-3 py-1 text-xs font-medium uppercase tracking-[0.14em] text-accent"
              initial={{ opacity: 0, x: -16 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ duration: 0.6, delay: 0.1 }}
            >
              <span className="h-1.5 w-1.5 rounded-full bg-accent shadow-[0_0_10px_rgba(94,234,212,0.8)]" />
              Open source · MIT
            </motion.p>

            <motion.h1
              className="mt-5 max-w-xl text-4xl font-semibold tracking-tight text-balance sm:text-5xl lg:text-6xl"
              initial={{ opacity: 0, y: 24 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.75, delay: 0.15, ease: [0.22, 1, 0.36, 1] }}
            >
              A real browser inside your terminal.
            </motion.h1>

            <motion.p
              className="mt-5 max-w-lg text-lg leading-relaxed text-muted-foreground"
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.75, delay: 0.28, ease: [0.22, 1, 0.36, 1] }}
            >
              Not Lynx. Not a screenshot viewer. Chromium 150 renders offscreen; a Rust core paints
              Kitty graphics and forwards your keyboard and mouse back to the page.
            </motion.p>

            <motion.div
              className="mt-8 flex flex-wrap gap-3"
              initial={{ opacity: 0, y: 16 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.65, delay: 0.4 }}
            >
              <a href="#install">
                <Button3D>Build the alpha</Button3D>
              </a>
              <a href="/docs/">
                <Button3D variant="ghost">Read the docs</Button3D>
              </a>
            </motion.div>

            <div
              ref={badgesRef}
              className="mt-7 flex flex-wrap gap-2 font-mono text-[11px] uppercase tracking-[0.12em] text-muted-foreground"
            >
              {badges.map((badge, i) => (
                <span
                  key={badge}
                  data-badge
                  className={
                    i === 0
                      ? 'rounded-full border border-accent/30 bg-accent/10 px-3 py-1 text-accent opacity-0'
                      : 'rounded-full border border-border bg-secondary/50 px-3 py-1 opacity-0'
                  }
                >
                  {badge}
                </span>
              ))}
            </div>
          </div>

          <div className="flex flex-col items-center gap-8 lg:pt-6">
            <DemoTiltCard />
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.7, delay: 0.55 }}
            >
              <TerminalDock />
            </motion.div>
          </div>
        </div>
      </motion.div>
    </section>
  );
}
