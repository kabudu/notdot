// Run one silent conversation-harness scenario headlessly and print its JSON.
// Optional developer tool; not part of CI and not a repository dependency.
//
//   trunk build --dist .local/continuity-ui
//   python3 scripts/prepare_conversation_harness.py
//   NODE_PATH=<dir with playwright installed> node scripts/run_conversation_harness.mjs \
//     .local/continuity-ui speech-stream [webkit|chromium]
//
// The harness replaces audio, microphone and services before WASM loads; it
// plays no sound and opens no microphone. UA overrides the user agent (for
// example a Windows one to exercise the metered output path).
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, resolve } from 'node:path';
const { webkit, chromium } = createRequire(import.meta.url)('playwright');
const [dist, hash = '', kind = 'webkit'] = process.argv.slice(2);
if (!dist) {
  console.error('Usage: node scripts/run_conversation_harness.mjs <dist> [scenario] [webkit|chromium]');
  process.exit(2);
}
const root = resolve(dist);
const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png' };
const server = createServer(async (request, response) => {
  let path = decodeURIComponent(new URL(request.url, 'http://local').pathname);
  if (path === '/') path = '/index.html';
  const file = join(root, path);
  if (!file.startsWith(root)) {
    response.writeHead(403);
    response.end();
    return;
  }
  try {
    const body = await readFile(file);
    response.writeHead(200, { 'content-type': types[extname(path)] || 'application/octet-stream' });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
}).listen(0, '127.0.0.1');
await new Promise(ready => server.on('listening', ready));
const mac = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)';
const browser = await (kind === 'chromium' ? chromium : webkit).launch();
try {
  const page = await browser.newPage({ userAgent: process.env.UA || (kind === 'webkit' ? mac : undefined) });
  page.on('pageerror', error => console.error('pageerror', error.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/#${hash}`);
  await page.waitForFunction(() => document.getElementById('orb')?.dataset.ready === 'true', null, { timeout: 30000 });
  await page.click('#run-silent-checks');
  const result = await page.waitForFunction(() => {
    const text = document.getElementById('harness-results').textContent;
    try { return JSON.parse(text).status !== 'running' && text; } catch { return false; }
  }, null, { timeout: 280000, polling: 500 });
  const text = await result.jsonValue();
  console.log(text);
  process.exitCode = JSON.parse(text).status === 'passed' ? 0 : 1;
} finally {
  await browser.close();
  server.close();
}
