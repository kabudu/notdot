#!/usr/bin/env bash
# GitHub Ubuntu runners sometimes advertise an unreachable Azure HTTP mirror.
# Use Ubuntu's signed official HTTPS archive and bound retries for all apt work.
set -euo pipefail
sudo python3 - <<'PY'
from pathlib import Path
paths = [Path('/etc/apt/sources.list')]
paths += list(Path('/etc/apt/sources.list.d').glob('*.list'))
paths += list(Path('/etc/apt/sources.list.d').glob('*.sources'))
for path in paths:
    if path.is_file():
        old = path.read_text()
        new = old.replace('http://azure.archive.ubuntu.com/ubuntu', 'https://archive.ubuntu.com/ubuntu')
        new = new.replace('https://azure.archive.ubuntu.com/ubuntu', 'https://archive.ubuntu.com/ubuntu')
        if new != old:
            path.write_text(new)
Path('/etc/apt/apt.conf.d/80notdot-ci').write_text(
    'Acquire::Retries "2";\nAcquire::http::Timeout "20";\nAcquire::https::Timeout "20";\n'
)
PY
sudo apt-get update -o APT::Update::Error-Mode=any
