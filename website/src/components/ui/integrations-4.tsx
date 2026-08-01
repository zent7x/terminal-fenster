import { motion } from 'motion/react';
import { Bot, Github, Monitor, Workflow } from 'lucide-react';
import { Reveal } from '@/components/motion/reveal';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { TERMINAL_APPS } from '@/components/terminal-dock';

type IntegrationItem =
  | { id: string; title: string; description: string; variant: 'terminals' }
  | {
      id: string;
      title: string;
      description: string;
      variant: 'icon';
      icon: typeof Bot;
    };

const integrations: IntegrationItem[] = [
  {
    id: 'terminals',
    title: 'Kitty graphics terminals',
    description:
      'Ghostty on macOS is verified. kitty, WezTerm, iTerm2, and Linux still need community validation.',
    variant: 'terminals',
  },
  {
    id: 'mcp',
    title: 'MCP agents',
    description: 'stdio JSON-RPC for Cursor, Claude Desktop, and other MCP clients.',
    variant: 'icon',
    icon: Bot,
  },
  {
    id: 'automation',
    title: 'Workflow scripts',
    description: 'Headless mode for CI, scraping, and scripted browser tasks in any terminal.',
    variant: 'icon',
    icon: Workflow,
  },
  {
    id: 'github',
    title: 'Open source',
    description: 'MIT-licensed Rust + Electron stack. Build from source or run the install script.',
    variant: 'icon',
    icon: Github,
  },
  {
    id: 'headless',
    title: 'Headless anywhere',
    description: 'Apple Terminal and SSH sessions run the same engine without graphics.',
    variant: 'icon',
    icon: Monitor,
  },
];

export function Integrations() {
  return (
    <section id="integrations" className="border-t border-border py-24">
      <div className="mx-auto w-full max-w-5xl px-4 sm:px-6">
        <Reveal>
          <h2 className="text-3xl font-semibold tracking-tight sm:text-4xl">Integrations</h2>
          <p className="mt-3 max-w-2xl text-muted-foreground">
            Terminal-Fenster meets you where you work — interactive Kitty terminals, headless
            shells, and agent automation over MCP.
          </p>
        </Reveal>

        <div className="mt-12 grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          {integrations.map((item, index) => (
            <Reveal key={item.id} delay={index * 0.06}>
              <motion.div
                whileHover={{ y: -4, scale: 1.01 }}
                transition={{ type: 'spring', stiffness: 400, damping: 24 }}
                className={item.id === 'terminals' ? 'sm:col-span-2 lg:col-span-1' : ''}
              >
                <Card className="h-full border-border/70 bg-gradient-to-b from-card/70 to-card/30 shadow-[0_20px_60px_-35px_rgba(0,0,0,0.9)]">
                  <CardHeader>
                    {item.variant === 'terminals' ? (
                      <div className="mb-3 flex flex-wrap gap-2">
                        {TERMINAL_APPS.map((app) => (
                          <motion.img
                            key={app.id}
                            src={app.icon}
                            alt={app.name}
                            title={app.name}
                            className="h-9 w-9 rounded-lg border border-border/60 bg-background/60 object-contain p-1"
                            whileHover={{ y: -3, scale: 1.08 }}
                          />
                        ))}
                      </div>
                    ) : (
                      <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-xl border border-accent/20 bg-accent/10">
                        <item.icon className="h-5 w-5 text-accent" aria-hidden />
                      </div>
                    )}
                    <CardTitle className="text-lg">{item.title}</CardTitle>
                  </CardHeader>
                  <CardContent>
                    <p className="text-sm leading-relaxed text-muted-foreground">{item.description}</p>
                  </CardContent>
                </Card>
              </motion.div>
            </Reveal>
          ))}
        </div>
      </div>
    </section>
  );
}
