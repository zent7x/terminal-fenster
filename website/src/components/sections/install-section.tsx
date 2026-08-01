import { useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { Reveal } from '@/components/motion/reveal';
import { Button3D } from '@/components/ui/button-3d';

const INSTALL_CMD =
  'git clone https://github.com/zent7x/terminal-fenster.git && cd terminal-fenster && ./install.sh';

export function InstallSection() {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(INSTALL_CMD);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      window.prompt('Copy the source install command:', INSTALL_CMD);
    }
  }

  return (
    <section id="install" className="relative border-t border-border py-24">
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_at_bottom,rgba(94,234,212,0.06),transparent_55%)]" />

      <div className="relative mx-auto w-full max-w-5xl px-4 sm:px-6">
        <Reveal>
          <h2 className="text-3xl font-semibold tracking-tight sm:text-4xl">Install</h2>
          <p className="mt-3 max-w-2xl text-muted-foreground">
            Source builds require Rust 1.80+ and Node 22.12+. Ghostty on macOS is verified
            end-to-end; kitty, WezTerm, and Linux need community testing.
          </p>
        </Reveal>

        <Reveal delay={0.08} className="mt-8">
          <div className="flex flex-col gap-4 sm:flex-row sm:items-stretch">
            <div className="flex-1 overflow-x-auto rounded-xl border border-border bg-[#0c0c0e]/90 p-4 font-mono text-sm text-foreground shadow-[0_12px_40px_-20px_rgba(0,0,0,0.8)] backdrop-blur-sm">
              {INSTALL_CMD}
            </div>
            <Button3D onClick={handleCopy} className="shrink-0 self-start sm:self-auto">
              {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
              {copied ? 'Copied!' : 'Copy command'}
            </Button3D>
          </div>
        </Reveal>

        <Reveal delay={0.12}>
          <pre className="mt-8 overflow-x-auto rounded-2xl border border-border bg-[#0c0c0e] p-5 font-mono text-sm leading-relaxed text-muted-foreground shadow-inner">
            <span className="text-muted-foreground/60"># interactive (Ghostty / kitty / WezTerm)</span>
            {'\n'}
            <span className="text-foreground">terminal-fenster setup</span>
            {'\n'}
            <span className="text-foreground">terminal-fenster open news.ycombinator.com</span>
            {'\n\n'}
            <span className="text-muted-foreground/60"># headless — any terminal</span>
            {'\n'}
            <span className="text-foreground">terminal-fenster open example.com --headless</span>
          </pre>
        </Reveal>
      </div>
    </section>
  );
}
