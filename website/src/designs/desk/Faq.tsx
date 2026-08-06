import ScrollFAQAccordion from '@/components/ui/scroll-faqaccordion';
import { FAQ } from '@/designs/content';

/**
 * The FAQ, wrapping `components/ui/scroll-faqaccordion`.
 *
 * Three things are configured here rather than left at their defaults:
 *
 * - `scrollDriven={false}`. The upstream component pins itself with GSAP
 *   ScrollTrigger over a 300vh runway. Pinning uses `position: fixed`, which
 *   cannot escape `.dk-sheet`'s `overflow: hidden`, so on this page it renders
 *   a clipped, unusable section. Un-pinned it is an ordinary accordion that
 *   opens on click — which is also the only way a keyboard user can reach it.
 * - Its own heading is suppressed; the page already supplies section heads, and
 *   the built-in one hard-links to the component author's GitHub.
 * - The answer bubble ships `!bg-blue-400` with `!important`. That blue belongs
 *   to nothing else on this page, so it is overridden through the
 *   `answerClassName` prop the component already exposes.
 */
export function Faq() {
  return (
    <ScrollFAQAccordion
      data={[...FAQ]}
      scrollDriven={false}
      title={null}
      description={null}
      className="dk-faq"
      questionClassName="dk-faq-q"
      answerClassName="dk-faq-a"
    />
  );
}
