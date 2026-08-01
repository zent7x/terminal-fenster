import { motion } from 'motion/react';
import { Bot, MousePointer2, Terminal } from 'lucide-react';
import { Reveal } from '@/components/motion/reveal';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

const features = [
  {
    icon: Terminal,
    title: 'Real Chromium',
    description:
      'Electron 43 / Chromium 150 offscreen rendering. Same engine as a desktop browser, not a DOM-to-text hack.',
  },
  {
    icon: MousePointer2,
    title: 'Your input, unchanged',
    description:
      'Clicks, hover, scroll, and keyboard events map back to the page. The terminal is restored on every exit path.',
  },
  {
    icon: Bot,
    title: 'MCP automation',
    description:
      'Sixteen browser tools over stdio: navigate, accessibility snapshot, click, type, screenshot, and more.',
  },
] as const;

export function Features() {
  return (
    <section id="how" className="border-t border-border py-24">
      <div className="mx-auto w-full max-w-5xl px-4 sm:px-6">
        <Reveal>
          <h2 className="text-3xl font-semibold tracking-tight sm:text-4xl">How it works</h2>
          <p className="mt-3 max-w-2xl text-muted-foreground">
            One process owns your TTY. Another runs sandboxed Chromium. They talk over a private Unix
            socket — JSON for control, binary for frames.
          </p>
        </Reveal>

        <div className="mt-12 grid gap-6 sm:grid-cols-3">
          {features.map((feature, index) => {
            const Icon = feature.icon;

            return (
              <Reveal key={feature.title} delay={index * 0.08}>
                <motion.div
                  whileHover={{ y: -6, rotateX: 2, rotateY: -2 }}
                  transition={{ type: 'spring', stiffness: 320, damping: 22 }}
                  className="h-full [perspective:900px]"
                >
                  <Card className="h-full border-border/80 bg-card/40 shadow-[0_16px_50px_-30px_rgba(0,0,0,0.9),0_0_0_1px_rgba(255,255,255,0.04)_inset] backdrop-blur-sm">
                    <CardHeader>
                      <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-xl border border-accent/20 bg-accent/10 shadow-[0_8px_24px_-12px_rgba(94,234,212,0.55)]">
                        <Icon className="h-5 w-5 text-accent" aria-hidden />
                      </div>
                      <CardTitle className="text-lg">{feature.title}</CardTitle>
                    </CardHeader>
                    <CardContent>
                      <p className="text-sm leading-relaxed text-muted-foreground">
                        {feature.description}
                      </p>
                    </CardContent>
                  </Card>
                </motion.div>
              </Reveal>
            );
          })}
        </div>
      </div>
    </section>
  );
}
