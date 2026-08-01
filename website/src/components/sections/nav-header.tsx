import { motion, useScroll, useTransform } from 'motion/react';
import { Github } from 'lucide-react';

const links = [
  { href: '#demo', label: 'Demo' },
  { href: '#how', label: 'How it works' },
  { href: '#install', label: 'Install' },
  { href: '/docs/', label: 'Docs' },
] as const;

export function NavHeader() {
  const { scrollY } = useScroll();
  const bgOpacity = useTransform(scrollY, [0, 80], [0.72, 0.95]);
  const borderOpacity = useTransform(scrollY, [0, 80], [0.35, 1]);
  const backgroundColor = useTransform(bgOpacity, (v) => `rgba(9, 9, 11, ${v})`);
  const borderColor = useTransform(borderOpacity, (v) => `rgba(39, 39, 42, ${v})`);

  return (
    <motion.header
      style={{ backgroundColor, borderColor }}
      className="sticky top-0 z-30 border-b backdrop-blur-xl"
    >
      <div className="mx-auto flex w-full max-w-6xl items-center justify-between gap-4 px-4 py-3.5 sm:px-6">
        <motion.a
          href="/"
          className="flex items-center"
          whileHover={{ scale: 1.02 }}
          whileTap={{ scale: 0.98 }}
        >
          <img src="/assets/logo.svg" alt="Terminal-Fenster" className="h-7 w-auto sm:h-8" />
        </motion.a>

        <nav className="flex items-center gap-4 text-sm text-muted-foreground" aria-label="Primary">
          {links.map((link) => (
            <motion.a
              key={link.href}
              href={link.href}
              className="hidden transition-colors hover:text-foreground sm:inline"
              whileHover={{ y: -1 }}
            >
              {link.label}
            </motion.a>
          ))}
          <motion.a
            href="https://github.com/zent7x/terminal-fenster"
            className="inline-flex items-center gap-1 transition-colors hover:text-foreground"
            whileHover={{ y: -1 }}
          >
            <Github className="h-4 w-4" />
            <span className="hidden sm:inline">GitHub</span>
          </motion.a>
          <a href="#install">
            <motion.span
              className="inline-flex rounded-lg bg-accent px-3 py-1.5 font-medium text-accent-foreground"
              whileHover={{ scale: 1.03, boxShadow: '0 0 24px rgba(94,234,212,0.25)' }}
              whileTap={{ scale: 0.97 }}
            >
              Install
            </motion.span>
          </a>
        </nav>
      </div>
    </motion.header>
  );
}
