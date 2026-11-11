/* Local-only UX acceptance, synthetic native adapter. */
(async()=>{
 if(new URLSearchParams(location.search).has("manual"))return;
 const sleep=ms=>new Promise(r=>setTimeout(r,ms)), $=id=>document.getElementById(id);
 const report={viewport:[innerWidth,innerHeight],checks:[],failures:[],sections:[]};
 const check=(name,pass,detail)=>{report.checks.push(name);if(!pass)report.failures.push({name,detail});};
 const shown=e=>e.getClientRects().length && getComputedStyle(e).visibility!=='hidden';
 const rect=e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height,bottom:r.bottom,right:r.right};};
 try{
 for(let i=0;i<100&&!$('orb').hasAttribute('data-ready');i++)await sleep(50);
 $('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await sleep(150);
 check('modal opens and names itself',$('panel').open&&$('panel').getAttribute('aria-labelledby')==='heading');
 check('search autofocus',document.activeElement===$('settings-search'));
 const panel=rect($('panel')),footer=rect(document.querySelector('.settings-footer'));
 check('panel inside viewport',panel.x>=0&&panel.y>=0&&panel.right<=innerWidth+1&&panel.bottom<=innerHeight+1,panel);
 check('footer inside viewport',footer.bottom<=innerHeight+1,footer);
 check('content has usable height',$('settings-content').clientHeight>=48,$('settings-content').clientHeight);
 for(const category of document.querySelectorAll('[role=tab]')){
  category.click();await sleep(60);
  check(category.textContent+' uniquely selected',document.querySelectorAll('[role=tab][aria-selected=true]').length===1);
  for(const section of document.querySelectorAll('.settings-section:not([hidden])')){
   if(!section.open)section.querySelector('summary').click();await sleep(80);
   const overflow=[...section.querySelectorAll('input,select,button,label,p,fieldset')].filter(shown).filter(e=>e.getBoundingClientRect().right>section.getBoundingClientRect().right+1||e.getBoundingClientRect().left<section.getBoundingClientRect().left-1).map(e=>({id:e.id,tag:e.tagName,rect:rect(e)}));
   check(section.id+' has no horizontal overflow',overflow.length===0,overflow);
   check(section.id+' scroll width fits',$('settings-content').scrollWidth<=$('settings-content').clientWidth+1,[$('settings-content').scrollWidth,$('settings-content').clientWidth]);
   report.sections.push(section.id);
   for(const input of section.querySelectorAll('input:not([hidden]),select,button:not([hidden])')){
    if(!shown(input)||input.disabled||input.type==='hidden'||input.tabIndex<0)continue;
    // WebKit applies focus scrolling on a display frame, including at page zoom.
    input.focus();await sleep(40);const r=rect(input),content=rect($('settings-content'));
    check((input.id||input.textContent.trim())+' keyboard scroll visibility',r.bottom>content.y&&r.y<content.bottom,{input:r,content});
   }
  }
 }
 $('settings-category-voice').click();
 if(!$('people-section').open)$('people-section').querySelector('summary').click();
 const teach=[...$('speaker-people').querySelectorAll('button')].find(b=>b.textContent==='Teach voice');
 teach.click();await sleep(100);
 check('enrolment focuses consent',document.activeElement===$('speaker-consent'));
 check('record disabled before consent',$('speaker-record').disabled);
 $('speaker-consent').checked=true;$('speaker-consent').dispatchEvent(new Event('change',{bubbles:true}));
 for(let phrase=0;phrase<4;phrase++){$('speaker-record').focus();$('speaker-record').click();await sleep(200);}
 check('completed enrolment focuses save',document.activeElement===$('speaker-enrol-save'));
 $('speaker-enrol-cancel').click();await sleep(50);
 check('enrolment cancel returns focus',document.activeElement===teach&&$('speaker-enrol').hidden);
 const remove=[...$('speaker-people').querySelectorAll('button')].find(b=>b.textContent.startsWith('Remove '));
 remove.click();await sleep(50);
 check('removal focuses named confirmation',document.activeElement===$('speaker-confirm-name')&&$('speaker-confirm-yes').disabled);
 check('long confirmation has no horizontal overflow',$('settings-content').scrollWidth<=$('settings-content').clientWidth+1);
 $('speaker-confirm-no').click();await sleep(50);
 check('removal cancel returns focus',document.activeElement===remove&&$('speaker-confirm').hidden);
 for(const hue of [0,40,120,192,265,320,360]){
  $('hue').value=String(hue);$('hue').dispatchEvent(new Event('input',{bubbles:true}));
  check('theme '+hue+' previews immediately',document.documentElement.style.getPropertyValue('--hue')===String(hue));
 }
 const search=$('settings-search');search.value='zzzz-no-setting';search.dispatchEvent(new Event('input',{bubbles:true}));await sleep(50);
 check('empty search feedback',$('settings-search-status').textContent.includes('No'));
 search.focus();search.value='b';search.dispatchEvent(new Event('input',{bubbles:true}));await sleep(50);
 check('opening search results keeps typing focus',document.activeElement===search);
 search.value+='ackup';search.dispatchEvent(new Event('input',{bubbles:true}));await sleep(50);
 check('backup search stays focused and reveals backup',document.activeElement===search&&!$('backup-section').hidden);
 search.value='quiet';search.dispatchEvent(new Event('input',{bubbles:true}));await sleep(50);
 {const shown=[...document.querySelectorAll('#settings-content .settings-section')].filter(s=>!s.hidden);check('matching search feedback',$('settings-search-status').textContent===shown.length+' sections match.'&&shown.length>=2&&!$('reminders-section').hidden);}
 $('settings-search-clear').click();await sleep(50);check('clear restores categories',!$('settings-categories').hidden&&!search.value);
 $('settings-category-voice').click();if(!$('voice-section').open)$('voice-section').querySelector('summary').click();$('load-voices').click();await sleep(100);$('voice-toggle').click();await sleep(80);
 check('voice picker expands',$('voice-toggle').getAttribute('aria-expanded')==='true'&&!$('voice-popup').hidden);
 check('voice search appears for eight voices',!$('voice-search-label').hidden);
 const toggle=rect($('voice-toggle')),preview=rect($('preview-voice'));
 check('voice icons aligned',Math.abs(toggle.y+toggle.h/2-preview.y-preview.h/2)<1,{toggle,preview});
 $('voice-search').value='no-such-voice';$('voice-search').dispatchEvent(new Event('input',{bubbles:true}));await sleep(400);
 check('voice no results readable',$('voice-info').textContent.includes('No matching'));
 $('settings-category-codex').click();
 if(!$('intelligence-section').open)$('intelligence-section').querySelector('summary').click();
 check('model cards and efforts available',$('model-options').querySelectorAll('input').length===3&&$('effort-options').querySelectorAll('input').length===5);
 $('model-search').value='no-such-model';$('model-search').dispatchEvent(new Event('input',{bubbles:true}));await sleep(50);
 check('model no results readable',$('model-search-status').textContent.includes('No'));
 $('settings-category-voice').click();
 if(!$('usage-section').open)$('usage-section').querySelector('summary').click();
 check('usage progress has accessible label',!!$('usage-details').querySelector('progress[aria-label][aria-valuetext]'));
 $('settings-category-app').click();
 if(!$('startup-section').open)$('startup-section').querySelector('summary').click();
 $('show-about').focus();$('show-about').click();await sleep(50);
 check('About is a named modal',$('about').open&&$('about').getAttribute('aria-labelledby')==='about-title');
 $('close-about').click();await sleep(50);
 check('About returns to Settings',$('panel').open&&document.activeElement===$('show-about'));
 // Private notes and backups never outlive Settings or the active person.
 const reopen=async()=>{$('orb').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));await sleep(200);$('settings-category-memory').click();await sleep(50);};
 const notesText=()=>$('voice-notes').textContent+$('notes').textContent;
 $('settings-category-memory').click();await sleep(50);
 $('review-voice-notes').click();await sleep(250);
 check('owner notes render',notesText().includes('Gate code'));
 // Password problems name the field, mark it invalid and describe why.
 const typed=(id,v)=>{$(id).value=v;$(id).dispatchEvent(new Event('input',{bubbles:true}));};
 const describedBy=id=>($(id).getAttribute('aria-describedby')||'').split(' ');
 // Category names stay whole words on one line, inside the bar.
 const lines=b=>{const r=document.createRange();r.selectNodeContents(b);return new Set([...r.getClientRects()].map(x=>Math.round(x.top))).size;};
 check('category names never break inside a word',[...$('settings-categories').querySelectorAll('button')].every(b=>lines(b)===1&&b.scrollWidth<=b.clientWidth+1)&&$('settings-categories').scrollWidth<=$('settings-categories').clientWidth+1);
 const rgb=c=>c.match(/[\d.]+/g).slice(0,3).map(Number),lum=v=>{const f=x=>{x/=255;return x<=.03928?x/12.92:((x+.055)/1.055)**2.4};const [r,g,b]=v.map(f);return .2126*r+.7152*g+.0722*b;},ratio=(a,b)=>{const [x,y]=[lum(a),lum(b)].sort((m,n)=>n-m);return (x+.05)/(y+.05);};
 const normalBorder=getComputedStyle($('backup-password')).borderTopColor;
 typed('backup-password','too short');$('backup-save').click();await sleep(50);
 {const s=getComputedStyle($('backup-password'));check('invalid password field has a visible border with 3:1 contrast',s.borderTopColor!==normalBorder&&ratio(rgb(s.borderTopColor),rgb(s.backgroundColor))>=3);}
 check('short backup password is marked invalid and keeps its text',$('backup-password').getAttribute('aria-invalid')==='true'&&describedBy('backup-password').includes('backup-status')&&describedBy('backup-password').includes('backup-password-hint')&&document.activeElement===$('backup-password')&&$('backup-password').value==='too short'&&$('backup-status').textContent.includes('12 characters'));
 typed('backup-password','correct horse battery');
 check('typing clears the invalid state',!$('backup-password').hasAttribute('aria-invalid')&&!describedBy('backup-password').includes('backup-status'));
 typed('backup-password-repeat','correct horse batterx');$('backup-save').click();await sleep(50);
 check('mismatched repeat is marked invalid and focused',$('backup-password-repeat').getAttribute('aria-invalid')==='true'&&describedBy('backup-password-repeat').includes('backup-status')&&!$('backup-password').hasAttribute('aria-invalid')&&document.activeElement===$('backup-password-repeat')&&$('backup-password').value==='correct horse battery'&&!$('backup-password-repeat').value);
 typed('backup-password','wrong password!!');$('backup-open').click();await sleep(250);
 check('wrong restore password is marked invalid on the password field',$('backup-password').getAttribute('aria-invalid')==='true'&&document.activeElement===$('backup-password')&&!$('backup-password').value&&$('backup-status').textContent.startsWith('The password is wrong'));
 $('backup-password').value='synthetic password';$('backup-open').click();await sleep(250);
 check('a valid restore password clears the invalid state',!$('backup-password').hasAttribute('aria-invalid'));
 check('restore preview waits for confirmation',!$('backup-confirm').hidden&&$('backup-preview').textContent.includes('3 memories')&&!!$('backup-apply').getAttribute('data-restore-id'));
 check('restore preview focuses the safe choice',document.activeElement===$('backup-keep'));
 $('backup-password').value='typed but unused';
 $('close').click();await sleep(150);
 check('closing clears notes, passwords and the pending restore',!notesText()&&!$('backup-password').value&&!$('backup-password-repeat').value&&$('backup-confirm').hidden&&!$('backup-preview').textContent&&!$('backup-apply').hasAttribute('data-restore-id')&&!$('backup-password').hasAttribute('aria-invalid')&&!$('backup-password-repeat').hasAttribute('aria-invalid'));
 await reopen();
 check('reopened Settings shows no cached notes',!notesText());
 // A slow owner result must not render after another person becomes active.
 window.notesDelay=700;$('review-voice-notes').click();await sleep(50);
 window.settingsFixture.state.profile_id='member';await sleep(1100);
 check('person change clears private Settings content',!notesText().includes('Gate code'));
 check('late owner notes are discarded after a person change',!notesText().includes('Gate code')&&!notesText().includes('shopping list'));
 $('review-voice-notes').click();await sleep(250);
 check('new person sees only their own notes',notesText().includes('Member chores')&&!notesText().includes('Gate code'));
 window.inspectDelay=600;$('backup-password').value='synthetic password';$('backup-open').click();await sleep(50);
 $('close').click();await sleep(800);
 check('late restore preview is discarded after Settings closes',$('backup-confirm').hidden&&!$('backup-preview').textContent);
 await reopen();
 window.settingsFixture.state.profile_id='owner';await sleep(700);
 check('switching back starts blank',!notesText());
 window.settingsFixture.state.error='The voice service could not be reached. '+('DiagnosticIdentifierWithoutSpaces'.repeat(40));
 await sleep(650);
 check('long error keeps footer visible',rect(document.querySelector('.settings-footer')).bottom<=innerHeight+1);
 check('long error keeps content usable',$('settings-content').clientHeight>=48,$('settings-content').clientHeight);
 }catch(e){report.failures.push({exception:String(e)});}
 report.status=report.failures.length?'failed':'passed';
 const output=document.createElement('pre');output.id='settings-qa-result';output.textContent=JSON.stringify({status:report.status,viewport:report.viewport,total:report.checks.length,failureNames:report.failures.map(f=>f.name||f.exception),failures:report.failures,sections:report.sections,checks:report.checks},null,2);output.style='position:fixed;z-index:99999;bottom:0;left:0;font:9px monospace;background:#111;color:white;max-height:20px;overflow:auto';document.body.append(output);
 if(parent!==window)parent.postMessage(report,location.origin);
})();
