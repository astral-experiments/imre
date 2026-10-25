"""Read-only scratch CI: rebuild in place if any of #810's files changed on main."""
import hashlib,json,os,subprocess,sys
from pathlib import Path
def out(*args): return subprocess.check_output(args,text=True).strip()
def run(*args): subprocess.run(args,check=True)
def fetch():
    out('git','fetch','--no-tags','origin','refs/heads/main'); return out('git','rev-parse','FETCH_HEAD')
temp=Path(os.environ['RUNNER_TEMP']); target=temp/'subprocess-finalizer-target'; os.environ['CARGO_TARGET_DIR']=str(target)
head='369628bb170126593d8778ff190a72f5e87072d6'; paths=['crates/imre-compiler/src/'+n for n in ('contracts.rs','frontend.rs','ir.rs','llvm.rs','modules.rs','modules/declarations.rs')]+['crates/imre-runtime/src/'+n for n in ('objects.rs','objects/processes/popen.rs','objects/warnings.rs')]+['tests/fixtures/processes/unobserved_child.py','tests/processes.py']
assert sys.version_info[:3]==(3,14,5) and len(paths)==11
while True:
    main=fetch(); tree=out('git','merge-tree','--write-tree',main,head); assert len(tree)==40 and out('git','diff','--name-only',main,tree).splitlines()==sorted(paths)
    # The verifier scripts were copied out before selecting the exact product.
    run('git','read-tree','--reset','-u',tree); assert out('git','write-tree')==tree and not out('git','diff','--name-only')
    print('Testing',main,tree,flush=True); run('cargo','build','--locked','-j','2','-p','imre-compiler','-p','imre-runtime')
    info=dict(main=main,head=head,tree=tree,run=int(os.environ['GITHUB_RUN_ID']),verification_workflow=os.environ['GITHUB_SHA'])
    (temp/'subprocess-finalizer-source.json').write_text(json.dumps(info)); run(sys.executable,str(temp/'ci-finalizer-probe.py'))
    current=fetch(); changed=out('git','diff','--name-only',main,current,'--',*paths)
    if changed:
        print('Main advanced in:',changed,flush=True); continue
    receipt=temp/'subprocess-finalizer-verified.json'; data=json.loads(receipt.read_text()); assert all(data[k]==v for k,v in info.items()); data['main_after_test']=current; data['paths']=paths
    receipt.write_text(json.dumps(data,indent=2)+'\n'); print('Verified',main,tree,'still applicable to',current,flush=True); break
