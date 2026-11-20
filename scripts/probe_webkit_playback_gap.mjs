// Optional developer probe; not part of CI. Muted: plays no sound.
// Usage: NODE_PATH=<playwright>/node_modules node scripts/probe_webkit_playback_gap.mjs <dir of sNN.mp3 (+ sNN.txt)> <single|preload>
// Silent (muted) WebKit: audible-clock gap between consecutive clips.
// single: one element, src swapped on ended (production). preload: next clip
// loaded in a second element while the current one plays.
import { createRequire } from 'node:module';
const { webkit } = createRequire(import.meta.url)('playwright');
import { readFileSync, readdirSync } from 'node:fs';
const [dir, mode] = process.argv.slice(2);
const clips = readdirSync(dir).filter(f => f.endsWith('.mp3')).sort().map(f => readFileSync(`${dir}/${f}`).toString('base64'));
const browser = await webkit.launch(); const page = await browser.newPage(); await page.goto('about:blank');
const r = await page.evaluate(async ({ clips, mode }) => {
  const urls = clips.map(b => URL.createObjectURL(new Blob([Uint8Array.from(atob(b), c => c.charCodeAt(0))], { type: 'audio/mpeg' })));
  const make = () => { const a = new Audio(); a.muted = true; a.preload = 'auto'; return a; };
  const els = mode === 'preload' ? [make(), make()] : [make()];
  const log = []; // per clip: first time currentTime advances, and ended time
  const watch = (a, i) => new Promise(done => { let first = null; const iv = setInterval(() => { if (first === null && a.currentTime > 0) { first = performance.now(); } }, 2); a.onended = () => { clearInterval(iv); log.push({ i, first, ended: performance.now() }); done(); }; });
  if (mode === 'preload') { els[0].src = urls[0]; els[0].load(); }
  for (let i = 0; i < urls.length; i++) {
    const a = mode === 'preload' ? els[i % 2] : els[0];
    if (mode !== 'preload') a.src = urls[i];
    const w = watch(a, i);
    await a.play();
    if (mode === 'preload' && i + 1 < urls.length) { const n = els[(i + 1) % 2]; n.src = urls[i + 1]; n.load(); }
    await w;
  }
  return log;
}, { clips, mode });
await browser.close();
const gaps = r.slice(1).map((x, k) => Math.round(x.first - r[k].ended));
console.log(mode, 'start lag of first clip (ms) n/a; gaps ended->clock moving (ms):', gaps.join(', '));
