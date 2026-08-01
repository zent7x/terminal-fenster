import { useCallback, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

export interface DockApp {
  id: string;
  name: string;
  icon: string;
}

export interface MacOSDockProps {
  apps: DockApp[];
  className?: string;
  iconSize?: number;
  magnification?: number;
  activeAppId?: string;
  onAppClick?: (app: DockApp) => void;
}

export function MacOSDock({
  apps,
  className,
  iconSize = 52,
  magnification = 1.65,
  activeAppId,
  onAppClick,
}: MacOSDockProps) {
  const dockRef = useRef<HTMLDivElement>(null);
  const iconRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [mouseX, setMouseX] = useState<number | null>(null);

  const handleMouseMove = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    setMouseX(event.clientX);
  }, []);

  const handleMouseLeave = useCallback(() => {
    setMouseX(null);
  }, []);

  const getScale = useCallback(
    (index: number) => {
      if (mouseX === null) return 1;

      const icon = iconRefs.current[index];
      if (!icon) return 1;

      const rect = icon.getBoundingClientRect();
      const iconCenter = rect.left + rect.width / 2;
      const distance = Math.abs(mouseX - iconCenter);
      const influence = iconSize * 1.75;

      if (distance >= influence) return 1;

      const ratio = 1 - distance / influence;
      return 1 + (magnification - 1) * ratio * ratio;
    },
    [mouseX, iconSize, magnification],
  );

  return (
    <div
      ref={dockRef}
      className={cn(
        'inline-flex items-end gap-1.5 rounded-2xl border border-white/10 bg-white/[0.08] px-3 pb-2 pt-3 shadow-2xl backdrop-blur-2xl',
        className,
      )}
      onMouseMove={handleMouseMove}
      onMouseLeave={handleMouseLeave}
    >
      {apps.map((app, index) => {
        const scale = getScale(index);
        const lift = (scale - 1) * iconSize * 0.35;

        return (
          <button
            key={app.id}
            ref={(element) => {
              iconRefs.current[index] = element;
            }}
            type="button"
            className="group relative flex shrink-0 flex-col items-center outline-none transition-[transform] duration-100 ease-out focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-background"
            style={{
              width: iconSize,
              height: iconSize,
              transform: `scale(${scale}) translateY(-${lift}px)`,
            }}
            onClick={() => onAppClick?.(app)}
            aria-label={app.name}
            aria-pressed={activeAppId === app.id}
            title={app.name}
          >
            <img
              src={app.icon}
              alt=""
              className="h-full w-full rounded-[18%] object-contain shadow-md"
              draggable={false}
            />
            {activeAppId === app.id ? (
              <span
                className="absolute -bottom-2 h-1 w-1 rounded-full bg-foreground/80"
                aria-hidden
              />
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
