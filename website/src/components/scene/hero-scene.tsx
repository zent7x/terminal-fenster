import { Grid, Stars } from '@react-three/drei';
import { useFrame } from '@react-three/fiber';
import { useMemo, useRef } from 'react';
import * as THREE from 'three';

function ParticleField() {
  const points = useRef<THREE.Points>(null);
  const positions = useMemo(() => {
    const arr = new Float32Array(600 * 3);
    for (let i = 0; i < 600; i++) {
      arr[i * 3] = (Math.random() - 0.5) * 24;
      arr[i * 3 + 1] = (Math.random() - 0.5) * 14;
      arr[i * 3 + 2] = (Math.random() - 0.5) * 16;
    }
    return arr;
  }, []);

  useFrame((state) => {
    if (!points.current) return;
    points.current.rotation.y = state.clock.elapsedTime * 0.02;
  });

  return (
    <points ref={points}>
      <bufferGeometry>
        <bufferAttribute attach="attributes-position" args={[positions, 3]} />
      </bufferGeometry>
      <pointsMaterial size={0.035} color="#5eead4" transparent opacity={0.55} sizeAttenuation />
    </points>
  );
}

export function HeroScene() {
  return (
    <>
      <color attach="background" args={['#09090b']} />
      <fog attach="fog" args={['#09090b', 8, 22]} />
      <ambientLight intensity={0.35} />
      <directionalLight position={[5, 8, 4]} intensity={1.1} color="#a7f3d0" />
      <directionalLight position={[-6, -2, -3]} intensity={0.35} color="#8b5cf6" />

      <Stars radius={60} depth={30} count={1800} factor={2.5} saturation={0} fade speed={0.35} />
      <ParticleField />

      <Grid
        position={[0, -2.2, 0]}
        args={[24, 24]}
        cellSize={0.55}
        cellThickness={0.45}
        sectionSize={3.3}
        sectionThickness={1}
        fadeDistance={22}
        fadeStrength={1.2}
        cellColor="#27272a"
        sectionColor="#3f3f46"
        infiniteGrid
      />
    </>
  );
}
