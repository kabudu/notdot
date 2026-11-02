/* Test-only adapters for the real WASM UI. Never requests a microphone, plays
   sound, reads credentials or contacts providers. Served only by the local harness. */
(() => {
  'use strict';
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const copy = x => JSON.parse(JSON.stringify(x));
  const settings = {name:'Ava',model:'gpt-6.1-terra',effort:'medium',hue:192,voice_id:'synthetic',voice_provider:'elevenlabs',speech_language:'en',stt_provider:'voice',voice_name:'Synthetic',memory_enabled:false,initiative:{behaviour:'reactive',quiet_start:1320,quiet_end:480,daily_limit:3,gap_minutes:60},onboarding_complete:true,thread_id:'synthetic-thread',startup:'manual',open_desktop:false,automatic_updates:false,followup_enabled:false,followup_seconds:3,wake_enabled:false,wake_name_only:false,microphone_muted:false};
  const state = {settings,phase:'idle',reply:'',speech:{turn:'',text:'',done:false,failed:false},request_id:null,error:null,key_configured:true,key_checking:false,key_authorized:true,stt_ready:true,window_visible:true,version:'test',desktop_launch_supported:false,updates:{phase:'idle',version:null,message:'',received:0,total:null,prompt:false},quota:{},wake:{phase:'paused'},learning:{},initiative:{}};
  const log = [], callbacks = [], timers = new Set(), cancelled = new Set();
  let active = 0, lastEnd = -Infinity, turn = 0, capture = 0, transcript = '', delayedStart = false;
  function later(ms, fn) { const id = setTimeout(() => {timers.delete(id);fn();},ms);timers.add(id);return id; }
  function record(event, detail={}) {log.push({event,at:performance.now(),...detail});}
  function emit(phase,token) {for (const cb of callbacks) cb({payload:{phase,token}});}
  class Node {connect(){return this;} disconnect(){} getByteTimeDomainData(a){a.fill(128);} }
  const parameter = () => ({value:0,cancelScheduledValues(){},setValueAtTime(){},linearRampToValueAtTime(){}});
  class Context {constructor(){this.state='running';this.destination=new Node();}get currentTime(){return performance.now()/1000;}resume(){return Promise.resolve();}createGain(){return Object.assign(new Node(),{gain:parameter()});}createAnalyser(){return Object.assign(new Node(),{fftSize:256,frequencyBinCount:128});}createMediaElementSource(){return new Node();}createOscillator(){return Object.assign(new Node(),{frequency:parameter(),start(){},stop(){}});} }
  class Audio extends EventTarget {constructor(){super();this.paused=true;this.ended=false;this.error=null;this.currentTime=0;}play(){this.paused=false;this.ended=false;active++;record('play');this.dispatchEvent(new Event('playing'));this.timer=later(200,()=>{this.paused=true;this.ended=true;active--;lastEnd=performance.now();record('ended');this.dispatchEvent(new Event('ended'));});return Promise.resolve();}pause(){if(!this.paused){clearTimeout(this.timer);timers.delete(this.timer);active--;this.paused=true;record('paused');}}}
  // Replacements are installed before WASM loads; no physical audio API is used.
  window.Audio=Audio;window.AudioContext=Context;window.webkitAudioContext=Context;
  if(navigator.mediaDevices) navigator.mediaDevices.getUserMedia=()=>Promise.reject(Error('Microphone forbidden in silent harness'));
  // Valid silent WAV, decoded through the normal UI path but played only by Audio above.
  const wav=new Uint8Array(46),v=new DataView(wav.buffer),put=(n,s)=>[...s].forEach((c,i)=>wav[n+i]=c.charCodeAt(0));
  put(0,'RIFF');v.setUint32(4,38,true);put(8,'WAVEfmt ');v.setUint32(16,16,true);v.setUint16(20,1,true);v.setUint16(22,1,true);v.setUint32(24,16000,true);v.setUint32(28,32000,true);v.setUint16(32,2,true);v.setUint16(34,16,true);put(36,'data');v.setUint32(40,2,true);
  const audio={data:btoa(String.fromCharCode(...wav)),mime:'audio/wav'};
  const windowApi={startDragging:async()=>{},setSize:async()=>{},outerPosition:async()=>({x:0,y:0}),outerSize:async()=>({width:360,height:360}),setPosition:async()=>{}};
  window.__TAURI__={event:{listen:async(name,cb)=>{if(name==='voice-capture')callbacks.push(cb);return()=>{};}},window:{getCurrentWindow:()=>windowApi,currentMonitor:async()=>null},dpi:{LogicalSize:class{},PhysicalSize:class{},PhysicalPosition:class{}},core:{invoke:async(cmd,args={})=>{
    if(cmd==='snapshot') return copy(state);
    if(cmd==='capture_status') return copy(state.wake);
    if(cmd==='voice_providers') return [{id:'elevenlabs',streaming_transcription:true}];
    if(cmd==='report_visual'){record('visual',{mode:args.report.mode});return null;}
    if(cmd==='wake_activity'){record('lease',{ready:args.ready});return null;}
    if(cmd==='model_catalog')return [{model:'gpt-6.1-terra',displayName:'Terra',supportedReasoningEfforts:[{reasoningEffort:'medium',description:'Medium'}]}];
    if(cmd==='voices')return {voices:[],next_cursor:null};
    if(cmd==='start_capture'){
      if(active || state.phase!=='idle') throw Error('Capture overlapped playback/model');
      if(args.followup && performance.now()-lastEnd<590) throw Error('Echo guard violated');
      record('pending-capture',{followup:!!args.followup,token:args.token});
      if(delayedStart)await sleep(500);
      if(cancelled.has(args.token))throw Error('Capture cancelled before activation');
      capture++;state.wake={phase:'capturing',capture_token:args.token,request_id:null,manual:true,rms:0};
      record('capture',{followup:!!args.followup,token:args.token,gap:performance.now()-lastEnd});emit('capturing',args.token);
      if(args.followup)later(settings.followup_seconds*1000,()=>{if(state.wake.capture_token===args.token && state.wake.phase==='capturing'){state.wake.phase='paused';record('timeout',{token:args.token});emit('empty',args.token);}});
      return null;
    }
    if(cmd==='cancel_capture'){cancelled.add(args.token);record('cancel',{token:args.token});if(state.wake.capture_token===args.token)state.wake={phase:'paused'};return null;}
    if(cmd==='transcribe_wake'){record('transcribe');state.wake={phase:'paused'};return transcript;}
    if(cmd==='send_message'){
      turn++;const id='turn-'+turn;record('message',{text:args.text});state.phase='thinking';state.speech={turn:id,text:'The first sentence.',done:false,failed:false};
      later(1200,()=>{if(state.speech.turn===id){state.speech={turn:id,text:'The first sentence. The second sentence.',done:true,failed:false};state.phase='idle';record('model-done');}});
      return id;
    }
    if(cmd==='speak'){record('synthesize');return audio;}
    if(cmd==='save_settings'){Object.assign(settings,args.settings);record('saved');return null;}
    if(cmd==='set_microphone_muted'){settings.microphone_muted=args.muted;return null;}
    if(cmd==='stop'){state.phase='idle';state.speech.failed=true;record('stop');return null;}
    return null;
  }}};
  async function until(fn,ms=7000){const begin=performance.now();while(!fn()){if(performance.now()-begin>ms)throw Error('Timed out: '+fn);await sleep(30);}}
  const count = name => log.filter(x=>x.event===name).length;
  function pressEscape(){window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));}
  function orbClick(){const orb=document.getElementById('orb');for(const type of ['mousedown','click'])orb.dispatchEvent(new MouseEvent(type,{bubbles:true,button:0,clientX:180,clientY:180}));}
  function submit(text){transcript=text;state.wake.phase='ready';state.wake.request_id=++capture;emit('ready',state.wake.capture_token);}
  async function initial(){const n=count('capture');orbClick();await until(()=>count('capture')===n+1);submit('Help me plan tomorrow.');}
  async function nextWindow(){const n=count('capture');await until(()=>count('capture')===n+1);await until(()=>document.getElementById('orb').dataset.state==='followup');}
  async function clean(){pressEscape();for(const id of timers)clearTimeout(id);timers.clear();active=0;Object.assign(settings,{followup_enabled:true,followup_seconds:3,wake_enabled:false,microphone_muted:false});state.window_visible=true;state.phase='idle';state.wake={phase:'paused'};state.speech={turn:'',text:'',done:false,failed:false};document.getElementById('panel').close();await sleep(650);}
  async function run(){
    const output=document.getElementById('harness-results'),passed=[];
    function ok(name,condition){if(!condition)throw Error(name);passed.push(name);output.textContent=JSON.stringify({status:'running',passed},null,2);}
    try {
      await until(()=>document.getElementById('orb').dataset.ready==='true');
      await clean();settings.followup_enabled=false;await sleep(600);let n=count('capture');await initial();await until(()=>count('ended')>=2);await sleep(1500);ok('default/off: no automatic capture',count('capture')===n+1);
      await clean();let m=count('message');await initial();await nextWindow();ok('distinct orb follow-up feedback',document.getElementById('orb-status').textContent.includes('follow-up'));
      submit('Half eight.');await until(()=>count('message')===m+2);await nextWindow();submit('That’s all.');await sleep(1400);ok('two-turn continuity; stop is not sent to model',count('message')===m+2);let captures=count('capture');await sleep(800);ok('goodbye does not reopen',count('capture')===captures);
      await clean();m=count('message');await initial();await nextWindow();await until(()=>count('cancel')>0 && state.wake.phase==='paused',5000);captures=count('capture');await sleep(800);ok('silent timeout discards and stays closed',count('message')===m+1 && count('capture')===captures);
      await clean();n=count('capture');let ended=count('ended');await initial();await until(()=>count('ended')>=ended+2);await until(()=>document.getElementById('orb').dataset.state==='idle');pressEscape();await sleep(1400);ok('Escape cancels cooling with no delayed reopening',count('capture')===n+1);
      for(const reason of ['mute','hide','settings']){
        await clean();await initial();await nextWindow();captures=count('capture');let cancels=count('cancel');
        if(reason==='mute')settings.microphone_muted=true;
        if(reason==='hide')state.window_visible=false;
        if(reason==='settings')document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));
        await until(()=>count('cancel')>cancels);await sleep(700);ok(reason+' closes capture',count('capture')===captures);
      }
      await clean();settings.wake_enabled=true;await sleep(600);m=count('message');const token='synthetic-wake';state.wake={phase:'ready',capture_token:token,request_id:++capture,manual:false};transcript='A wake-initiated request.';await until(()=>count('message')===m+1);await nextWindow();submit("That's all");await sleep(1000);ok('wake-initiated turn continues and stops',count('message')===m+1);
      await clean();n=count('capture');let pending=count('pending-capture');await initial();pending=count('pending-capture');delayedStart=true;await until(()=>count('pending-capture')>pending);pressEscape();await sleep(1100);delayedStart=false;ok('Escape cancels an in-flight start without resurrecting capture',count('capture')===n+1);
      await clean();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('model').disabled);const toggle=document.getElementById('followup-enabled');toggle.checked=true;toggle.dispatchEvent(new Event('change'));const range=document.getElementById('followup-seconds');range.value='12';range.dispatchEvent(new Event('input'));const saved=count('saved');document.getElementById('save-settings').click();await until(()=>count('saved')>saved);ok('settings control saves the chosen follow-up duration',settings.followup_enabled && settings.followup_seconds===12);
      await clean();const auto=log.filter(x=>x.event==='capture'&&x.followup);ok('no capture during playback or inter-sentence gaps',auto.length>=5 && auto.every(x=>x.gap>=590));
      output.textContent=JSON.stringify({status:'passed',passed,modelMessages:count('message'),automaticCaptures:auto.length,minimumEchoGapMs:Math.min(...auto.map(x=>x.gap)),adapters:'synthetic IPC, model, STT and audio; no real microphone, speaker, credentials or provider calls'},null,2);
    }catch(error){pressEscape();output.textContent=JSON.stringify({status:'failed',error:String(error),passed,lastEvents:log.filter(x=>x.event!=='lease').slice(-24)},null,2);}
  }
  document.addEventListener('DOMContentLoaded',()=>{const box=document.createElement('aside');box.style='position:fixed;z-index:9999;right:4px;top:4px;background:#132333;color:white;padding:10px;max-width:500px;font:12px monospace;max-height:90vh;overflow:auto';box.innerHTML='<button id="run-silent-checks">Run silent integration checks</button><pre id="harness-results">Ready. All audio and services are synthetic.</pre>';document.body.append(box);document.getElementById('run-silent-checks').onclick=run;});
})();
