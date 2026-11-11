/* Test-only adapters for the real WASM UI. Never requests a microphone, plays
   sound, reads credentials or contacts providers. Served only by the local harness. */
(() => {
  'use strict';
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const copy = x => JSON.parse(JSON.stringify(x));
  const settings = {name:'Ava',model:'gpt-6.1-terra',effort:'medium',hue:192,voice_id:'synthetic',voice_provider:'elevenlabs',speech_language:'en',stt_provider:'voice',voice_name:'Synthetic',memory_enabled:false,session_control:'read_only',task_notifications:false,initiative:{behaviour:'reactive',quiet_start:1320,quiet_end:480,daily_limit:3,gap_minutes:60},onboarding_complete:true,thread_id:'synthetic-thread',startup:'manual',open_desktop:false,automatic_updates:false,followup_enabled:false,followup_seconds:3,wake_enabled:false,wake_name_only:false,microphone_muted:false};
  const state = {settings,profile_id:'owner',phase:'idle',reply:'',speech:{turn:'',text:'',done:false,failed:false},request_id:null,settings_pending:null,error:null,key_configured:true,key_checking:false,key_authorized:true,stt_ready:true,window_visible:true,version:'test',desktop_launch_supported:false,updates:{phase:'idle',version:null,message:'',received:0,total:null,prompt:false},quota:{},wake:{phase:'paused'},learning:{},initiative:{}};
  const speakerCallbacks = [];
  const people={mode:"open",active_profile:"owner",people:[{id:"owner",role:"owner",name:"Me",needs_name:true,voice_enrolled:false,shared_chats:[]}],removed_chats:{archived:0,pending:0},self_removal:null};
  const notebook={version:1,revision:3,notes:[
    {id:'11111111-1111-4111-8111-111111111111',title:'shopping list',kind:'list',body:'',items:[{id:'a',text:'oat milk',done:false},{id:'b',text:'2 litres of orange juice',done:true},{id:'c',text:'A deliberately long synthetic list item that keeps going to check wrapping in narrow layouts without spaces: supercalifragilisticexpialidocious',done:false}],created:0,updated:0},
    {id:'22222222-2222-4222-8222-222222222222',title:'Gate code',kind:'note',body:'The code is 4 7 1 9, ask Priya.\nBring the blue key.',items:[],created:0,updated:0}]};
  const notebooks={owner:notebook,member:{version:1,revision:1,notes:[{id:'33333333-3333-4333-8333-333333333333',title:'Member chores',kind:'list',body:'',items:[{id:'d',text:'dishes',done:false}],created:0,updated:0}]}};
  window.notebooks=notebooks;
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
  class Node {connect(){return this;} disconnect(){} getByteTimeDomainData(a){a.fill(128);} }
  const parameter = () => ({value:0,cancelScheduledValues(){},setValueAtTime(){},linearRampToValueAtTime(){}});
  class Context {constructor(){this.state='running';this.destination=new Node();}get currentTime(){return performance.now()/1000;}resume(){return Promise.resolve();}createGain(){return Object.assign(new Node(),{gain:parameter()});}createAnalyser(){return Object.assign(new Node(),{fftSize:256,frequencyBinCount:128});}createMediaElementSource(){record('output-graph');return new Node();}createOscillator(){return Object.assign(new Node(),{frequency:parameter(),start(){},stop(){}});} }
  class Audio extends EventTarget {removeAttribute(name){if(name==='src'){this.src='';record('audio-source-cleared');}}load(){this.pause();this.ended=false;this.error=null;}constructor(){super();this.paused=true;this.ended=false;this.error=null;this.currentTime=0;}play(){this.paused=false;this.ended=false;active++;record('play');this.dispatchEvent(new Event('playing'));this.timer=later(audioDuration,()=>{this.paused=true;this.ended=true;active--;lastEnd=performance.now();record('ended');this.dispatchEvent(new Event('ended'));});return Promise.resolve();}pause(){if(!this.paused){clearTimeout(this.timer);timers.delete(this.timer);active--;this.paused=true;record('paused');}}}
  // Replacements are installed before WASM loads; no physical audio API is used.
  window.Audio=Audio;window.AudioContext=Context;window.webkitAudioContext=Context;
  if(navigator.mediaDevices) navigator.mediaDevices.getUserMedia=()=>Promise.reject(Error('Microphone forbidden in silent harness'));
  // Interruption scenarios: a fake echo-cancelled track and worklet. Detections
  // are injected after "capture" and are SYNTHETIC, not acoustic evidence.
  const interruptMode=location.hash.startsWith('#interrupt');
  let fakeAec=location.hash!=='#interrupt-unsupported',injectDetection=false,interruptResult=null,interruptPending=null,micTracks=0,sessions=0;
  if(interruptMode&&navigator.mediaDevices){
    navigator.mediaDevices.getSupportedConstraints=()=>({echoCancellation:true});
    navigator.mediaDevices.getUserMedia=async c=>{record('mic-request');if(location.hash==='#interrupt-start-failure'&&count('mic-request')>1)throw Error('Microphone denied');const ec=c&&c.audio&&c.audio.echoCancellation;if(window.gumDelay)await sleep(window.gumDelay);micTracks++;record('mic-open',{ec});const track={stopped:false,getSettings:()=>({echoCancellation:fakeAec&&ec===true}),stop(){if(!this.stopped){this.stopped=true;micTracks--;record('mic-stop');}}};return {getAudioTracks:()=>[track],getTracks:()=>[track]};};
  }
  Context.prototype.audioWorklet={addModule:async()=>{record('worklet');}};
  Context.prototype.createMediaStreamSource=function(){return new Node();};
  Context.prototype.close=async function(){this.state='closed';record('context-closed');};
  window.AudioWorkletNode=class extends Node{constructor(){super();this.port={onmessage:null};this.iv=setInterval(()=>{if(this.port.onmessage)this.port.onmessage({data:new Float32Array(1600)});},100);}disconnect(){clearInterval(this.iv);}};
  // Valid silent WAV, decoded through the normal UI path but played only by Audio above.
  const wav=new Uint8Array(46),v=new DataView(wav.buffer),put=(n,s)=>[...s].forEach((c,i)=>wav[n+i]=c.charCodeAt(0));
  put(0,'RIFF');v.setUint32(4,38,true);put(8,'WAVEfmt ');v.setUint32(16,16,true);v.setUint16(20,1,true);v.setUint16(22,1,true);v.setUint32(24,16000,true);v.setUint32(28,32000,true);v.setUint16(32,2,true);v.setUint16(34,16,true);put(36,'data');v.setUint32(40,2,true);
  const audio={data:btoa(String.fromCharCode(...wav)),mime:'audio/wav'};
  const windowApi={startDragging:async()=>{},setSize:async()=>{},outerPosition:async()=>({x:0,y:0}),outerSize:async()=>({width:360,height:360}),setPosition:async()=>{}};
  window.__TAURI__={event:{emitTo:async()=>{},listen:async(name,cb)=>{if(name==='voice-capture')callbacks.push(cb);if(name==='speaker-enrol-progress')speakerCallbacks.push(cb);return()=>{};}},window:{getCurrentWindow:()=>windowApi,currentMonitor:async()=>null},dpi:{LogicalSize:class{},PhysicalSize:class{},PhysicalPosition:class{}},core:{invoke:async(cmd,args={})=>{
    if(cmd==='speaker_forget_all_voices'){for(const p of people.people)p.voice_enrolled=false;people.mode='open';people.health={state:'idle'};record('speaker-reset');return null;}
    if(cmd==='speaker_recover'){people.people=[people.people[0]];people.people[0].shared_chats=[];people.recovery_required=false;record('speaker-recovered');return [];}
    if(cmd==='speaker_summary')return copy(people);
    if(cmd==='speaker_add_person'){if(!people.people[0].voice_enrolled)throw Error('Teach your voice first');people.mode='trusted_group';people.people.push({id:'member',role:'member',name:args.name,voice_enrolled:false,shared_chats:[]});return 'member';}
    if(cmd==='speaker_rename_person'){people.people.find(p=>p.id===args.profile).name=args.name;return null;}
    if(cmd==='speaker_use_owner'){people.active_profile='owner';state.profile_id='owner';return null;}
    if(cmd==='speaker_set_mode'){people.mode=args.mode;record('speaker-mode');return null;}
    if(cmd==='speaker_enrol_begin'){const p=people.people.find(p=>p.id===args.profile);if(p.needs_name&&!args.name)throw Error('Tell notdot what to call you first.');if(args.name){p.name=args.name;p.needs_name=false;}enrolProfile=args.profile;enrolIndex=0;record('speaker-enrol-begin',{name:args.name||null});return phrase();}
    if(cmd==='speaker_enrol_capture'){enrolToken=args.token;return null;}
    if(cmd==='start_capture' && args.token===enrolToken){record('speaker-record');later(120,()=>{enrolIndex++;for(const cb of speakerCallbacks)cb({payload:{token:args.token,progress:phrase()}});});return null;}
    if(cmd==='speaker_enrol_cancel'){enrolToken=null;enrolProfile=null;return null;}
    if(cmd==='speaker_enrol_save'){if(args.consent!==true)throw Error('Confirm consent first');people.people.find(p=>p.id===enrolProfile).voice_enrolled=true;record('speaker-saved');return null;}
    if(cmd==='speaker_remove_person'||cmd==='speaker_confirm_self_removal'){const p=people.people.find(p=>p.id===args.profile);if(!p||p.role!=='member'||(args.confirmName||'').trim().toLowerCase()!==p.name.toLowerCase())throw Error('Type '+(p?p.name:'the name')+' to confirm');if(cmd==='speaker_confirm_self_removal'&&!(people.self_removal&&people.self_removal.profile===p.id&&people.self_removal.stage==='awaiting_local_confirmation'))throw Error('Not confirmed');people.people=people.people.filter(x=>x!==p);people.self_removal=null;people.removed_chats.pending++;record(cmd,{profile:p.id,confirm:args.confirmName});return {name:p.name,route:cmd==='speaker_remove_person'?'owner':'self_service',data:'deleted',voice:'deleted',chat:'pending'};}
    if(cmd==='speaker_cancel_self_removal'){people.self_removal=null;record('self-removal-cancel');return null;}
    if(cmd==='speaker_available_chats')return {chats:[{id:'chat-a',title:'Garden plans'}],next_cursor:null};
    if(cmd==='speaker_share_chat'){const p=people.people.find(p=>p.id===args.profile);p.shared_chats=p.shared_chats.filter(g=>g.chat_id!==args.chat);p.shared_chats.push({chat_id:args.chat,access:args.access});record('speaker-share',{access:args.access});return null;}
    if(cmd==='speaker_unshare_chat'){const p=people.people.find(p=>p.id===args.profile);p.shared_chats=p.shared_chats.filter(g=>g.chat_id!==args.chat);return null;}
    if(cmd==='notes_list'){const book=copy(notebooks[state.profile_id]||{version:1,revision:0,notes:[]});if(window.notesDelay){const d=window.notesDelay;window.notesDelay=0;await sleep(d);}return book;}
    if(cmd==='notes_delete'){const notebook=notebooks[state.profile_id];notebook.notes=notebook.notes.filter(n=>n.id!==args.id);record('note-deleted',{id:args.id});return null;}
    if(cmd==='notes_export'){record('notes-export',{id:args.id});return args.id?'Exported 1 note.':'Exported '+(notebooks[state.profile_id]||{notes:[]}).notes.length+' notes.';}
    if(cmd==='backup_export'){if((args.password||'').length<12)throw Error('Use a backup password of at least 12 characters.');record('backup-export');return 'Saved an encrypted backup of 3 memories and 2 notes. Keep the password safe: notdot can’t recover it.';}
    if(cmd==='backup_inspect'){record('backup-inspect');if(window.inspectDelay){const d=window.inspectDelay;window.inspectDelay=0;await sleep(d);}if(args.password!=='synthetic password')throw Error('The password is wrong, or the backup was changed after it was made. Nothing was restored.');return {id:'restore-1',created_unix_ms:1800000000000,memories:3,skipped_memories:1,notes:2,renamed_notes:1,preferences:true};}
    if(cmd==='backup_restore'){record('backup-restore',{id:args.id});return 'Restored 3 memories and 2 notes. Colour and announcement limits were restored.';}
    if(cmd==='backup_cancel'){record('backup-cancel');return null;}
    if(cmd==='snapshot') return copy(state);
    if(cmd==='capture_status') return copy(state.wake);
    if(cmd==='voice_providers') return [{id:'elevenlabs',name:'ElevenLabs',streaming_transcription:true}];
    if(cmd==='report_visual'){record('visual',{mode:args.report.mode});return null;}
    if(cmd==='wake_activity'){record('lease',{ready:args.ready});return null;}
    if(cmd==='model_catalog')return [{model:'gpt-6.1-terra',displayName:'Terra',supportedReasoningEfforts:[{reasoningEffort:'medium',description:'Medium'}]}];
    if(cmd==='voice_catalog')return {voices:[{id:'synthetic',name:'Synthetic',description:'Test adapter'}],next_cursor:null};
    if(cmd==='interrupt_support'){record('interrupt-support',{ok:args.echoCancellation,detail:args.detail});return null;}
    if(cmd==='interrupt_begin'){if(args.turn!==state.speech.turn)throw Error('That reply is no longer speaking');sessions++;record('interrupt-begin',{turn:args.turn});return 'session-'+sessions;}
    if(cmd==='interrupt_audio'){record('interrupt-audio',{guarded:args.guarded});if(injectDetection&&args.session==='session-'+sessions){injectDetection=false;record('synthetic-detection');return true;}return false;}
    if(cmd==='interrupt_collect'){record('interrupt-capture',{token:args.token});return await new Promise(resolve=>{interruptPending=resolve;});}
    if(cmd==='interrupt_end'){record('interrupt-end');if(interruptPending){const done=interruptPending;interruptPending=null;done({interrupt:'resume',line:'Capture cancelled.'});}return null;}
    if(cmd==='start_capture'&&args.interrupt){if(args.interrupt!=='session-'+sessions)throw Error('Interruption is no longer available');capture++;state.wake={phase:'capturing',capture_token:args.token,request_id:null,manual:true,rms:0};record('interrupt-capture',{token:args.token});emit('capturing',args.token);return null;}
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
    if(cmd==='transcribe_wake'&&interruptResult){const r=interruptResult;interruptResult=null;if(window.transcribeDelay){const d=window.transcribeDelay;window.transcribeDelay=0;await sleep(d);}record('transcribe');state.wake={phase:'paused'};return r;}
    if(cmd==='transcribe_wake'){record('transcribe');state.wake={phase:'paused'};return transcript;}
    if(cmd==='send_message'){
      turn++;const id='turn-'+turn;record('message',{text:args.text});state.phase='thinking';state.speech={turn:id,text:'The first sentence.',done:false,failed:false};
      later(1200,()=>{if(state.speech.turn===id){state.speech={turn:id,text:'The first sentence. The second sentence.',done:true,failed:false};state.phase='idle';record('model-done');}});
      return id;
    }
    if(cmd==='settings_question'){const p=settingsProposals.get(args.id);if(!p || p.offered || p.cancelled)throw Error('Settings check expired');p.offered=true;state.settings_pending=null;record('settings-question',{id:args.id});return 'Change the requested settings? Say yes, please change those settings.';}
    if(cmd==='answer_settings'){
      const p=settingsProposals.get(args.id);record('settings-answer',{id:args.id,text:args.text});
      if(!p || !p.offered || p.cancelled)throw Error('Settings check expired');
      const approval=args.text.toLowerCase().replace(/[^a-z ]/g,' ').replace(/\s+/g,' ').trim();
      if(!['yes please change those settings','yes change settings'].includes(approval)){settingsProposals.delete(args.id);record('settings-rejected');return 'Cancelled. Your settings are unchanged.';}
      if(p.delay)await sleep(p.delay);
      if(p.cancelled || settingsProposals.get(args.id)!==p){record('settings-aborted');return 'Cancelled. Your settings are unchanged.';}
      settingsProposals.delete(args.id);
      if(JSON.stringify(settings)!==JSON.stringify(p.base)){record('settings-stale');return 'I could not change your settings: settings changed during confirmation.';}
      Object.assign(settings,p.changes);record('settings-applied',{id:args.id});return 'Your settings are saved.';
    }
    if(cmd==='cancel_settings'){cancelSettings(args.id);return null;}
    if(cmd==='session_question'){record('session-question',{id:args.id});state.session_pending=null;return args.id.includes('automatic')?{outcome:'Sent to Synthetic.'}:{question:'Send the staged instruction to Synthetic? Say yes, please send that message.'};}
    if(cmd==='answer_session'){record('session-answer',{text:args.text});return ['yes, please send that message','yes, send it'].includes(args.text)?'Sent to Synthetic.':'Cancelled. Nothing was sent.';}
    if(cmd==='cancel_session'){if(state.session_pending || args.id){record('session-cancel');state.session_pending=null;}return null;}
    if(cmd==='cancel_followup_speech'){noticeGeneration++;record('notice-cancel');return null;}
    if(cmd==='announce_followup'){
      const generation=noticeGeneration;state.initiative={};active++;record('notice-start',{id:args.id});
      await sleep(800);active--;lastEnd=performance.now();
      if(generation!==noticeGeneration){record('notice-interrupted');throw Error('Follow-up cancelled');}
      record('notice-end');return null;
    }
    if(cmd==='speak'){record('synthesize');return audio;}
    if(cmd==='save_settings'){cancelSettings();Object.assign(settings,args.settings);record('saved');return null;}
    if(cmd==='set_microphone_muted'){settings.microphone_muted=args.muted;return null;}
    if(cmd==='stop'&&window.stopKeepsBusy){record('stop');state.phase='thinking';return null;}
    if(cmd==='stop'){state.phase='idle';state.speech.failed=true;record('stop');return null;}
    return null;
  }}};
  async function until(fn,ms=7000){const begin=performance.now();while(!fn()){if(performance.now()-begin>ms)throw Error('Timed out: '+fn);await sleep(30);}}
  const count = name => log.filter(x=>x.event===name).length;
  function pressEscape(){window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));}
  function orbClick(){const orb=document.getElementById('orb');for(const type of ['mousedown','click'])orb.dispatchEvent(new MouseEvent(type,{bubbles:true,button:0,clientX:180,clientY:180}));}
  function submit(text){if(interruptPending){const done=interruptPending;interruptPending=null;const result=interruptResult||{interrupt:'resume',line:'I did not hear speech, so I will carry on.'};interruptResult=null;const delay=window.transcribeDelay||0;window.transcribeDelay=0;later(delay,()=>done(result));return;}transcript=text;state.wake.phase='ready';state.wake.request_id=++capture;emit('ready',state.wake.capture_token);}
  async function initial(){const n=count('capture');orbClick();await until(()=>count('capture')===n+1);submit('Help me plan tomorrow.');}
  async function nextWindow(){const n=count('capture');await until(()=>count('capture')===n+1);await until(()=>document.getElementById('orb').dataset.state==='followup');}
  async function clean(){pressEscape();for(const id of timers)clearTimeout(id);timers.clear();active=0;Object.assign(settings,{followup_enabled:true,followup_seconds:3,wake_enabled:false,microphone_muted:false});state.session_pending=null;cancelSettings();state.settings_pending=null;state.initiative={};state.window_visible=true;state.phase='idle';state.wake={phase:'paused'};state.speech={turn:'',text:'',done:false,failed:false};document.getElementById('panel').close();await sleep(650);}
  async function run(){
    const output=document.getElementById('harness-results'),passed=[];
    function ok(name,condition){if(!condition)throw Error(name);passed.push(name);output.textContent=JSON.stringify({status:'running',passed},null,2);}
    try {
      if(location.hash==='#tasks-only'){
        await clean();settings.followup_enabled=false;
        const orb=document.getElementById('orb');
        for(const cue of ['working','attention','review','finished']) {
          state.initiative={tasks:{cue,description:'Synthetic '+cue+' task status.'}};
          await until(()=>orb.dataset.taskState===cue);
          ok(cue+' task cue appears inside the production orb',orb.dataset.state==='idle');
          ok(cue+' task status has a text equivalent',document.getElementById('orb-status').textContent==='Synthetic '+cue+' task status.');
          if(cue==='working'){state.initiative.tasks.description='2 tasks are working.';await until(()=>document.getElementById('orb-status').textContent==='2 tasks are working.');ok('same-state count changes refresh accessible status text',orb.dataset.taskState==='working');}
        }
        state.initiative={tasks:{cue:'attention',description:'Approval needed.'}};
        state.phase='thinking';await until(()=>orb.dataset.state==='thinking');
        ok('foreground thinking takes priority over a background task cue',orb.dataset.taskState==='none');
        state.phase='idle';await until(()=>orb.dataset.taskState==='attention');
        ok('attention cue returns after foreground work',document.getElementById('orb-status').textContent==='Approval needed.');
        state.initiative={};await until(()=>orb.dataset.taskState==='none');
        ok('acknowledged tasks leave no stale visual cue',orb.dataset.taskState==='none');
        state.initiative={tasks:{cue:'attention',description:'1 task needs attention in Codex. No approval was granted.'}};
        output.textContent=JSON.stringify({status:'passed',passed,adapters:'production WASM renderer; synthetic native task snapshot, no providers or audio'},null,2);return;
      }
      if(location.hash==='#profile-capture'){
        await clean();settings.followup_enabled=false;let captures=count('capture'),cancels=count('cancel'),messages=count('message');orbClick();await until(()=>count('capture')===captures+1);const token=state.wake.capture_token;state.profile_id='member';await sleep(700);ok('identification preserves the same active microphone capture',state.wake.capture_token===token && state.wake.phase==='capturing' && count('cancel')===cancels);submit('Please calculate five plus seven.');await until(()=>count('message')===messages+1);ok('identified first member turn submits exactly once',count('message')===messages+1);pressEscape();output.textContent=JSON.stringify({status:'passed',passed,adapters:'synthetic native-capture IPC/model/audio; no microphone or providers'},null,2);return;
      }
      if(location.hash==='#interrupt-start-failure'){
        await clean();Object.assign(settings,{followup_enabled:false,interrupt_enabled:true});audioDuration=6000;
        await initial();await until(()=>count('mic-request')>=2);await sleep(1800);
        ok('a failed microphone start does not poll or repeatedly request permission',count('mic-request')===2&&micTracks===0);
        ok('microphone failure explains the explicit retry control',document.getElementById('error').textContent.includes('Toggle it off and on'));
        ok('a microphone failure leaves the reply playing',active>0);
        output.textContent=JSON.stringify({status:'passed',passed,adapters:'production WASM; simulated microphone denial; no microphone'},null,2);return;
      }
      if(location.hash==='#interrupt-unsupported'){
        await clean();Object.assign(settings,{followup_enabled:false,interrupt_enabled:true});audioDuration=3000;
        const checks=count('interrupt-support');await initial();settings.wake_enabled=true;
        await until(()=>count('interrupt-support')>checks);await sleep(1200);
        ok('without echo cancellation nothing listens during replies',count('interrupt-begin')===0&&log.filter(e=>e.event==='interrupt-support').pop().ok===false);
        ok('the capability check releases the microphone',micTracks===0);
        output.textContent=JSON.stringify({status:'passed',passed,adapters:'production WASM; fake media track without echo cancellation; no microphone'},null,2);return;
      }
      if(location.hash==='#interrupt-only'){
        const orb=document.getElementById('orb');
        const reply=async()=>{const plays=count('play'),begins=count('interrupt-begin');settings.wake_enabled=false;pressEscape();state.phase='idle';await sleep(700);await initial();await until(()=>count('play')>plays);await until(()=>count('interrupt-begin')>begins);};
        const held=async()=>{const pauses=count('paused');injectDetection=true;await until(()=>count('interrupt-capture')>0&&count('paused')>pauses&&log[log.length-1]);await until(()=>count('synthetic-detection')>0);};
        await clean();
        document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));
        await until(()=>!document.getElementById('model').disabled);
        document.getElementById('wake-enabled').checked=false;
        document.getElementById('interrupt-enabled').checked=true;
        document.getElementById('interrupt-enabled').dispatchEvent(new Event('change'));
        const saves=count('saved');document.getElementById('save-settings').click();
        await until(()=>count('saved')>saves);
        ok('interruption saves independently of wake listening',settings.interrupt_enabled&&!settings.wake_enabled);
        Object.assign(settings,{followup_enabled:false,interrupt_enabled:true});audioDuration=6000;
        await reply();
        ok('listening starts only during a reply, with an echo-cancelled track',log.find(e=>e.event==='mic-open').ec===true&&log.filter(e=>e.event==='interrupt-support').pop().ok===true);
        await until(()=>count('interrupt-audio')>3);ok('echo-cancelled audio goes only to the local detector',count('message')===1);
        await until(()=>(orb.getAttribute('aria-label')||'').includes('interrupt with Stop'),3000).catch(e=>{throw Error('orb label was: '+orb.getAttribute('aria-label'))});ok('the orb explains how to interrupt',true);
        let captures=count('interrupt-capture'),plays=count('play'),messages=count('message');
        await held();
        ok('a detection pauses the reply and captures the request',count('interrupt-capture')===captures+1);
        ok('the same echo-cancelled microphone keeps the complete interruption',micTracks===1);
        await until(()=>(orb.getAttribute('aria-label')||'').includes('Paused my reply'),3000);ok('the orb says the reply is paused',true);
        interruptResult={interrupt:'resume',line:'That sounded like my own voice, so I will carry on.'};submit('ignored');
        await until(()=>count('play')>plays);ok('echo, silence or another voice resumes the same reply',count('message')===messages&&count('stop')===0);
        ok('the resume reason is shown',document.getElementById('error').textContent.includes('carry on'));
        await until(()=>count('interrupt-begin')>=2);
        let stops=count('stop');messages=count('message');captures=count('interrupt-capture');
        await held();interruptResult={interrupt:'request',text:'What about tomorrow?'};submit('ignored');
        await until(()=>count('message')>messages);
        ok('a request cancels the reply through the normal path before it is sent',count('stop')>stops&&log.filter(e=>e.event==='message').pop().text==='What about tomorrow?');
        await until(()=>count('play')>0&&count('interrupt-begin')>=3,9000);
        stops=count('stop');messages=count('message');
        await held();interruptResult={interrupt:'stop'};submit('ignored');
        await until(()=>count('stop')>stops);await sleep(400);
        ok('Stop ends the reply without a new request',count('message')===messages&&micTracks===0);
        await reply();plays=count('play');
        await held();submit('');
        await until(()=>count('play')>plays,6000);ok('a capture that ends silently resumes the reply',count('message')===messages+1);
        await until(()=>count('interrupt-begin')>=5);
        await held();const cancels=count('cancel'),ends=count('interrupt-end');pressEscape();
        await until(()=>count('interrupt-end')>ends);await sleep(300);
        ok('Escape during a held reply cancels it and releases listening',micTracks===0&&count('interrupt-end')>ends);
        await reply();const ended=count('interrupt-end');settings.microphone_muted=true;
        await until(()=>count('interrupt-end')>ended&&micTracks===0);ok('muting stops interruption listening',true);
        settings.microphone_muted=false;
        const sent=text=>log.some(e=>e.event==='message'&&e.text===text);
        // A late recognition result after Escape must not act.
        await reply();await held();window.transcribeDelay=800;interruptResult={interrupt:'request',text:'Late request'};submit('ignored');
        await sleep(150);pressEscape();await sleep(1500);
        ok('a delayed result after Escape is ignored',!sent('Late request'));
        // Escape while the cancelled reply is still finishing revokes the request.
        await reply();await held();window.stopKeepsBusy=true;state.phase='thinking';const stopsBefore=count('stop');interruptResult={interrupt:'request',text:'Waiting request'};submit('ignored');
        await until(()=>count('stop')>stopsBefore);await sleep(400);ok('the request waits while the cancelled reply finishes',!sent('Waiting request'));pressEscape();
        await sleep(400);window.stopKeepsBusy=false;state.phase='idle';await sleep(1500);
        ok('Escape during the cancellation wait means the request is never sent',!sent('Waiting request'));
        // Stopping during a slow microphone start, then starting again, keeps
        // only the newer session's microphone.
        window.gumDelay=700;const begins=count('interrupt-begin');settings.wake_enabled=false;pressEscape();state.phase='idle';await sleep(700);await initial();settings.wake_enabled=true;
        await until(()=>count('interrupt-begin')>begins);const endsBefore=count('interrupt-end');settings.microphone_muted=true;await until(()=>count('interrupt-end')>endsBefore);settings.microphone_muted=false;
        await until(()=>count('interrupt-begin')>begins+1,9000);await sleep(1600);window.gumDelay=0;
        ok('a late microphone from an older start is released and the newer one kept',micTracks===1&&count('mic-stop')>=1);
        output.textContent=JSON.stringify({status:'passed',passed,adapters:'production WASM; fake echo-cancelled track; detections injected after capture (synthetic, not acoustic); no microphone or providers'},null,2);return;
      }
      if(location.hash==='#profile-only'){
        await clean();settings.followup_enabled=false;audioDuration=2000;let played=count('play'),paused=count('paused');await initial();await until(()=>count('play')===played+1);state.profile_id='member';state.speech={turn:'',text:'',done:false,failed:false};state.phase='idle';await until(()=>count('paused')>paused);played=count('play');let captures=count('capture');await sleep(1500);ok('profile switch stops old audio and drops queued old sentences',count('play')===played && count('capture')===captures);output.textContent=JSON.stringify({status:'passed',passed,adapters:'synthetic IPC/model/audio; no microphone or providers'},null,2);return;
      }
      document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));
      await until(()=>document.querySelectorAll('.speaker-person').length===1);
      document.getElementById('settings-category-voice').click();document.getElementById('people-section').open=true;
      ok('adding people waits until the owner has a saved voice',document.getElementById('speaker-add').disabled);
      const ownerCard=document.querySelector('.speaker-person');const ownerTeach=()=>[...document.querySelector('.speaker-person').querySelectorAll('button')].find(b=>b.textContent==='Teach voice');
      ok('owner is asked for a name instead of the placeholder',ownerCard.querySelector('input').value===''&&ownerCard.textContent.includes('What should notdot call you'));
      ownerTeach().click();await sleep(150);ok('teaching the owner voice waits for a real name',count('speaker-enrol-begin')===0&&document.getElementById('speaker-enrol').hidden&&document.getElementById('speaker-status').textContent.includes('call you'));
      ownerCard.querySelector('input').value='Kamba';ownerTeach().click();await until(()=>!document.getElementById('speaker-enrol').hidden);ok('owner name is saved with enrolment',people.people[0].name==='Kamba'&&document.getElementById('speaker-enrol-title').textContent==='Teach notdot your voice');document.getElementById('speaker-consent').checked=true;document.getElementById('speaker-consent').dispatchEvent(new Event('change'));for(let i=0;i<4;i++){document.getElementById('speaker-record').click();await until(()=>enrolIndex===i+1);}await until(()=>!document.getElementById('speaker-enrol-save').hidden);document.getElementById('speaker-enrol-save').click();await until(()=>!document.getElementById('speaker-add').disabled);
      document.getElementById('speaker-new-name').value='Sam';document.getElementById('speaker-add').click();await until(()=>document.querySelectorAll('.speaker-person').length===2);
      const person=()=>[...document.querySelectorAll('.speaker-person')].find(p=>p.querySelector('h3').textContent.startsWith('Sam'));
      const action=(node,text)=>[...node.querySelectorAll('button')].find(b=>b.textContent===text);
      action(person(),'Teach voice').click();await until(()=>!document.getElementById('speaker-enrol').hidden);
      ok('enrolment names the person being taught',document.getElementById('speaker-enrol-title').textContent==="Teach notdot Sam's voice"&&document.getElementById('speaker-consent-text').textContent.startsWith('Sam is here'));
      ok('enrollment requires participant consent before recording',document.getElementById('speaker-record').disabled);
      document.getElementById('speaker-consent').checked=true;document.getElementById('speaker-consent').dispatchEvent(new Event('change'));
      for(let i=0;i<4;i++){document.getElementById('speaker-record').click();await until(()=>enrolIndex===i+1);}
      await until(()=>!document.getElementById('speaker-enrol-save').hidden);ok('all four local samples are required before saving',count('speaker-record')===8 && count('speaker-saved')===1);
      document.getElementById('speaker-enrol-save').click();await until(()=>count('speaker-saved')===2);await until(()=>person().textContent.includes('Voice saved'));
      people.active_profile='member';state.profile_id='member';const mode=document.getElementById('speaker-mode');mode.value='owner_only';document.getElementById('speaker-mode-save').click();ok('listening mode waits for local confirmation',people.mode==='trusted_group');document.getElementById('speaker-confirm-yes').click();await until(()=>people.mode==='owner_only');await until(()=>!document.getElementById('speaker-use-owner').hidden);document.getElementById('speaker-use-owner').click();await until(()=>people.active_profile==='owner');ok('local Use my profile restores owner context',people.active_profile==='owner');
      action(person(),'Load available chats').click();await until(()=>person().querySelector('select').options.length===1 && person().querySelector('option').value==='chat-a');person().querySelector('select').value='chat-a';action(person(),'Share for reading').click();ok('chat sharing waits for local confirmation',people.people[1].shared_chats.length===0);document.getElementById('speaker-confirm-yes').click();await until(()=>people.people[1].shared_chats.length===1);await until(()=>action(person(),'Allow messages'));
      ok('first chat grant is read only',people.people[1].shared_chats[0].access==='read');action(person(),'Allow messages').click();ok('sending needs a separate approval',people.people[1].shared_chats[0].access==='read');document.getElementById('speaker-confirm-yes').click();await until(()=>people.people[1].shared_chats[0].access==='read_write');
      action(person(),'Teach voice').click();await until(()=>!document.getElementById('speaker-enrol').hidden);document.getElementById('speaker-enrol-cancel').click();for(const cb of speakerCallbacks)cb({payload:{token:'retired-token',progress:phrase()}});await sleep(150);ok('cancelled enrollment ignores stale progress',document.getElementById('speaker-enrol').hidden);
      action(person(),'Teach voice').click();await until(()=>!document.getElementById('speaker-enrol').hidden);for(const cb of speakerCallbacks)cb({payload:{token:'retired-token',progress:{step:'ready',of:4}}});await sleep(150);ok('old capture progress cannot complete a new enrollment',document.getElementById('speaker-enrol-save').hidden);document.getElementById('speaker-enrol-cancel').click();
      people.self_removal={profile:'member',stage:'awaiting_code',expires_in_s:90};document.getElementById('panel').close();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('speaker-leave').hidden);
      ok('a pending self-removal waits for the spoken code before any click',document.getElementById('speaker-leave-yes').hidden&&document.getElementById('speaker-leave-text').textContent.includes('Nothing has been deleted'));
      people.self_removal.stage='awaiting_local_confirmation';document.getElementById('panel').close();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('speaker-leave-yes').hidden);
      ok('self-removal needs the member to type their own name',document.getElementById('speaker-leave-yes').disabled);const leave=document.getElementById('speaker-leave-name');leave.value='Kamba';leave.dispatchEvent(new Event('input'));ok('another name does not enable self-removal',document.getElementById('speaker-leave-yes').disabled);
      document.getElementById('speaker-leave-cancel').click();await until(()=>count('self-removal-cancel')===1);ok('keeping the profile cancels without deleting',people.people.length===2&&people.self_removal===null);
      const addPerson=async(name,id)=>{const before=people.people.length;document.getElementById('speaker-new-name').value=name;document.getElementById('speaker-add').click();await until(()=>people.people.length===before+1);people.people[people.people.length-1].id=id;};
      await addPerson('Robin','robin');await addPerson('Jo','jo');const card=name=>[...document.querySelectorAll('.speaker-person')].find(p=>p.querySelector('h3').textContent.startsWith(name));
      people.self_removal={profile:'robin',stage:'awaiting_local_confirmation',expires_in_s:90};document.getElementById('panel').close();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('speaker-leave').hidden&&!document.getElementById('speaker-leave-yes').hidden&&document.getElementById('speaker-leave-title').textContent.startsWith('Robin'));
      leave.value=' robin ';leave.dispatchEvent(new Event('input'));ok('typing their own name enables self-removal',!document.getElementById('speaker-leave-yes').disabled);document.getElementById('speaker-leave-yes').click();await until(()=>count('speaker_confirm_self_removal')===1);await until(()=>document.getElementById('speaker-status').textContent.includes("Robin's profile was removed"));
      ok('self-removal reports what happened, including shared messages kept',document.getElementById('speaker-status').textContent.includes('Messages they added to shared chats stay')&&document.getElementById('speaker-status').textContent.includes('keep trying to archive'));
      ok('archive retry status stays visible',!document.getElementById('speaker-removed-chats').hidden);
      await until(()=>card('Jo'));action(card('Jo'),'Remove Jo').click();await until(()=>!document.getElementById('speaker-confirm').hidden);ok('owner removal names the person and what happens',document.getElementById('speaker-confirm-text').textContent.startsWith('Remove Jo from notdot?')&&document.getElementById('speaker-confirm-text').textContent.includes("can't be undone"));
      ok('owner removal needs the name typed',document.getElementById('speaker-confirm-yes').disabled);const named=document.getElementById('speaker-confirm-name');named.value='Sam';named.dispatchEvent(new Event('input'));document.getElementById('speaker-confirm-yes').click();await sleep(100);ok('a different name cannot confirm removal',count('speaker_remove_person')===0);
      named.value='jo';named.dispatchEvent(new Event('input'));document.getElementById('speaker-confirm-yes').click();await until(()=>count('speaker_remove_person')===1);ok('the typed name is sent for native checking',log.find(e=>e.event==='speaker_remove_person').confirm==='jo');await until(()=>document.getElementById('speaker-status').textContent.includes('Jo was removed'));
      ok('other people are untouched by removals',people.people.map(p=>p.name).join()==='Kamba,Sam');
      ok('the owner has no remove control',!action(card('Kamba'),'Remove Kamba'));
      ok('recovery controls are hidden for healthy voices',document.getElementById('speaker-reset-voices').hidden && document.getElementById('speaker-recover').hidden);
      people.health={state:'voices_unavailable'};document.getElementById('panel').close();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('speaker-reset-voices').hidden);document.getElementById('speaker-reset-voices').click();ok('voice reset requires explicit local confirmation',count('speaker-reset')===0);document.getElementById('speaker-confirm-no').click();ok('cancelling voice reset preserves saved voices',people.people[0].voice_enrolled);document.getElementById('speaker-reset-voices').click();document.getElementById('speaker-confirm-yes').click();await until(()=>count('speaker-reset')===1);ok('confirmed reset keeps people and returns to Anyone',people.people.length===2 && people.mode==='open');
      people.recovery_required=true;document.getElementById('panel').close();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('speaker-recover').hidden);document.getElementById('speaker-recover').click();ok('people recovery requires explicit confirmation',count('speaker-recovered')===0);document.getElementById('speaker-confirm-yes').click();await until(()=>count('speaker-recovered')===1);
      document.getElementById('panel').close();
      if(location.hash==='#people-only'){output.textContent=JSON.stringify({status:'passed',passed,adapters:'synthetic IPC and local capture; no microphone, keys or provider calls'},null,2);return;}
      await until(()=>document.getElementById('orb').dataset.ready==='true');
      await clean();delayedStart=true;let pendingReady=count('pending-capture'),readyCaptures=count('capture');orbClick();await until(()=>count('pending-capture')>pendingReady);await sleep(100);ok('preparing feedback does not claim a live microphone',count('capture')===readyCaptures && document.getElementById('orb').dataset.state==='thinking' && document.getElementById('orb-status').textContent.includes('Starting microphone'));await until(()=>count('capture')===readyCaptures+1);await until(()=>document.getElementById('orb').dataset.state==='listening');pressEscape();delayedStart=false;
      await clean();settings.followup_enabled=false;await sleep(600);let n=count('capture');await initial();await until(()=>count('ended')>=2);await sleep(1500);ok('default/off: no automatic capture',count('capture')===n+1);
      await clean();let m=count('message');await initial();await nextWindow();ok('distinct orb follow-up feedback',document.getElementById('orb-status').textContent.includes('follow-up'));
      submit('Half eight.');await until(()=>count('message')===m+2);await nextWindow();submit('That’s all.');await sleep(1400);ok('two-turn continuity; stop is not sent to model',count('message')===m+2);let captures=count('capture');await sleep(800);ok('goodbye does not reopen',count('capture')===captures);
      await clean();m=count('message');await initial();await nextWindow();submit('');await sleep(1400);let emptyCaptures=count('capture');await sleep(800);ok('provider-confirmed empty speech closes without a model call or settings dialog',count('message')===m+1 && count('capture')===emptyCaptures && !document.getElementById('panel').open);
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
      await clean();n=count('capture');let pending=count('pending-capture');await initial();pending=count('pending-capture');delayedStart=true;await until(()=>count('pending-capture')>pending);await sleep(100);ok('follow-up waits for actual microphone readiness',count('capture')===n+1 && document.getElementById('orb').dataset.state==='thinking' && document.getElementById('orb-status').textContent.includes('Starting microphone'));pressEscape();await sleep(1100);delayedStart=false;ok('Escape cancels an in-flight start without resurrecting capture',count('capture')===n+1);
      await clean();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('model').disabled);const toggle=document.getElementById('followup-enabled');toggle.checked=true;toggle.dispatchEvent(new Event('change'));const range=document.getElementById('followup-seconds');range.value='12';range.dispatchEvent(new Event('input'));const saved=count('saved');document.getElementById('save-settings').click();await until(()=>count('saved')>saved);ok('settings control saves the chosen follow-up duration',settings.followup_enabled && settings.followup_seconds===12);
      await clean();settings.followup_enabled=false;await sleep(600);let changes=count('session-answer');n=count('capture');state.session_pending='session-confirm';await until(()=>count('capture')===n+1);submit('yes, please send that message');await until(()=>count('session-answer')===changes+1);await sleep(600);ok('session action uses scoped spoken confirmation',count('session-answer')===changes+1);
      await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');let questions=count('session-question');state.session_pending='session-automatic';await until(()=>count('session-question')===questions+1);await sleep(800);ok('authorised direct action does not open confirmation capture',count('capture')===n);
      await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');changes=count('session-answer');let sessionCancels=count('session-cancel');state.session_pending='session-cancel';await until(()=>count('capture')===n+1);pressEscape();await until(()=>count('session-cancel')>sessionCancels);ok('Escape cancels session consent without sending an answer',count('session-answer')===changes);
      await clean();document.getElementById('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await until(()=>!document.getElementById('model').disabled);document.getElementById('session-direct').checked=true;document.getElementById('task-notifications').checked=true;let savedMode=count('saved');document.getElementById('save-settings').click();await until(()=>count('saved')>savedMode);ok('session control and task reporting preferences persist through Settings',settings.session_control==='when_asked' && settings.task_notifications===true);
      await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');let notices=count('notice-end');state.initiative={announce:'task-off',task_report:true};await until(()=>count('notice-end')===notices+1);await sleep(1300);ok('task result is spoken once without opening capture when follow-ups are off',count('capture')===n && count('notice-end')===notices+1);
      await clean();await sleep(600);n=count('capture');m=count('message');state.initiative={announce:'task-on',task_report:true};await nextWindow();ok('completed task opens one opt-in follow-up after the audio echo guard',count('capture')===n+1);submit("That's all");await sleep(1000);ok('spoken stop after a task result does not send another request',count('message')===m);
      await clean();settings.followup_enabled=false;settings.microphone_muted=true;await sleep(600);notices=count('notice-start');state.initiative={announce:'task-muted',task_report:true};await sleep(800);ok('muting defers task announcements',count('notice-start')===notices);settings.microphone_muted=false;await until(()=>count('notice-start')===notices+1);await until(()=>count('notice-end')>=notices+1);
      await clean();await sleep(600);n=count('capture');notices=count('notice-start');let interrupted=count('notice-interrupted');const previousError=document.getElementById('error').textContent;state.initiative={announce:'task-cancel',task_report:true};await until(()=>count('notice-start')===notices+1);pressEscape();await until(()=>count('notice-interrupted')===interrupted+1);await sleep(1300);ok('Escape cancels task speech without resurrecting follow-up capture',count('capture')===n);ok('intentional task-report cancellation does not show an error',document.getElementById('error').textContent===previousError);
      await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');m=count('message');let applied=count('settings-applied');queueSettings('settings-approved',{hue:240});await until(()=>count('capture')===n+1);ok('settings readback does not apply an unapproved change',settings.hue===192 && count('settings-applied')===applied);submit('Yes, please change those settings.');await until(()=>count('settings-applied')===applied+1);await until(()=>document.documentElement.style.getPropertyValue('--hue')==='240');ok('exact settings approval applies and updates the orb theme',settings.hue===240 && count('message')===m);
      for(const answer of ['yes','no','yes, send it']){await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');applied=count('settings-applied');const hue=settings.hue;let rejected=count('settings-rejected');queueSettings('settings-reject-'+rejected,{hue:60});await until(()=>count('capture')===n+1);submit(answer);await until(()=>count('settings-rejected')===rejected+1);ok('settings rejects '+JSON.stringify(answer)+' without changing preferences',settings.hue===hue && count('settings-applied')===applied);}
      await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');applied=count('settings-applied');let stale=count('settings-stale');queueSettings('settings-stale',{hue:60});await until(()=>count('capture')===n+1);settings.hue=120;submit('yes, please change those settings');await until(()=>count('settings-stale')===stale+1);ok('a concurrent settings edit invalidates the old approval',settings.hue===120 && count('settings-applied')===applied);
      await clean();settings.followup_enabled=false;await sleep(600);n=count('capture');applied=count('settings-applied');let answers=count('settings-answer'),aborted=count('settings-aborted');queueSettings('settings-async-cancel',{hue:300},1200);await until(()=>count('capture')===n+1);submit('yes, please change those settings');await until(()=>count('settings-answer')===answers+1);pressEscape();await until(()=>count('settings-aborted')===aborted+1);captures=count('capture');let playback=count('play');await sleep(800);ok('Escape revokes settings approval while native validation is pending',settings.hue===120 && count('settings-applied')===applied && count('capture')===captures && count('play')===playback);
      await clean();const auto=log.filter(x=>x.event==='capture'&&x.followup);ok('no capture during playback or inter-sentence gaps',auto.length>=5 && auto.every(x=>x.gap>=590));
      ok('speech output graph is reused across replies and cancellations',count('play')>2 && count('output-graph')===(navigator.userAgent.includes('Linux')?0:1));
      ok('cancellation clears the previous speech source',count('audio-source-cleared')>0);
      output.textContent=JSON.stringify({status:'passed',passed,modelMessages:count('message'),automaticCaptures:auto.length,minimumEchoGapMs:Math.min(...auto.map(x=>x.gap)),adapters:'synthetic IPC, model, STT and audio; no real microphone, speaker, credentials or provider calls'},null,2);
    }catch(error){pressEscape();output.textContent=JSON.stringify({status:'failed',error:String(error),passed,lastEvents:log.filter(x=>x.event!=='lease').slice(-24)},null,2);}
  }
  document.addEventListener('DOMContentLoaded',()=>{const box=document.createElement('aside');box.style='position:fixed;z-index:9999;right:4px;top:4px;background:#132333;color:white;padding:10px;max-width:500px;font:12px monospace;max-height:90vh;overflow:auto';box.innerHTML='<button id="run-silent-checks">Run silent integration checks</button><pre id="harness-results">Ready. All audio and services are synthetic.</pre>';document.body.append(box);document.getElementById('run-silent-checks').onclick=run;});
})();
