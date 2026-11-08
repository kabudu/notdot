'use strict';
const checks=[];const sleep=ms=>new Promise(r=>setTimeout(r,ms)); async function until(test){const end=performance.now()+10000;while(!test()&&performance.now()<end)await sleep(100);return test();}
async function run(){checks.length=0;const f=document.querySelector('iframe');const w=f.contentWindow;const d=w.document;const check=(name,ok,detail='')=>{checks.push({name,ok:!!ok,detail});document.querySelector('pre').textContent=JSON.stringify(checks,null,2)};
try{
 await sleep(1200);const orb=d.querySelector('#orb'),canvas=d.querySelector('#orb-gl'),fixture=w.settingsFixture;
 await until(()=>orb.dataset.renderer==='webgl');check('Real Rust/WASM shader selected',orb.dataset.renderer==='webgl',orb.dataset.renderer);
 check('Shader layer never intercepts DOM pointers',w.getComputedStyle(canvas).pointerEvents==='none');
 check('Accessible interaction remains keyboard focusable',orb.tabIndex===0&&orb.getAttribute('role')==='button');
 const gl=canvas.getContext('webgl');check('Shader context is available',gl&&!gl.isContextLost());
 const getProgram=()=>gl.getParameter(gl.CURRENT_PROGRAM);
 for(const [phase,uniform] of [['thinking','thinking'],['announcing','speaking'],['listening','listening']]){fixture.state.phase=phase;await sleep(1900);const p=getProgram();check(phase+' drives distinct shader state',gl.getUniform(p,gl.getUniformLocation(p,uniform))>.9, {value:gl.getUniform(p,gl.getUniformLocation(p,uniform)),mode:orb.dataset.state});}
 fixture.settings.hue=280;await sleep(900);let p=getProgram();const tint=gl.getUniform(p,gl.getUniformLocation(p,'tint'));check('Saved theme reaches shader',tint[0]>.7&&tint[2]>.95,Array.from(tint));
 fixture.state.phase='error';await until(()=>{const c=gl.getUniform(getProgram(),gl.getUniformLocation(getProgram(),'tint'));return c[0]>.95&&c[1]<.4;});p=getProgram();const red=gl.getUniform(p,gl.getUniformLocation(p,'tint'));check('Error retains warning colour',red[0]>.95&&red[1]<.4,{tint:Array.from(red),mode:orb.dataset.state,visibility:d.visibilityState,phase:fixture.state.phase});
 const ext=gl.getExtension('WEBGL_lose_context');check('Context-loss extension available',!!ext);if(ext){ext.loseContext();await until(()=>orb.dataset.renderer==='canvas');check('Loss selects dark Canvas fallback',orb.dataset.renderer==='canvas',orb.dataset.renderer);ext.restoreContext();await until(()=>orb.dataset.renderer==='webgl'&&!gl.isContextLost());await sleep(200);check('Restoration recompiles and resumes shader',orb.dataset.renderer==='webgl'&&!gl.isContextLost(),orb.dataset.renderer);}
 check('No WebGL error after restoration',gl.getError()===gl.NO_ERROR);
 fixture.state.phase='idle';fixture.settings.hue=192;
}catch(e){check('Harness completed',false,String(e));}
 window.shaderResults={checks,passed:checks.every(c=>c.ok),userAgent:navigator.userAgent};document.title=window.shaderResults.passed?'PASS shader acceptance':'FAIL shader acceptance';
 fetch('/shader-results',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(window.shaderResults)}).catch(()=>{});
}
document.querySelector('button').onclick=run;
