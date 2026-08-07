import { useEffect, useRef } from 'react';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';

/**
 * Scroll reveal that degrades safely.
 *
 * The markup ships visible. On mount we *opt in* to the hidden state by setting
 * `data-reveal="hidden"`, then flip it to `"shown"` as each element crosses the
 * viewport. Without JS — or with reduced motion — nothing is ever hidden, which
 * is the bug the previous site had: its whole body below the fold was blank
 * until an in-view handler fired.
 */
export function useReveal<T extends HTMLElement = HTMLElement>() {
  const root = useRef<T>(null);
  const reduced = usePrefersReducedMotion();

  useEffect(() => {
    const el = root.current;
    if (!el) return;

    const targets = Array.from(el.querySelectorAll<HTMLElement>('[data-reveal-item]'));
    if (targets.length === 0) return;

    if (reduced) {
      targets.forEach((t) => t.setAttribute('data-reveal', 'shown'));
      return;
    }

    targets.forEach((t) => t.setAttribute('data-reveal', 'hidden'));

    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          entry.target.setAttribute('data-reveal', 'shown');
          io.unobserve(entry.target);
        });
      },
      { rootMargin: '0px 0px -12% 0px', threshold: 0.05 },
    );

    targets.forEach((t) => io.observe(t));
    return () => io.disconnect();
  }, [reduced]);

  return root;
}
