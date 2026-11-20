// Optional developer probe; not part of CI. Muted: plays no sound.
// Usage: NODE_PATH=<playwright>/node_modules node scripts/probe_webkit_playback_rate.mjs <dir of sNN.mp3 (+ sNN.txt)>
// Silent (muted) real-WebKit playback-rate probe for sentence MP3s.
import { createRequire } from 'node:module';
const { webkit } = createRequire(import.meta.url)('playwright');
import { readFileSync, readdirSync } from 'node:fs';
const dir = process.argv[2];
const clips = readdirSync(dir).filter(f => f.endsWith('.mp3')).sort().map(f => ({ name: f, b64: readFileSync(`${dir}/${f}`).toString('base64'), text: readFileSync(`${dir}/${f.replace('.mp3', '.txt')}`, 'utf8').trim() }));
const browser = await webkit.launch();
const page = await browser.newPage();
await page.goto('about:blank');
const out = await page.evaluate(async (clips) => {
  const urls = clips.map(c => URL.createObjectURL(new Blob([Uint8Array.from(atob(c.b64), ch => ch.charCodeAt(0))], { type: 'audio/mpeg' })));
  const ctx = new (window.AudioContext || window.webkitAudioContext)();
  const decoded = [];
  for (const c of clips) { try { const buf = await ctx.decodeAudioData(Uint8Array.from(atob(c.b64), ch => ch.charCodeAt(0)).buffer); decoded.push({ rate: buf.sampleRate, duration: buf.duration, length: buf.length }); } catch (e) { decoded.push({ error: String(e) }); } }
  const ctxRate = ctx.sampleRate; await ctx.close();
  const audio = new Audio(); audio.muted = true; // silent: no audible output
  const samples = []; let index = -1; const events = [];
  const timer = setInterval(() => samples.push({ t: performance.now(), ct: audio.currentTime, i: index, pr: audio.playbackRate, dpr: audio.defaultPlaybackRate, pp: audio.preservesPitch, paused: audio.paused }), 10);
  for (let i = 0; i < urls.length; i++) {
    index = i; audio.src = urls[i];
    const ended = new Promise(r => audio.onended = r);
    await audio.play(); events.push({ i, play: performance.now() });
    await ended; events.push({ i, ended: performance.now(), duration: audio.duration });
  }
  clearInterval(timer);
  return { ctxRate, decoded, samples, events };
}, clips);
await browser.close();
const rows = clips.map((c, i) => {
  const s = out.samples.filter(x => x.i === i && !x.paused && x.ct > 0);
  const rate = (a, b) => (b.ct - a.ct) / ((b.t - a.t) / 1000);
  let maxWindow = 0, minWindow = 9;
  for (let k = 0; k < s.length; k++) { const j = s.findIndex((x, n) => n > k && x.t - s[k].t >= 200); if (j < 0) break; const r = rate(s[k], s[j]); maxWindow = Math.max(maxWindow, r); minWindow = Math.min(minWindow, r); }
  const play = out.events.find(e => e.i === i && e.play).play, end = out.events.find(e => e.i === i && e.ended);
  const d = out.decoded[i];
  return { clip: c.name, chars: c.text.length, elementDuration: +end.duration.toFixed(3), decodedRate: d.rate, decodedDuration: +(d.duration ?? 0).toFixed(3), wallMs: Math.round(end.ended - play), overallRate: s.length > 1 ? +rate(s[0], s.at(-1)).toFixed(3) : null, window200msRate: [+minWindow.toFixed(3), +maxWindow.toFixed(3)], playbackRate: [...new Set(s.map(x => x.pr))], defaultPlaybackRate: [...new Set(s.map(x => x.dpr))], preservesPitch: [...new Set(s.map(x => x.pp))], charsPerSecond: +(c.text.length / end.duration).toFixed(1) };
});
console.log(JSON.stringify({ audioContextRate: out.ctxRate, rows }, null, 1));
