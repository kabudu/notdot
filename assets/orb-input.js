'use strict';
const api=window.__TAURI__, control=document.querySelector('#control');
const send=payload=>api.event.emitTo('main','orb-control',payload);
let origin=null,dragged=false;
control.addEventListener('pointerdown',event=>{origin={x:event.screenX,y:event.screenY};dragged=false;});
control.addEventListener('pointermove',event=>{
 if(origin&&event.buttons===1&&!dragged&&Math.hypot(event.screenX-origin.x,event.screenY-origin.y)>5){dragged=true;api.window.Window.getByLabel('main').then(main=>main?.startDragging());}
});
control.addEventListener('click',event=>{if(!dragged&&event.button===0&&!event.ctrlKey)send({action:'click'});origin=null;});
control.addEventListener('contextmenu',event=>{event.preventDefault();send({action:'settings'});});
document.addEventListener('keydown',event=>{
 if(['Enter',' ','Escape','Tab'].includes(event.key)||event.key===','&&(event.metaKey||event.ctrlKey)){
  event.preventDefault();send({action:'key',key:event.key,meta:event.metaKey,ctrl:event.ctrlKey,repeat:event.repeat});
 }
});
control.addEventListener('focus',()=>send({action:'focus'}));
control.addEventListener('blur',()=>send({action:'blur'}));

api.event.listen('orb-visual-state',event=>{
 const p=event.payload;
 if(!p||typeof p.mode!=='string')return;
 const state=p.mode==='followup'?'Ready for your reply':p.mode;
 control.setAttribute('aria-label','notdot voice companion. '+state+'. '+(p.help||''));
 if(p.pressed===null)control.removeAttribute('aria-pressed');
 else control.setAttribute('aria-pressed',p.pressed);
 control.dataset.state=p.mode;
}).then(()=>send({action:'state'}));
