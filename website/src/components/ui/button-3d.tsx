import { motion } from 'motion/react';
import type { ReactNode } from 'react';
import { cn } from '@/lib/utils';

type Button3DProps = {
  children: ReactNode;
  className?: string;
  variant?: 'primary' | 'secondary' | 'ghost';
  depth?: 'shallow' | 'normal';
  onClick?: () => void;
  type?: 'button' | 'submit';
};

export function Button3D({
  className,
  variant = 'primary',
  depth = 'normal',
  children,
  onClick,
  type = 'button',
}: Button3DProps) {
  const depthClass = depth === 'shallow' ? 'shadow-[0_4px_0_0]' : 'shadow-[0_6px_0_0]';

  return (
    <motion.button
      type={type}
      onClick={onClick}
      whileHover={{ y: -2 }}
      whileTap={{ y: depth === 'shallow' ? 2 : 4, scale: 0.98 }}
      transition={{ type: 'spring', stiffness: 500, damping: 28 }}
      className={cn(
        'relative inline-flex items-center justify-center gap-2 rounded-xl px-5 py-2.5 text-sm font-semibold transition-colors',
        depthClass,
        variant === 'primary' &&
          'bg-accent text-accent-foreground shadow-teal-800/80 hover:bg-teal-300',
        variant === 'secondary' &&
          'border border-border bg-secondary text-secondary-foreground shadow-zinc-900/80 hover:bg-secondary/80',
        variant === 'ghost' &&
          'border border-border/60 bg-card/40 text-foreground shadow-zinc-950/80 backdrop-blur-sm hover:bg-card/70',
        className,
      )}
    >
      {children}
    </motion.button>
  );
}
