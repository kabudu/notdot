import json
from pathlib import Path
import re
import sys
import subprocess
version = sys.argv[1]
if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
    raise SystemExit('Use a stable major.minor.patch version')
packages = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version', '1', '--no-deps']))['packages']
versions = {p['version'] for p in packages if p['name'].startswith('notdot')}
config = json.loads(Path('src-tauri/tauri.conf.json').read_text())
assert versions == {version} and config['version'] == version, 'Version mismatch'
assert config['plugins']['updater']['requireSignedVersion'], 'Version-bound signatures are required'
assert config['plugins']['updater']['pubkey'].strip() == Path('assets/updater.pub').read_text().strip(), 'Updater key mismatch'
assert Path('docs/release-notes.md').is_file(), 'Write reviewed release notes before preparing a release'
