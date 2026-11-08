#!/usr/bin/env python3
"""Verify pinned native archives once, before Cargo's per-profile builds."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
from install_release_tools import download
ROOT=Path(__file__).resolve().parents[1]
MANIFEST=ROOT/'third_party/sherpa-native-archives.json'
MAX_ARCHIVE=128*1024*1024

def prepare(target, output, manifest=MANIFEST, fetch=download):
    metadata=json.loads(Path(manifest).read_text())
    version=metadata['version']
    targets=['aarch64-apple-darwin','x86_64-apple-darwin'] if target=='universal-apple-darwin' else [target]
    entries=[metadata['targets'][name] for name in targets]
    output=Path(output)
    if output.is_symlink():raise ValueError('Refusing a symlinked archive cache')
    output.mkdir(parents=True,exist_ok=True,mode=0o700)
    for entry in entries:
        name=entry['archive'];size=entry['bytes']
        if not re.fullmatch(r'sherpa-onnx-v'+re.escape(version)+r'-[A-Za-z0-9-]+\.tar\.bz2',name):raise ValueError('Invalid native archive name')
        if not isinstance(size,int) or not 0<size<=MAX_ARCHIVE:raise ValueError('Invalid native archive size')
        destination=output/name
        if destination.is_symlink():raise ValueError('Refusing a symlinked archive')
        if destination.is_file() and destination.stat().st_size==size and hashlib.sha256(destination.read_bytes()).hexdigest()==entry['sha256']:
            continue
        payload=fetch(f'https://github.com/k2-fsa/sherpa-onnx/releases/download/v{version}/{name}',limit=size)
        if len(payload)!=size or hashlib.sha256(payload).hexdigest()!=entry['sha256']:raise ValueError('Native archive size or SHA-256 mismatch')
        temporary=None
        try:
            with tempfile.NamedTemporaryFile(dir=output,prefix='download-',delete=False) as stream:
                temporary=Path(stream.name);stream.write(payload)
            os.replace(temporary,destination)
        finally:
            if temporary is not None:temporary.unlink(missing_ok=True)
    return output.resolve()

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target')
    parser.add_argument('--output',type=Path,default=ROOT/'.local/sherpa-archives')
    args=parser.parse_args()
    target=args.target
    if not target:
        target=next(line.split(': ',1)[1] for line in subprocess.check_output(['rustc','-vV'],text=True).splitlines() if line.startswith('host: '))
    metadata=json.loads(MANIFEST.read_text())
    lock=tomllib.loads((ROOT/'Cargo.lock').read_text())
    assert next(p['version'] for p in lock['package'] if p['name']=='sherpa-onnx-sys')==metadata['version'],'Native archive version must match Cargo.lock'
    output=prepare(target,args.output)
    if any(char in str(output) for char in '\r\n'):raise ValueError('Invalid archive cache path')
    if path:=os.environ.get('GITHUB_ENV'):
        with open(path,'a',encoding='utf-8') as stream:stream.write(f'SHERPA_ONNX_ARCHIVE_DIR={output}\n')
    print(f'Verified Sherpa archives for {target}: {output}')
if __name__=='__main__':main()
