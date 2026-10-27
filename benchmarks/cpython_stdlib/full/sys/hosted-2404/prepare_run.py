"""Record the exact product checkout and its two release artifacts."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

folder = Path(__file__).resolve().parent
repo = Path(os.environ['GITHUB_WORKSPACE'])
source = os.environ['SYS_SOURCE']
git = lambda *args: subprocess.check_output(['git','-C',repo,*args],text=True).strip()
assert sys.version_info[:3] == (3,14,5) and git('rev-parse','HEAD') == source
files = {name:repo/'target/release'/name for name in ('imre','libimre_runtime.a')}
parents = [line[7:] for line in git('cat-file','-p','HEAD').split('\n\n',1)[0].splitlines() if line.startswith('parent ')]
assert len(parents) == 2
pair = {'source':source,'tree':git('rev-parse','HEAD^{tree}'),'parents':parents,'paths':{n:str(p) for n,p in files.items()},'sha256':{n:sha(p) for n,p in files.items()},'python_sha256':sha(Path(sys.executable)),'mode':'ordinary release compiler and runtime','workflow_sha':os.environ['GITHUB_SHA']}
for dst in (folder/'pair.json',folder/'area/pair/pair.json'):
    dst.parent.mkdir(parents=True,exist_ok=True)
    dst.write_text(json.dumps(pair,indent=2)+'\n')
(folder/'area/tmp').mkdir(parents=True,exist_ok=True)
manifest=json.loads((folder/'click-input.json').read_bytes())
manifest['source']=source
manifest['parents']=pair['parents']
del manifest['parent']
(folder/'inputs.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(pair))
