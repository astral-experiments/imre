"""Run original socketserver test registration and a fixture-preserving isolated descriptor check."""
import argparse, hashlib, json, os, signal, subprocess, time
from pathlib import Path
root=Path(__file__).resolve().parent
manifest=json.loads((root/'manifest.json').read_bytes())
def sha(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def save(p,d):
    t=p.with_suffix('.tmp');t.write_text(json.dumps(d,indent=2)+'\n');t.replace(p)
def run(cmd,cwd,limit):
    cmd=list(map(str,cmd));cwd.mkdir(parents=True);env={k:v for k,v in os.environ.items() if k not in ('PYTHONPATH','IMREFLAGS','IMRE_ENCODED_FLAGS')}
    env.update(TMPDIR=str(cwd),PYTHONDONTWRITEBYTECODE='1')
    with (cwd/'stdout').open('wb') as out,(cwd/'stderr').open('wb') as err:
        start=time.monotonic();p=subprocess.Popen(cmd,cwd=cwd,env=env,stdout=out,stderr=err,stdin=subprocess.DEVNULL,start_new_session=True)
        expired=False
        try:p.wait(limit)
        except subprocess.TimeoutExpired:expired=True;os.killpg(p.pid,signal.SIGKILL);p.wait()
    r={'argv':cmd,'cwd':str(cwd),'exit':p.returncode,'seconds':round(time.monotonic()-start,3),'timed_out':expired,'timeout_seconds':limit,
       'stdout':(cwd/'stdout').read_text(errors='replace'),'stderr':(cwd/'stderr').read_text(errors='replace')}
    save(cwd/'receipt.json',r);return r
