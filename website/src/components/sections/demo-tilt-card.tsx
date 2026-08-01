import { motion, useMotionValue, useSpring, useTransform } from 'motion/react';
import { useRef } from 'react';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';
import { cn } from '@/lib/utils';

export function DemoTiltCard({ className }: { className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const reduced = usePrefersReducedMotion();

  const mx = useMotionValue(0);
  const my = useMotionValue(0);
  const rotateX = useSpring(useTransform(my, [-0.5, 0.5], [10, -10]), {
    stiffness: 180,
    damping: 22,
  });
  const rotateY = useSpring(useTransform(mx, [-0.5, 0.5], [-10, 10]), {
    stiffness: 180,
    damping: 22,
  });
  const glareX = useTransform(mx, [-0.5, 0.5], ['20%', '80%']);
  const glareY = useTransform(my, [-0.5, 0.5], ['20%', '80%']);

  function onMove(e: React.PointerEvent<HTMLDivElement>) {
    if (reduced || !ref.current) return;
    const rect = ref.current.getBoundingClientRect();
    mx.set((e.clientX - rect.left) / rect.width - 0.5);
    my.set((e.clientY - rect.top) / rect.height - 0.5);
  }

  function onLeave() {
    mx.set(0);
    my.set(0);
  }

  const glareBg = useTransform(
    [glareX, glareY],
    ([x, y]) =>
      `radial-gradient(600px circle at ${x} ${y}, rgba(94,234,212,0.14), transparent 45%)`,
  );

  return (
    <motion.figure
      id="demo"
      ref={ref}
      className={cn('group relative w-full [perspective:1200px]', className)}
      onPointerMove={onMove}
      onPointerLeave={onLeave}
      initial={{ opacity: 0, y: 40, rotateX: 8 }}
      animate={{ opacity: 1, y: 0, rotateX: 0 }}
      transition={{ duration: 0.9, ease: [0.22, 1, 0.36, 1] }}
    >
      <motion.div
        style={reduced ? undefined : { rotateX, rotateY, transformStyle: 'preserve-3d' }}
        className="relative overflow-hidden rounded-2xl border border-white/10 bg-[#0c0c0e] shadow-[0_24px_80px_-20px_rgba(0,0,0,0.85),0_0_0_1px_rgba(255,255,255,0.04)_inset]"
      >
        <div className="flex items-center gap-2 border-b border-white/10 bg-zinc-900/90 px-4 py-2.5">
          <span className="h-3 w-3 rounded-full bg-red-500/90 shadow-[0_0_12px_rgba(239,68,68,0.45)]" />
          <span className="h-3 w-3 rounded-full bg-yellow-500/90" />
          <span className="h-3 w-3 rounded-full bg-green-500/90" />
          <span className="ml-2 font-mono text-[11px] text-zinc-400">
            terminal-fenster open news.ycombinator.com
          </span>
        </div>

        <picture>
          <source srcSet="/assets/demo.gif" type="image/gif" />
          <img
            src="/assets/demo.png"
            alt="Terminal-Fenster running Hacker News in Ghostty"
            className="aspect-[16/11] w-full object-cover object-top"
            width={1024}
            height={704}
            loading="eager"
          />
        </picture>

        {!reduced && (
          <motion.div
            className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover:opacity-100"
            style={{ background: glareBg }}
          />
        )}

        <figcaption className="flex items-center justify-between gap-4 border-t border-white/10 bg-zinc-950/80 px-4 py-3 font-mono text-[11px] text-zinc-400 backdrop-blur-sm">
          <span>real Ghostty capture · Chromium pixels</span>
          <span className="text-teal-300">click · type · scroll</span>
        </figcaption>
      </motion.div>
    </motion.figure>
  );
}
