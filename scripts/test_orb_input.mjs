import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const handlers=new Map(),sent=[],positions=[],attributes=new Map();
let visualState;
const control={addEventListener:(name,fn)=>handlers.set(name,fn),setPointerCapture(){},setAttribute:(key,value)=>attributes.set(key,value),removeAttribute:key=>attributes.delete(key),dataset:{}};
const main={outerPosition:async()=>({x:100,y:200}),scaleFactor:async()=>2,setPosition:async p=>positions.push(p)};
const context={console,Math,window:{__TAURI__:{window:{Window:{getByLabel:async()=>main}},dpi:{PhysicalPosition:class {constructor(x,y){this.x=x;this.y=y;}}},event:{emitTo:async(label,name,p)=>sent.push(p),listen:async(name,fn)=>{visualState=fn;}}}},document:{querySelector:()=>control,addEventListener(){}}};
vm.runInNewContext(readFileSync(new URL('../assets/orb-input.js',import.meta.url),'utf8'),context);
const event=(x,y,button=0)=>({screenX:x,screenY:y,button,pointerId:1,ctrlKey:false});
const dispatch=(name,e)=>handlers.get(name)(e);
const flush=()=>new Promise(resolve=>setImmediate(resolve));
// A click speaks once. Small pointer jitter remains a click.
dispatch('pointerdown',event(20,20));await dispatch('pointermove',event(22,21));dispatch('pointerup',event(22,21));dispatch('click',event(22,21));await flush();
assert.equal(sent.filter(p=>p.action==='click').length,1);assert.equal(positions.length,0);
assert.equal(sent.filter(p=>p.action==='blur').length,1,'Pointer use clears the visual keyboard focus cue before a click or drag.');
// Drag follows physical display coordinates and cannot also activate speech.
dispatch('pointerdown',event(20,20));await dispatch('pointermove',event(50,40));dispatch('pointerup',event(50,40));dispatch('click',event(50,40));await flush();
assert.deepEqual({...positions.at(-1)},{x:160,y:240});assert.equal(sent.filter(p=>p.action==='click').length,1);
// Remote desktops may coalesce the last move. Release must land at its position.
dispatch('pointerdown',event(20,20));await dispatch('pointermove',event(35,30));dispatch('pointerup',event(80,60));dispatch('click',event(80,60));await flush();
assert.deepEqual({...positions.at(-1)},{x:220,y:280});assert.equal(sent.filter(p=>p.action==='click').length,1);
// Movement after release never drags, and cancellation clears a pending gesture.
const count=positions.length;await dispatch('pointermove',event(90,90));dispatch('pointerdown',event(20,20));dispatch('pointercancel',event(20,20));await dispatch('pointermove',event(90,90));await flush();assert.equal(positions.length,count);
// Right-click is reserved for Settings.
dispatch('pointerdown',event(20,20,2));dispatch('click',event(20,20,2));dispatch('contextmenu',{preventDefault(){}});await flush();assert.equal(sent.filter(p=>p.action==='click').length,1);assert.equal(sent.filter(p=>p.action==='settings').length,1);
console.log('Orb input: click, jitter, scaled drag, release, cancellation and Settings passed.');

// Native click surface exposes task changes, including updates while still idle.
for(const status of ['Two tasks need your attention.', 'One task finished successfully.', 'No saved task updates.']){
 visualState({payload:{mode:'idle',status,help:'Hey Ava',pressed:null}});
 assert.equal(attributes.get('aria-label'),'notdot voice companion. '+status);
}
visualState({payload:{mode:'speaking',status:'Speaking. Press Escape to stop.',pressed:'false'}});
assert.equal(attributes.get('aria-label'),'notdot voice companion. Speaking. Press Escape to stop.');
visualState({payload:{mode:'idle',help:'Hey Ava',pressed:null}});
assert.equal(attributes.get('aria-label'),'notdot voice companion. Ready. Hey Ava');
// Exercise the actual WASM bridge producer, not a separately reimplemented payload.
const source=readFileSync(new URL('../crates/notdot-orb/src/lib.rs',import.meta.url),'utf8');
const bridge=source.slice(source.indexOf('export function installOrbControls(){'),source.indexOf('export async function resize')).replace('export function','function');
const observers=[],forwarded=[];
const statusNode={textContent:'One task needs attention.'};
const orb={dataset:{state:'idle'},title:'Hey Ava',getAttribute:()=>null};
const producer={document:{getElementById:id=>id==='orb'?orb:statusNode},window:{__TAURI__:{event:{emitTo:async(label,name,p)=>forwarded.push(p),listen:async()=>{}}}},MutationObserver:class{constructor(fn){this.fn=fn;}observe(node,options){observers.push({node,options,notify:this.fn});}}};
vm.runInNewContext(bridge+';installOrbControls();',producer);
const statusObserver=observers.find(o=>o.node===statusNode);
assert.ok(statusObserver.options.childList&&statusObserver.options.characterData&&statusObserver.options.subtree);
statusObserver.notify();
assert.equal(forwarded.at(-1).status,'One task needs attention.');
statusNode.textContent='All tasks finished.';statusObserver.notify();
assert.equal(forwarded.at(-1).status,'All tasks finished.');
console.log('Orb accessibility: task status producer and native label passed.');
