// Exercise the exact inline module shipped in the WASM frontend.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source = readFileSync(new URL('../crates/notdot-orb/src/interruption.rs', import.meta.url), 'utf8').split('inline_js = r#"')[1].split('"#)]')[0].replaceAll('export ', '');
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
const flush = async () => { for (let n = 0; n < 15; n++) await Promise.resolve(); };
function fixture() {
  const requests = [], contexts = [], timers = [], nodes = [];
  const box = {
    window: { __TAURI__: { core: { invoke: async () => box.invokeResult || false } } },
    navigator: { mediaDevices: { getSupportedConstraints: () => ({ echoCancellation: true }), getUserMedia: () => { const d = deferred(); requests.push(d); return d.promise; } } },
    setTimeout: fn => { timers.push(fn); return timers.length; },
    AudioWorkletNode: class { constructor() { this.port = {}; nodes.push(this); } disconnect() {} },
    AudioContext: class {
      constructor() { this.state = 'running'; this.closed = deferred(); contexts.push(this); this.audioWorklet = { addModule: async () => {} }; }
      resume() { return Promise.resolve(); }
      close() { this.state = 'closed'; return this.closed.promise; }
      createMediaStreamSource() { return { connect() {} }; }
    },
  };
  vm.createContext(box); vm.runInContext(source, box);
  const stream = () => { const track = { stopped: false, stop() { this.stopped = true; }, getSettings: () => ({ echoCancellation: true }) }; return { track, getTracks: () => [track], getAudioTracks: () => [track] }; };
  return { box, requests, contexts, timers, nodes, stream };
}
{
  const f = fixture();
  const first = f.box.interruptStart('first', () => {}); await flush();
  const s1 = f.stream(); f.requests[0].resolve(s1); await first;
  const older = f.box.interruptStart('older', () => {}); // waits for first close
  const newer = f.box.interruptStart('newer', () => {});
  await flush();
  assert.equal(f.requests.length, 2);
  const s2 = f.stream(); f.requests[1].resolve(s2); await newer;
  f.contexts[0].closed.resolve(); await older;
  assert.equal(f.requests.length, 2, 'stale start must not reopen microphone');
  assert.equal(s2.track.stopped, false, 'newer microphone remains alive');
  assert.equal(f.box.interruptActive(), true);
}
{
  const f = fixture();
  const support = f.box.interruptSupport(); await flush();
  f.timers.at(-1)(); await support;
  const late = f.stream(); f.requests[0].resolve(late); await flush();
  assert.equal(late.track.stopped, true, 'timed-out permission request releases late track');
}
{
  const f = fixture(); let detections = 0;
  const started = f.box.interruptStart('speech', () => { detections++; }); await flush();
  const stream = f.stream(); f.requests[0].resolve(stream); await started;
  f.box.invokeResult = true;
  f.nodes[0].port.onmessage({ data: new Float32Array(1600) }); await flush();
  assert.equal(detections, 1);
  assert.equal(stream.track.stopped, false, 'speech onset keeps the same capture for the first word');
  f.nodes[0].port.onmessage({ data: new Float32Array(1600) }); await flush();
  assert.equal(detections, 1, 'no repeated hold from continued speech');
  f.box.invokeResult = 'complete';
  f.nodes[0].port.onmessage({ data: new Float32Array(1600) }); await flush();
  assert.equal(stream.track.stopped, true, 'endpoint releases the microphone before waiting for recognition');
}
{
  const f = fixture(); const support = f.box.interruptSupport(); await flush();
  const stream = f.stream(); stream.track.getSettings = () => { throw Error('settings unavailable'); };
  f.requests[0].resolve(stream); await support;
  assert.equal(stream.track.stopped, true, 'capability errors must release microphone tracks');
}
console.log('Interruption lifecycle: 4 regression scenarios passed');
