#!/usr/bin/env python3
"""Prepare a new macOS-only QA profile. Refuses to overwrite any existing QA data."""
import json, os, sys
from pathlib import Path
if sys.platform != 'darwin':raise SystemExit('This helper currently prepares the macOS QA path only')
root=Path.home()/'Library/Application Support'
source=root/'com.kabudu.notdot/settings.json'
profile=root/'com.kabudu.notdot.qa'
if profile.exists():raise SystemExit(f'QA profile already exists; review it instead of overwriting: {profile}')
settings=json.loads(source.read_text()) if source.exists() else {}
# Copy only non-secret voice/model preferences; never copy conversation or memory.
keep={key:settings[key] for key in ('voice_provider','voice_id','voice_name','model','effort') if key in settings}
keep.update(name='notdot QA',hue=285,memory_enabled=True,onboarding_complete=True,thread_id=None,startup='manual',open_desktop=False,automatic_updates=False,wake_enabled=False,microphone_muted=False,initiative={'behaviour':'reactive','quiet_start':1320,'quiet_end':480,'daily_limit':3,'gap_minutes':60})
profile.mkdir(mode=0o700)
path=profile/'settings.json'
fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
with os.fdopen(fd,'w') as out:json.dump(keep,out,indent=2);out.write('\n')
print(f'Created isolated QA settings: {profile}. No memory, conversation or API key was copied.')
