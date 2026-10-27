#!/usr/bin/env python3
"""Run a compiled release bench repeatedly. No user data, voice or account calls."""
import hashlib, json, platform, subprocess, sys, time
from pathlib import Path
root=Path(__file__).resolve().parents[1]
binary=root/'target/release/examples/acceptance_bench'
if not binary.is_file():
    raise SystemExit('Build first: cargo build -p notdot-core --example acceptance_bench --release --locked')
runs=[]
for repeat in range(3):
    for records in (100,1000):
        start=time.monotonic()
        result=subprocess.run([str(binary),str(records)],cwd=root,text=True,capture_output=True,timeout=60,check=True)
        run=json.loads(result.stdout)
        assert run['failures']==0 and run['completed_queries']==40 and run['returned_hits']==240 and run['mode']=='release',run
        run.update(repeat=repeat+1,elapsed_seconds=time.monotonic()-start)
        runs.append(run)
report={'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'working_tree':'bench may be uncommitted; retain source hash with results','source_sha256':hashlib.sha256((root/'crates/notdot-core/examples/acceptance_bench.rs').read_bytes()).hexdigest(),'platform':platform.platform(),'rust':subprocess.check_output(['rustc','--version'],text=True).strip(),'method':'3 fresh-process repetitions per size, alternated; 40 verified six-hit recalls each; no model/voice/renderer measurement','runs':runs}
text=json.dumps(report,indent=2)+'\n'
if len(sys.argv)>1:Path(sys.argv[1]).write_text(text)
else:print(text,end='')
