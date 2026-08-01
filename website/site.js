(function () {
  const INSTALL_CMD =
    'curl -fsSL https://raw.githubusercontent.com/zent7x/terminal-fenster/main/install.sh | bash';

  document.querySelectorAll('[data-copy]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const text = btn.getAttribute('data-copy') || INSTALL_CMD;
      try {
        await navigator.clipboard.writeText(text);
        const label = btn.textContent;
        btn.textContent = 'Copied';
        setTimeout(() => {
          btn.textContent = label;
        }, 1600);
      } catch {
        window.prompt('Copy install command:', text);
      }
    });
  });

  const demo = document.getElementById('live-demo');
  if (!demo) return;

  const urlField = demo.querySelector('.url-field');
  const caption = demo.querySelector('.viewport-caption');
  const prompt = demo.querySelector('.demo-prompt-text');
  const pixels = [...demo.querySelectorAll('.pixel')];

  const steps = [
    {
      url: 'about:blank',
      caption: 'starting engine…',
      prompt: 'terminal-fenster open ',
      tab: '■ new tab',
    },
    {
      url: 'https://news.ycombinator.com',
      caption: 'kitty graphics · first frame 220ms',
      prompt: 'terminal-fenster open news.ycombinator.com',
      tab: '■ news.ycombinator.com',
    },
    {
      url: 'https://news.ycombinator.com/item?id=1',
      caption: 'click · scroll · type — real input',
      prompt: 'terminal-fenster open news.ycombinator.com',
      tab: '■ news.ycombinator.com',
    },
  ];

  let step = 0;
  setInterval(() => {
    const s = steps[step % steps.length];
    if (urlField) urlField.textContent = s.url;
    if (caption) caption.textContent = s.caption;
    if (prompt) prompt.textContent = s.prompt;
    const tab = demo.querySelector('.tab-active');
    if (tab) tab.textContent = s.tab;
    pixels.forEach((p, i) => {
      p.style.opacity = String(0.35 + ((i + step) % 5) * 0.12);
    });
    step += 1;
  }, 2800);
})();
