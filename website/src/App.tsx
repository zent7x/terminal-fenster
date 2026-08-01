import { Features } from '@/components/ui/features-1';
import { Integrations } from '@/components/ui/integrations-4';
import { HeroSection } from '@/components/sections/hero-section';
import { InstallSection } from '@/components/sections/install-section';
import { NavHeader } from '@/components/sections/nav-header';
import { Reveal } from '@/components/motion/reveal';

export default function App() {
  return (
    <div className="min-h-screen overflow-x-hidden">
      <NavHeader />

      <main>
        <HeroSection />
        <Features />
        <Integrations />
        <InstallSection />
      </main>

      <footer className="border-t border-border py-10">
        <Reveal className="mx-auto flex w-full max-w-5xl flex-col items-start justify-between gap-4 px-4 text-sm text-muted-foreground sm:flex-row sm:items-center sm:px-6">
          <p>Terminal-Fenster — MIT · experimental source alpha · no signed binaries</p>
          <div className="flex flex-wrap gap-4">
            <a href="/docs/" className="transition-colors hover:text-foreground">
              Docs
            </a>
            <a
              href="https://github.com/zent7x/terminal-fenster"
              className="transition-colors hover:text-foreground"
            >
              GitHub
            </a>
            <a
              href="https://github.com/zent7x/terminal-fenster/blob/main/SECURITY.md"
              className="transition-colors hover:text-foreground"
            >
              Security
            </a>
            <a href="#install" className="transition-colors hover:text-foreground">
              Install
            </a>
          </div>
        </Reveal>
      </footer>
    </div>
  );
}
