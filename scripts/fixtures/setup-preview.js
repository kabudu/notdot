/* Test-only adapters for the real WASM UI. Never requests a microphone, plays
   sound, reads credentials or contacts providers. Served only by the local harness. */
(() => {
  'use strict';
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const copy = x => JSON.parse(JSON.stringify(x));
  const settings = {name:'Ava',model:'gpt-6.1-terra',effort:'medium',hue:192,voice_id:'synthetic',voice_provider:'elevenlabs',speech_language:'en',stt_provider:'voice',voice_name:'Synthetic',memory_enabled:false,session_control:'read_only',task_notifications:false,initiative:{behaviour:'reactive',quiet_start:1320,quiet_end:480,daily_limit:3,gap_minutes:60},onboarding_complete:true,thread_id:'synthetic-thread',startup:'manual',open_desktop:false,automatic_updates:false,followup_enabled:false,followup_seconds:3,wake_enabled:false,wake_name_only:false,microphone_muted:false};
  settings.onboarding_complete=false;
  let setupStage='install',setupPolls=0,setupTrial=false,setupPassed=false;
  const state = {settings,profile_id:'owner',phase:'idle',reply:'',speech:{turn:'',text:'',done:false,failed:false},request_id:null,settings_pending:null,error:null,key_configured:true,key_checking:false,key_authorized:true,stt_ready:true,window_visible:true,version:'test',desktop_launch_supported:false,updates:{phase:'idle',version:null,message:'',received:0,total:null,prompt:false},quota:{},wake:{phase:'paused'},learning:{},initiative:{}};
  if(new URLSearchParams(location.search).has('startup'))settings.hue=285;
  let startupFirst=true;
  const speakerCallbacks = [];
  const people={mode:"open",active_profile:"owner",people:[{id:"owner",role:"owner",name:"Me",needs_name:true,voice_enrolled:false,shared_chats:[]}],removed_chats:{archived:0,pending:0},self_removal:null};
  let enrolIndex=0,enrolProfile=null,enrolToken=null;
  const phrase=()=>({step:enrolIndex===4?"ready":"prompt",index:enrolIndex+1,of:4,phrase:"Synthetic enrollment phrase."});
  const log = [], callbacks = [], timers = new Set(), cancelled = new Set(), settingsProposals = new Map();
  function queueSettings(id, changes, delay=0) {settingsProposals.set(id,{base:copy(settings),changes,delay,offered:false,cancelled:false});state.settings_pending=id;}
  function cancelSettings(id) {for(const [key,p] of settingsProposals){if(!id || key===id){p.cancelled=true;settingsProposals.delete(key);record('settings-cancel',{id:key});}}if(!id || state.settings_pending===id)state.settings_pending=null;}
  let audioDuration = 200;
  let active = 0, lastEnd = -Infinity, turn = 0, capture = 0, transcript = '', delayedStart = false, noticeGeneration = 0;
  function later(ms, fn) { const id = setTimeout(() => {timers.delete(id);fn();},ms);timers.add(id);return id; }
  function record(event, detail={}) {log.push({event,at:performance.now(),...detail});}
  function emit(phase,token) {for (const cb of callbacks) cb({payload:{phase,token}});}
  class Node {connect(){return this;} disconnect(){} getByteTimeDomainData(a){const amplitude=active?(12+10*Math.sin(performance.now()/270)):0;for(let i=0;i<a.length;i++)a[i]=128+Math.sin(i*.35)*amplitude;} }
  const parameter = () => ({value:0,cancelScheduledValues(){},setValueAtTime(){},linearRampToValueAtTime(){}});
  class Context {constructor(){this.state='running';this.destination=new Node();}get currentTime(){return performance.now()/1000;}resume(){return Promise.resolve();}createGain(){return Object.assign(new Node(),{gain:parameter()});}createAnalyser(){return Object.assign(new Node(),{fftSize:256,frequencyBinCount:128});}createMediaElementSource(){return new Node();}createOscillator(){return Object.assign(new Node(),{frequency:parameter(),start(){},stop(){}});} }
  class Audio extends EventTarget {constructor(){super();this.paused=true;this.ended=false;this.error=null;this.currentTime=0;}play(){this.paused=false;this.ended=false;active++;record('play');this.dispatchEvent(new Event('playing'));this.timer=later(audioDuration,()=>{this.paused=true;this.ended=true;active--;lastEnd=performance.now();record('ended');this.dispatchEvent(new Event('ended'));});return Promise.resolve();}pause(){if(!this.paused){clearTimeout(this.timer);timers.delete(this.timer);active--;this.paused=true;record('paused');}}}
  // Replacements are installed before WASM loads; no physical audio API is used.
  window.Audio=Audio;window.AudioContext=Context;window.webkitAudioContext=Context;
  if(navigator.mediaDevices) navigator.mediaDevices.getUserMedia=()=>Promise.reject(Error('Microphone forbidden in silent harness'));
  // Valid silent WAV, decoded through the normal UI path but played only by Audio above.
  const wav=new Uint8Array(46),v=new DataView(wav.buffer),put=(n,s)=>[...s].forEach((c,i)=>wav[n+i]=c.charCodeAt(0));
  put(0,'RIFF');v.setUint32(4,38,true);put(8,'WAVEfmt ');v.setUint32(16,16,true);v.setUint16(20,1,true);v.setUint16(22,1,true);v.setUint32(24,16000,true);v.setUint32(28,32000,true);v.setUint16(32,2,true);v.setUint16(34,16,true);put(36,'data');v.setUint32(40,2,true);
  const audio={data:btoa(String.fromCharCode(...wav)),mime:'audio/wav'};
  const windowApi={startDragging:async()=>{},setSize:async()=>{},outerPosition:async()=>({x:0,y:0}),outerSize:async()=>({width:360,height:360}),setPosition:async()=>{}};
  window.__TAURI__={event:{emitTo:async()=>{},listen:async(name,cb)=>{if(name==='voice-capture')callbacks.push(cb);return()=>{};}},window:{getCurrentWindow:()=>windowApi,currentMonitor:async()=>null},dpi:{LogicalSize:class{},PhysicalSize:class{},PhysicalPosition:class{}},core:{invoke:async(cmd,args={})=>{
    if(cmd==='setup_codex'){
      if(args.action==='install')setupStage='signin';
      if(args.action==='signin'){setupStage='waiting';setupPolls=0;}
      if(args.action==='cancel'){setupStage='signin';setupTrial=false;}
      if(['check','poll'].includes(args.action)&&setupStage==='waiting'&&++setupPolls>2)setupStage='ready';
      if(args.action==='trial'){setupTrial=true;setupPassed=false;return {stage:'trial',message:'Speak a test request.'};}
      if(args.action==='finish'){if(!setupPassed)throw Error('A spoken test is required');settings.onboarding_complete=true;return {stage:'complete'};}
      return {stage:setupStage,trial_passed:setupPassed,message:{install:'Install Codex from the official source.',signin:'Sign in with ChatGPT.',waiting:'Finish signing in in your browser.',ready:'Codex is connected.'}[setupStage]};
    }

    if(cmd==='snapshot')return copy(state);
    if(cmd==='speaker_summary')return copy(people);
    if(cmd==='capture_status')return copy(state.wake);
    if(cmd==='voice_providers')return [{id:'elevenlabs',name:'ElevenLabs',streaming_transcription:true}];
    if(cmd==='model_catalog')return [{model:'gpt-6.1-terra',displayName:'Terra',supportedReasoningEfforts:[{reasoningEffort:'medium',description:'Medium'}]}];
    if(cmd==='voice_catalog')return {voices:[{id:'synthetic',name:'Synthetic',description:'Test fixture'}],next_cursor:null};
    if(cmd==='report_visual'){if(setupTrial&&args.report.mode==='speaking'&&turn>0)setupPassed=true;record('visual',{mode:args.report.mode});return null;}
    if(cmd==='start_capture'){capture++;state.wake={phase:'capturing',capture_token:args.token,manual:true,rms:0};record('capture',{token:args.token});emit('capturing',args.token);return null;}
    if(cmd==='cancel_capture'){state.wake={phase:'paused'};return null;}
    if(cmd==='transcribe_wake'){state.wake={phase:'paused'};return transcript;}
    if(cmd==='send_message'){turn++;state.phase='thinking';state.speech={turn:'turn-'+turn,text:'Five plus seven is twelve.',done:true,failed:false};later(300,()=>{state.phase='idle';});return 'turn-'+turn;}
    if(cmd==='speak')return audio;
    if(cmd==='save_settings'){Object.assign(settings,args.settings);return null;}
    if(cmd==='stop'){state.phase='idle';state.speech.failed=true;return null;}
    return null;
  }}};
  async function until(fn,ms=10000){const begin=performance.now();while(!fn()){if(performance.now()-begin>ms)throw Error('Timed out: '+fn);await sleep(30);}}
  const count=name=>log.filter(x=>x.event===name).length;
  function submit(text){transcript=text;state.wake.phase='ready';state.wake.request_id=++capture;emit('ready',state.wake.capture_token);}
  async function run(){const output=document.getElementById('harness-results'),passed=[];const ok=(label,value)=>{if(!value)throw Error(label);passed.push(label);};try{
        const el=id=>document.getElementById(id);
        await until(()=>el('setup-panel').open&&!el('setup-install').hidden);
        ok('fresh installation shows guided setup',!settings.onboarding_complete);
        el('setup-install').click();await until(()=>!el('setup-signin').hidden);
        el('setup-signin').click();await until(()=>!el('setup-cancel').hidden);
        el('setup-cancel').click();await until(()=>!el('setup-signin').hidden);
        ok('cancelled sign-in remains recoverable',!settings.onboarding_complete);
        el('setup-signin').click();await until(()=>!el('setup-voice').hidden,12000);
        el('setup-voice').click();await until(()=>el('panel').open);
        ok('voice settings preserve incomplete setup',!settings.onboarding_complete);
        el('save-settings').click();await until(()=>el('setup-panel').open);
        ok('saving settings does not skip the spoken test',!settings.onboarding_complete);
        el('setup-try').click();await until(()=>count('capture')>0);
        const emptyToken=state.wake.capture_token;state.wake.phase='paused';emit('empty',emptyToken);
        await until(()=>el('setup-panel').open&&el('setup-status').textContent.includes('didn’t hear'));
        ok('empty capture returns immediately to setup without completing it',!settings.onboarding_complete&&el('setup-finish').hidden);
        el('setup-try').click();await until(()=>count('capture')>1);
        window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
        await until(()=>el('setup-panel').open&&el('setup-status').textContent.includes('cancelled'));
        ok('cancelled trial returns to setup and permits retry',!settings.onboarding_complete&&el('setup-finish').hidden);
        el('setup-try').click();await until(()=>count('capture')>2);
        const clock=Date.now;Date.now=()=>clock()+121000;
        try{await until(()=>el('setup-panel').open&&el('setup-status').textContent.includes('timed out'));}finally{Date.now=clock;}
        ok('expired trial returns to setup without accepting a stale result',!settings.onboarding_complete&&el('setup-finish').hidden);
        el('setup-try').click();await until(()=>count('capture')>3);submit('What is five plus seven?');
        await until(()=>!el('setup-finish').hidden,12000);
        ok('spoken test returns to an explicit audible confirmation',!settings.onboarding_complete);
        el('setup-finish').click();await until(()=>settings.onboarding_complete);
        ok('finishing returns to the orb',!el('setup-panel').open);
        output.textContent=JSON.stringify({status:'passed',passed,adapters:'Simulated Codex/auth/STT/TTS; production WASM; no credentials, microphone or audio'},null,2);return;
    }catch(error){output.textContent=JSON.stringify({status:'failed',error:String(error),passed},null,2);}}
  document.addEventListener('DOMContentLoaded',()=>{const result=document.createElement('pre');result.id='harness-results';result.style='white-space:pre-wrap;color:white;position:relative;z-index:10000';document.body.append(result);setTimeout(run,500);});
})();
