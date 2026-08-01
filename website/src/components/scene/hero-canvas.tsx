import { Canvas } from '@react-three/fiber';
import { Suspense, lazy } from 'react';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';

const HeroScene = lazy(() =>
  import('./hero-scene').then((m) => ({ default: m.HeroScene })),
);

function SceneFallback() {
  return (
    <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_30%_20%,rgba(94,234,212,0.12),transparent_50%),radial-gradient(ellipse_at_70%_80%,rgba(139,92,246,0.14),transparent_45%)]" />
  );
}

export function HeroCanvas() {
  const reduced = usePrefersReducedMotion();

  if (reduced) {
    return <SceneFallback />;
  }

  return (
    <div className="absolute inset-0">
      <Canvas
        className="h-full w-full"
        camera={{ position: [0, 0.4, 7.2], fov: 42 }}
        dpr={[1, 1.75]}
        gl={{ antialias: true, alpha: true }}
      >
        <Suspense fallback={null}>
          <HeroScene />
        </Suspense>
      </Canvas>
      <div className="pointer-events-none absolute inset-0 bg-gradient-to-b from-background/20 via-transparent to-background" />
    </div>
  );
}
