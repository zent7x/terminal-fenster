import { useState } from 'react';
import { MacOSDock, type DockApp } from '@/components/ui/mac-os-dock';
import { cn } from '@/lib/utils';

export const TERMINAL_APPS: DockApp[] = [
  { id: 'ghostty', name: 'Ghostty', icon: '/icons/ghostty.svg' },
  { id: 'kitty', name: 'kitty', icon: '/icons/kitty.svg' },
  { id: 'wezterm', name: 'WezTerm', icon: '/icons/wezterm.svg' },
  { id: 'iterm2', name: 'iTerm2', icon: '/icons/iterm2.svg' },
  { id: 'apple-terminal', name: 'Terminal.app', icon: '/icons/apple-terminal.svg' },
];

export const TERMINAL_BLURB: Record<string, string> = {
  ghostty: 'Primary target — verified end-to-end on macOS with Kitty graphics.',
  kitty: 'Kitty graphics expected; community testing is still needed.',
  wezterm: 'Kitty graphics expected; community testing is still needed.',
  iterm2: 'Protocol-verified only; pixel-coordinate mouse input is unavailable.',
  'apple-terminal': 'No graphics support — headless mode only.',
};

interface TerminalDockProps {
  className?: string;
  defaultAppId?: string;
}

export function TerminalDock({ className, defaultAppId = 'ghostty' }: TerminalDockProps) {
  const [activeAppId, setActiveAppId] = useState(defaultAppId);
  const activeApp = TERMINAL_APPS.find((app) => app.id === activeAppId) ?? TERMINAL_APPS[0];

  return (
    <div className={cn('flex flex-col items-center gap-5', className)}>
      <MacOSDock
        apps={TERMINAL_APPS}
        activeAppId={activeAppId}
        onAppClick={(app) => setActiveAppId(app.id)}
      />
      <p className="max-w-md text-center text-sm text-muted-foreground transition-opacity">
        <span className="font-medium text-foreground">{activeApp.name}</span>
        {' — '}
        {TERMINAL_BLURB[activeApp.id]}
      </p>
    </div>
  );
}
