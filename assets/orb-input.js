'use strict';
const api=window.__TAURI__, control=document.querySelector('#control');
const send=payload=>api.event.emitTo('main','orb-control',payload);
let origin=null,dragged=false,positioning=false,pendingPosition=null,generation=0;
const mainWindow=api.window.Window.getByLabel('main');
async function moveMain(){
 if(positioning)return;
 positioning=true;
 try{
  const main=await mainWindow;
  while(pendingPosition){const position=pendingPosition;pendingPosition=null;await main?.setPosition(position);}
 }catch(error){console.warn('Unable to move orb',error);pendingPosition=null;}
 finally{positioning=false;}
}
control.addEventListener('pointerdown',event=>{
 if(event.button!==0)return;
 dragged=false;
 const point={x:event.screenX,y:event.screenY,generation:++generation};
 point.base=mainWindow.then(async main=>main?{position:await main.outerPosition(),scale:await main.scaleFactor()}:null).catch(error=>{console.warn('Unable to start orb drag',error);return null;});
 origin=point;control.setPointerCapture(event.pointerId);
});
control.addEventListener('pointermove',async event=>{
 const point=origin;if(!point)return;
 const dx=event.screenX-point.x,dy=event.screenY-point.y;
 if(!dragged&&Math.hypot(dx,dy)<=5)return;
 dragged=true;
 const base=await point.base;if(!base||point.generation!==generation)return;
 pendingPosition=new api.dpi.PhysicalPosition(Math.round(base.position.x+dx*base.scale),Math.round(base.position.y+dy*base.scale));
 void moveMain();
});
control.addEventListener('pointerup',()=>{origin=null;});
control.addEventListener('pointercancel',()=>{origin=null;});
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
 const state={idle:'Ready',listening:'Listening',followup:'Ready for your reply',thinking:'Thinking',speaking:'Speaking',error:'Needs attention'}[p.mode]||'Ready';
 control.setAttribute('aria-label','notdot voice companion. '+state+'. '+(p.help||''));
 if(p.pressed===null)control.removeAttribute('aria-pressed');
 else control.setAttribute('aria-pressed',p.pressed);
 control.dataset.state=p.mode;
}).then(()=>send({action:'state'}));
