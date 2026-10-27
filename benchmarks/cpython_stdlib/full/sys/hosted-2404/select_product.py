"""Select a current, reproducible product merge after the hosted Rust target is warm."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

base = '1f7821023cccb72ee3ba7e7d971d06b877923093'
head = 'aa8ab3a353e43f92498e8fcd5d69d11c4bdcd3a9'
folder = Path(__file__).resolve().parent
repo = Path(os.environ['GITHUB_WORKSPACE'])
incoming = repo / '.textio-base'
label = sys.argv[1]
assert label in ('warm', 'end')
def git(*args, directory=repo, env=None):
    return subprocess.check_output(['git','-C',directory,*args],text=True,env=env).strip()
def parents(commit):
    return [s[7:] for s in git('cat-file','-p',commit).split('\n\n',1)[0].splitlines() if s.startswith('parent ')]
current = git('rev-parse','HEAD')
current_base,check_head = parents(current)
assert current == os.environ['SYS_SOURCE'] and check_head == head
latest = git('rev-parse','HEAD',directory=incoming)
git('fetch','--no-tags','--depth=1',str(incoming),latest)
touched=set(git('diff','--name-only',base,head).splitlines()) | {
    'crates/imre-runtime/src/dynamic/stdlib/codecs.rs','crates/imre-runtime/src/objects/codecs.rs',
    'tests/fixtures/boxed_codecs.py','tests/fixtures/boxed_descriptors.py'}
overlap=sorted(set(git('diff','--name-only',current_base,latest).splitlines()) & touched)
source=current
if overlap:
    tree=git('merge-tree','--write-tree','--merge-base='+base,latest,head)
    date=git('show','-s','--format=%cI',latest)
    ident={'GIT_AUTHOR_NAME':'Imre CI qualification','GIT_COMMITTER_NAME':'Imre CI qualification',
           'GIT_AUTHOR_EMAIL':'qualification@localhost','GIT_COMMITTER_EMAIL':'qualification@localhost',
           'GIT_AUTHOR_DATE':date,'GIT_COMMITTER_DATE':date}
    source=git('commit-tree',tree,'-p',latest,'-p',head,'-m','Validate the prospective TextIOWrapper merge for #2404',env={**os.environ,**ident})
    git('checkout','--detach',source)
    assert parents(source)==[latest,head] and git('rev-parse','HEAD^{tree}')==tree
else:
    tree=git('rev-parse','HEAD^{tree}')
raw=subprocess.check_output(['git','-C',repo,'cat-file','commit',source])
assert hashlib.sha1(b'commit '+str(len(raw)).encode()+b'\0'+raw).hexdigest()==source
doc={'label':label,'previous_source':current,'previous_base':current_base,'observed_main':latest,
     'overlap':overlap,'source':source,'tree':tree,'parents':parents(source),'commit_object':raw.decode()}
(folder/f'source-{label}.json').write_text(json.dumps(doc,indent=2)+'\n')
with open(os.environ['GITHUB_ENV'],'a') as f:print('SYS_SOURCE='+source,file=f)
with open(os.environ['GITHUB_OUTPUT'],'a') as f:print('changed='+str(bool(overlap)).lower(),file=f)
print(json.dumps({k:v for k,v in doc.items() if k!='commit_object'}))
