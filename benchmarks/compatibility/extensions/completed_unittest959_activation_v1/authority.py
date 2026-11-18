"""Append-only public authority for a completed finite original-test cohort."""
from functools import lru_cache
import json,subprocess
import raw_control,source_stage_pins
from extensions.successor959_v1 import authority as common
from extensions.data_resource959_v1 import authority as resource

BASE='benchmarks/compatibility/extensions/completed_unittest959_activation_v1';MANIFEST=BASE+'/manifest.json'
SOURCE={'commit':'656141bd7cb572ffbaebf981d6bd385e99c9f599','manifest':'benchmarks/compatibility/extensions/completed_unittest959_v1/manifest.json','git_blob':'5e5aac64ab74e357ac393cf3cec5042ed38d411c','sha256':'c66861317b95f4640e91d5e0134db308a7c3a955d30a59dd7d074b4cc87f28fa'}
TARGETS={'benchmarks/compatibility/'+p for p in ('scorecard.py','raw_control.py','extensions/successor959_v1/authority.py','extensions/data_resource959_v1/authority.py')}
def safe(api,p):return common.safe_path(api,p)
def fixed_pin(api):
 p=getattr(source_stage_pins,'REVISION_959_COMPLETED',None);api.require(isinstance(p,dict) and set(p)=={'commit','manifest','git_blob','sha256'} and p['manifest']==MANIFEST and api.HEX40.fullmatch(p['commit']) and api.HEX40.fullmatch(p['git_blob']) and api.HEX64.fullmatch(p['sha256']) and p['commit']!=SOURCE['commit'],'completed amendment requires a distinct public source pin');return p

def verify_stage(api,m,post=True):
 check=api.require;check(m.get('schema')=='imre.compatibility.completed-finite-amendment.v1' and m.get('credit')==0 and m.get('source_stage')==SOURCE,'completed amendment source or credit differs');raw_control.public_commit(api,SOURCE['commit']);source=raw_control.git_manifest(api.ROOT,SOURCE['commit'],SOURCE['manifest'],SOURCE['git_blob'],SOURCE['sha256']);check(source['credit']==0 and safe(api,SOURCE['manifest']).read_bytes()==raw_control.git(api.ROOT,'show','--no-textconv',SOURCE['commit']+':'+SOURCE['manifest']), 'completed public source manifest differs')
 for p,d in source['files'].items():check(raw_control.sha(safe(api,p).read_bytes())==d,'completed source evidence changed: '+p)
 ext=api.ROOT/BASE;files=m['files'];actual={p.relative_to(api.ROOT).as_posix() for p in ext.rglob('*') if p.is_file() and p not in {ext/'manifest.json',ext/'activation.json'}};check(actual==set(files) and not any(p.is_symlink() for p in ext.rglob('*')) and all(p.startswith(BASE+'/') and raw_control.sha(safe(api,p).read_bytes())==d for p,d in files.items()),'completed amendment file closure differs')
 patch=m['patch'];check(patch['path'] in files and files[patch['path']]==patch['sha256'],'completed amendment patch differs');stat=subprocess.check_output(['git','apply','--numstat',str(safe(api,patch['path']))],cwd=api.ROOT,text=True);observed=[x.split('\t',2)[2] for x in stat.splitlines()];items=patch['files'];check(len(observed)==len(set(observed))==len(items)==len(TARGETS) and set(observed)=={x['path'] for x in items}==TARGETS,'completed amendment patch target closure differs');changes={}
 for x in items:
  p=x['path'];old=raw_control.git(api.ROOT,'show','--no-textconv',SOURCE['commit']+':'+p);oid=raw_control.git(api.ROOT,'rev-parse',SOURCE['commit']+':'+p).decode().strip();check(x['old_git_blob']==oid and x['old_sha256']==raw_control.sha(old) and api.HEX64.fullmatch(x['post_sha256']) and raw_control.sha(safe(api,p).read_bytes())==(x['post_sha256'] if post else x['old_sha256']),'completed amendment source postimage differs: '+p);changes[p]=x
 for name,e in m['bindings'].items():
  check(set(e)=={'adapter','binding','receipt','staged_receipt'} and e['adapter']=='raw-control-v4' and e['binding'] in files and e['staged_receipt']==source['evidence']['receipt'] and e['receipt']==f'benchmarks/compatibility/revisions/{source["revision"]}/verified/completed-finite-v1/stdlib/{name}.jsonl','completed amendment binding differs');b=json.loads(safe(api,e['binding']).read_bytes());rows=[json.loads(x) for x in safe(api,e['staged_receipt']).read_bytes().splitlines()];keys=[(r['id'],r['occurrence']) for r in b['originals']];check(b['family']==name and b['source']==source['source'] and b['canonical_denominator']==api.families()[name]['originals'] and 0<len(keys)==b['observed_originals']<=b['canonical_denominator'] and b['other_originals_not_run']==b['canonical_denominator']-len(keys) and len(keys)==len(set(keys)) and keys==[(r['id'],r['occurrence']) for r in rows] and all(not r['paired'] and r['source']==b['source'] for r in rows),'completed canonical original subset differs')
  for r in b['originals']:api.canonical_original({'family':name,'id':r['id'],'occurrence':r['occurrence'],'source':b['source']},'stdlib')
  allowed={**source['files'],**files};check(all(a in allowed and s['outer_seal'] in allowed and allowed[a]==s['archive_sha256'] and allowed[s['outer_seal']]==s['outer_sha256'] for a,s in b['archives'].items()) and all(loc in allowed or loc.partition('!')[0] in b['archives'] and bool(loc.partition('!')[2]) for loc in b['roles'].values()),'completed artifact role closure differs')
 return m,source,changes

@lru_cache(maxsize=None)
def _stage(api,pin_json):
 p=json.loads(pin_json);raw_control.public_commit(api,p['commit']);m=raw_control.git_manifest(api.ROOT,p['commit'],p['manifest'],p['git_blob'],p['sha256']);api.require(raw_control.sha(safe(api,p['manifest']).read_bytes())==p['sha256'],'completed local manifest differs from public stage');return verify_stage(api,m)
def stage(api):return _stage(api,json.dumps(fixed_pin(api),sort_keys=True))
def expected(api,path,digest):
 _,_,changes=stage(api);x=changes.get(path);api.require(x is None or x['old_sha256']==digest,'completed amendment predecessor source conflicts');return x['post_sha256'] if x else digest

@lru_cache(maxsize=None)
def activated_entries(api):
 m,_,_=stage(api);a=json.loads(safe(api,BASE+'/activation.json').read_bytes());api.require(set(a)=={'schema','source_stage','families','receipt_hashes'} and a['schema']=='imre.compatibility.completed-finite-activation.v1' and a['source_stage']==fixed_pin(api) and a['families']==m['bindings'] and set(a['receipt_hashes'])=={e['receipt'] for e in a['families'].values()},'completed activation differs from public amendment')
 for e in a['families'].values():
  raw=safe(api,e['receipt']).read_bytes();api.require(raw_control.sha(raw)==a['receipt_hashes'][e['receipt']],'completed receipt hash differs');old=[json.loads(x) for x in safe(api,e['staged_receipt']).read_bytes().splitlines()];new=[json.loads(x) for x in raw.splitlines()];api.require(len(old)==len(new) and all({k:v for k,v in x.items() if k not in {'paired','credit_reason','runs'}}=={k:v for k,v in y.items() if k not in {'paired','credit_reason','runs'}} and all({k:v for k,v in x['runs'][t].items() if k not in {'argv','evidence'}}=={k:v for k,v in y['runs'][t].items() if k not in {'argv','evidence'}} for t in api.TIERS) for x,y in zip(old,new)),'completed activation altered a frozen original outcome or reason')
 return a
def effective_binding(api,e):return json.loads(safe(api,e['binding']).read_bytes())
def read(api,e,loc):
 m,source,_=stage(api);api.require(e in m['bindings'].values(),'completed entry was not staged');b=effective_binding(api,e);allowed=set(b['roles'].values())|{e['binding'],e['staged_receipt'],e['receipt']};api.require(loc in allowed,'unbound completed artifact');arc,sep,member=loc.partition('!')
 if sep:
  api.require(arc in b['archives'],'unknown completed archive');members=common.archive_members(api,arc,json.dumps(b['archives'][arc],sort_keys=True));api.require(member in members,'unbound completed member');return members[member]
 if loc==e['receipt']:digest=activated_entries(api)['receipt_hashes'][loc]
 else:digest={**source['files'],**m['files']}[loc]
 v=safe(api,loc).read_bytes();api.require(raw_control.sha(v)==digest,'completed artifact digest differs');return v
def digest(api,e,p):return raw_control.sha(read(api,e,p))
def artifact(api,e,p):v=read(api,e,p);file=safe(api,p);api.require(file.read_bytes()==v,'completed artifact differs');return file
predecessor_stock=common.predecessor_stock
def donor(api,product):
 _,s,_=stage(api);entries=resource.activated_entries(api)['families'];matches=[e for e in entries.values() if e['binding']==s['custody']['public_donor']];api.require(len(matches)==1,'completed source donor is not uniquely public');resource.donor(api,matches[0],product)
