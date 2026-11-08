import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const handlers=new Map(),sent=[],positions=[];
const control={addEventListener:(name,fn)=>handlers.set(name,fn),setPointerCapture(){},setAttribute(){},removeAttribute(){},dataset:{}};
const main={outerPosition:async()=>({x:100,y:200}),scaleFactor:async()=>2,setPosition:async p=>positions.push(p)};
const context={console,Math,window:{__TAURI__:{window:{Window:{getByLabel:async()=>main}},dpi:{PhysicalPosition:class {constructor(x,y){this.x=x;this.y=y;}}},event:{emitTo:async(label,name,p)=>sent.push(p),listen:async()=>{}}}},document:{querySelector:()=>control,addEventListener(){}}};
vm.runInNewContext(readFileSync(new URL('../assets/orb-input.js',import.meta.url),'utf8'),context);
const event=(x,y,button=0)=>({screenX:x,screenY:y,button,pointerId:1,ctrlKey:false});
const dispatch=(name,e)=>handlers.get(name)(e);
const flush=()=>new Promise(resolve=>setImmediate(resolve));
// A click speaks once. Small pointer jitter remains a click.
dispatch('pointerdown',event(20,20));await dispatch('pointermove',event(22,21));dispatch('pointerup',event(22,21));dispatch('click',event(22,21));await flush();
assert.equal(sent.filter(p=>p.action==='click').length,1);assert.equal(positions.length,0);
// Drag follows physical display coordinates and cannot also activate speech.
dispatch('pointerdown',event(20,20));await dispatch('pointermove',event(50,40));dispatch('pointerup',event(50,40));dispatch('click',event(50,40));await flush();
assert.deepEqual({...positions.at(-1)},{x:160,y:240});assert.equal(sent.filter(p=>p.action==='click').length,1);
// Movement after release never drags, and cancellation clears a pending gesture.
const count=positions.length;await dispatch('pointermove',event(90,90));dispatch('pointerdown',event(20,20));dispatch('pointercancel',event(20,20));await dispatch('pointermove',event(90,90));await flush();assert.equal(positions.length,count);
// Right-click is reserved for Settings.
dispatch('pointerdown',event(20,20,2));dispatch('click',event(20,20,2));dispatch('contextmenu',{preventDefault(){}});await flush();assert.equal(sent.filter(p=>p.action==='click').length,1);assert.equal(sent.filter(p=>p.action==='settings').length,1);
console.log('Orb input: click, jitter, scaled drag, release, cancellation and Settings passed.');
