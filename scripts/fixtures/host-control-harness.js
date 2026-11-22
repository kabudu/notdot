/* Test-only adapter for the real WASM UI's browser/computer readiness lines
   and confirmation dialog. Loaded after conversation-harness.js; it wraps that
   adapter's invoke with the three host-control commands and mirrors the
   bridge's rules: an answer closes the question once, an unknown or closed id
   is refused, and an unanswered question closes after its timeout.
   window.hostFixture drives it; window.hostLog records every command.
   Prepare with scripts/prepare_host_control_harness.py. */
(() => {
  'use strict';
  const fixture = window.hostFixture = {
    controls: {browser: {state: 'ready', detail: ''}, desktop: {state: 'setup_required', detail: 'Allow notdot under Privacy & Security › Accessibility, then reconnect.'}},
    confirmation: null,
    open: new Set(),
    timers: new Map(),
  };
  const log = window.hostLog = [];
  let next = 1;
  fixture.ask = (message, timeoutMs = 60000) => {
    const id = 'confirm-' + next++;
    fixture.open.add(id);
    fixture.confirmation = {id, message, timeout_ms: timeoutMs};
    fixture.timers.set(id, setTimeout(() => fixture.close(id, 'timeout'), timeoutMs));
    return id;
  };
  // What the bridge does on timeout, turn end or disconnect.
  fixture.close = (id, reason) => {
    if (!fixture.open.delete(id)) return;
    clearTimeout(fixture.timers.get(id));
    log.push({event: 'closed', id, reason});
    if (fixture.confirmation && fixture.confirmation.id === id) fixture.confirmation = null;
  };
  const tauri = window.__TAURI__, original = tauri.core.invoke;
  tauri.core.invoke = async (cmd, args = {}) => {
    if (cmd === 'answer_host_confirmation') {
      log.push({event: 'answer', id: args.id, allow: args.allow});
      if (!fixture.open.has(args.id)) {
        if (fixture.confirmation && fixture.confirmation.id === args.id) fixture.confirmation = null;
        throw 'That question has already closed';
      }
      fixture.open.delete(args.id);
      clearTimeout(fixture.timers.get(args.id));
      if (fixture.confirmation && fixture.confirmation.id === args.id) fixture.confirmation = null;
      return null;
    }
    if (cmd === 'refresh_host_controls') { log.push({event: 'refresh'}); return null; }
    if (cmd === 'set_up_computer_use') { log.push({event: 'setup'}); return null; }
    const result = await original(cmd, args);
    if (cmd === 'snapshot' && result) {
      result.host_controls = JSON.parse(JSON.stringify(fixture.controls));
      result.host_confirmation = fixture.confirmation && {...fixture.confirmation};
    }
    return result;
  };
  // Automatic checks (open index.html#host-checks). Escape and Tab need
  // trusted key events, so they are checked separately with real key presses.
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const $ = id => document.getElementById(id);
  async function runHostChecks() {
    const results = [], failed = [], d = $('host-confirm');
    const check = (name, ok, detail) => { results.push({name, ok, detail}); if (!ok) failed.push(name); };
    const watch = () => { const t = []; const mo = new MutationObserver(() => t.push(d.open)); mo.observe(d, {attributes: true, attributeFilter: ['open']}); return () => { mo.disconnect(); return t; }; };
    const answers = () => log.filter(e => e.event === 'answer');
    await sleep(2500);
    let stop = watch(), id = fixture.ask('Allow notdot to open 127.0.0.1 in its private browser for this conversation?');
    await sleep(1200);
    check('dialog opens as a modal alertdialog', d.open && d.matches(':modal') && d.getAttribute('role') === 'alertdialog');
    check('dialog is named and described', d.getAttribute('aria-labelledby') === 'host-confirm-title' && /host-confirm-message/.test(d.getAttribute('aria-describedby')));
    check('Don\'t allow has focus first', document.activeElement.id === 'host-confirm-deny', document.activeElement.id);
    check('timeout is stated', $('host-confirm-timeout').textContent.includes('60 seconds'));
    $('host-confirm-allow').click(); await sleep(1500);
    check('Allow answers yes once and closes', JSON.stringify(answers().map(a => [a.id, a.allow])) === JSON.stringify([[id, true]]) && !d.open);
    check('dialog opened and closed once', JSON.stringify(stop()) === '[true,false]');
    log.length = 0; id = fixture.ask('Allow notdot to type into “Note” on 127.0.0.1?'); await sleep(1200);
    $('host-confirm-deny').click(); await sleep(1500);
    check('Don\'t allow answers no once', JSON.stringify(answers().map(a => [a.id, a.allow])) === JSON.stringify([[id, false]]) && !d.open);
    // The native command clears the question only after the bridge replies.
    const invoke = tauri.core.invoke;
    tauri.core.invoke = async (cmd, args) => { if (cmd === 'answer_host_confirmation') await sleep(1500); return invoke(cmd, args); };
    log.length = 0; stop = watch(); id = fixture.ask('Slow answer?'); await sleep(1200);
    $('host-confirm-allow').click(); await sleep(3000);
    tauri.core.invoke = invoke;
    check('an answered question is not shown again while the answer is sent', JSON.stringify(stop()) === '[true,false]' && answers().length === 1);
    log.length = 0; id = fixture.ask('Allow notdot to press “Delete everything” on 127.0.0.1?', 3000); await sleep(1200);
    check('a short timeout is stated', $('host-confirm-timeout').textContent.includes('3 seconds'));
    await sleep(3000);
    check('no answer closes as a timeout with no yes', !d.open && answers().length === 0 && log.some(e => e.reason === 'timeout'));
    const first = fixture.ask('Question A?'); await sleep(1000); const second = fixture.ask('Question B?'); await sleep(1000);
    fixture.close(first, 'timeout'); await sleep(1200);
    check('an older question closing leaves the newer one open', d.open && d.dataset.id === second && $('host-confirm-message').textContent === 'Question B?');
    fixture.close(second, 'turn ended'); await sleep(1200);
    check('a turn ending closes the dialog', !d.open);
    log.length = 0; id = fixture.ask('Stale question?'); await sleep(1200);
    fixture.open.delete(id); $('host-confirm-allow').click(); await sleep(1500);
    check('a stale answer is refused and explained', !d.open && ($('error') || {}).textContent === 'That question has already closed');
    // Settings readiness lines.
    log.length = 0;
    window.dispatchEvent(new KeyboardEvent('keydown', {key: ',', code: 'Comma', metaKey: true, bubbles: true})); await sleep(1200);
    [...document.querySelectorAll('[role=tab]')].find(t => t.textContent.trim() === 'Codex').click(); await sleep(500);
    const section = $('web-section'); if (!section.open) section.querySelector('summary').click(); await sleep(500);
    check('opening Settings refreshes readiness', log.some(e => e.event === 'refresh'));
    const lines = () => [$('browser-use-status').textContent, $('computer-use-status').textContent, $('computer-use-setup').hidden];
    check('ready and setup needed are shown separately', lines()[0].startsWith('Ready.') && lines()[1].startsWith('Setup needed:') && !lines()[2], lines());
    fixture.controls = {browser: {state: 'ready', detail: ''}, desktop: {state: 'ready', detail: ''}}; await sleep(1300);
    check('readiness changes while Settings is open', lines()[1].startsWith('Ready.') && lines()[2], lines());
    fixture.controls = {browser: {state: 'unsupported', detail: 'Not on this system.'}, desktop: {state: 'unsupported', detail: 'macOS only.'}}; await sleep(1300);
    check('unsupported is shown', lines()[0].startsWith('Not available') && lines()[1].startsWith('Not available') && lines()[2], lines());
    fixture.controls = {browser: {state: 'ready', detail: ''}, desktop: {state: 'setup_required', detail: 'Allow notdot under Accessibility.'}}; await sleep(1300);
    $('browser-use').click(); $('computer-use').click(); await sleep(200);
    check('a switch turned off is reflected before saving', lines()[0].startsWith('Off.') && lines()[1].startsWith('Off. Setup is also needed'), lines());
    $('browser-use').click(); $('computer-use').click(); await sleep(200);
    $('computer-use-setup').click(); await sleep(300);
    check('Set up Computer Use… asks the native side once', log.filter(e => e.event === 'setup').length === 1);
    check('switches are labelled switches with their status as description', ['browser-use', 'computer-use'].every(id => $(id).getAttribute('role') === 'switch' && $(id).getAttribute('aria-describedby').includes(id + '-status')));
    const report = {status: failed.length ? 'failed' : 'passed', failed, results};
    window.hostChecks = report;
    const out = document.createElement('pre'); out.id = 'host-results'; out.textContent = JSON.stringify(report, null, 2);
    out.style = 'position:fixed;z-index:9999;left:0;bottom:0;max-height:40vh;overflow:auto;background:#111;color:#fff;font:11px monospace;margin:0';
    document.body.append(out);
    return report;
  }
  window.runHostChecks = runHostChecks;
  // The silent-checks overlay is not part of this page's focus order.
  document.addEventListener('DOMContentLoaded', () => setTimeout(() => {
    const overlay = document.getElementById('run-silent-checks');
    if (overlay) overlay.closest('aside').remove();
    if (location.hash === '#host-checks') runHostChecks();
  }));
})();
