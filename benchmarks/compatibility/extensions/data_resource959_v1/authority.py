"""Append-only, separately public authority for declarative finite resource cohorts."""
from functools import lru_cache
import json,subprocess
from pathlib import Path
import raw_control,source_stage_pins
from extensions.successor959_v1 import authority as prior

REVISION=prior.REVISION
BASE='benchmarks/compatibility/extensions/data_resource959_v1'
MANIFEST=BASE+'/manifest.json'
PREDECESSOR={'commit':'406127b2323d799789a1c2d33f5a573753f7e870','manifest':prior.MANIFEST,'git_blob':'3ff071ee3bc586d5d606fd5854ef7323bfaa647a','sha256':'36910bbf580e5aed8789f323a36eb6d097fe391782872b1cb5416c1ed074179c'}
ACTIVATION='cfe668cd5b12c5678d3f9b80edc47f20d65851bf'
TARGETS={'benchmarks/compatibility/'+x for x in ('scorecard.py','raw_control.py','extensions/successor959_v1/authority.py','test_revision959_stage.py')}

def safe(api,path):return prior.safe_path(api,path)
def fixed_pin(api):
 p=getattr(source_stage_pins,'REVISION_959_RESOURCE',None);api.require(isinstance(p,dict) and set(p)=={'commit','manifest','git_blob','sha256'} and p['manifest']==MANIFEST and api.HEX40.fullmatch(p['commit']) and api.HEX40.fullmatch(p['git_blob']) and api.HEX64.fullmatch(p['sha256']) and p['commit'] not in {PREDECESSOR['commit'],ACTIVATION},'resource stage requires a separate public pin');return p

def verify_stage(api,m,post=True):
 api.require(m.get('schema')=='imre.compatibility.finite-resource-stage.v1' and m.get('imre_revision')==REVISION and m.get('credit')==0 and m.get('predecessor')==PREDECESSOR and m.get('prior_activation')==ACTIVATION,'resource stage public predecessor or activation differs');raw_control.public_commit(api,PREDECESSOR['commit']);old=raw_control.git_manifest(api.ROOT,PREDECESSOR['commit'],PREDECESSOR['manifest'],PREDECESSOR['git_blob'],PREDECESSOR['sha256']);raw_control.public_commit(api,ACTIVATION)
 api.require(safe(api,prior.MANIFEST).read_bytes()==raw_control.git(api.ROOT,'show','--no-textconv',PREDECESSOR['commit']+':'+prior.MANIFEST),'public406 manifest changed');old_a=json.loads(raw_control.git(api.ROOT,'show','--no-textconv',ACTIVATION+':'+prior.BASE+'/activation.json'));base_a=json.loads(raw_control.git(api.ROOT,'show','--no-textconv',ACTIVATION+':benchmarks/compatibility/revisions/'+REVISION+'/activation.json'))
 for path in (prior.BASE+'/activation.json','benchmarks/compatibility/revisions/'+REVISION+'/activation.json',*[e['receipt'] for e in old_a['families'].values()],*[e['receipt'] for g in ('families','script_shards') for e in base_a[g].values()]):api.require(safe(api,path).read_bytes()==raw_control.git(api.ROOT,'show','--no-textconv',ACTIVATION+':'+path),'prior activation or receipt bytes changed: '+path)
 files=m.get('files');ext=api.ROOT/BASE;actual={p.relative_to(api.ROOT).as_posix() for p in ext.rglob('*') if p.is_file() and p not in {ext/'manifest.json',ext/'activation.json'}};api.require(isinstance(files,dict) and set(files)==actual and not any(p.is_symlink() for p in ext.rglob('*')) and all(loc.startswith(BASE+'/') and api.HEX64.fullmatch(d) and raw_control.sha(safe(api,loc).read_bytes())==d for loc,d in files.items()),'resource stage file closure differs')
 patch=m.get('patch');api.require(isinstance(patch,dict) and patch.get('path') in files and files[patch['path']]==patch.get('sha256'),'resource future patch differs');num=subprocess.check_output(['git','apply','--numstat','--unidiff-zero',str(safe(api,patch['path']))],cwd=api.ROOT,text=True);observed=[x.split('\t',2)[2] for x in num.splitlines()];items=patch.get('files',[]);api.require(len(observed)==len(set(observed))==len(items)==len(TARGETS) and set(observed)=={x['path'] for x in items}==TARGETS,'resource patch contains extra or missing paths');changes={}
 for x in items:
  path=x['path'];original=raw_control.git(api.ROOT,'show','--no-textconv',ACTIVATION+':'+path);blob=raw_control.git(api.ROOT,'rev-parse',ACTIVATION+':'+path).decode().strip();api.require(x['old_git_blob']==blob and x['old_sha256']==raw_control.sha(original) and api.HEX64.fullmatch(x['post_sha256']) and raw_control.sha(safe(api,path).read_bytes())==(x['post_sha256'] if post else x['old_sha256']),'resource source preimage/postimage differs: '+path);changes[path]=x
 entries=m.get('bindings');api.require(isinstance(entries,dict) and entries and all(set(e)=={'adapter','binding','receipt','staged_receipt'} and e['adapter']=='raw-control-v3' and e['binding'] in files and e['staged_receipt'] in files and e['receipt']==f'benchmarks/compatibility/revisions/{REVISION}/verified/data-resource-v1/stdlib/{name}.jsonl' for name,e in entries.items()),'invalid resource binding references')
 for name,e in entries.items():
  b=json.loads(safe(api,e['binding']).read_bytes());api.require(b.get('schema')=='imre.compatibility.successor-input.v1' and b.get('kind')=='stdlib' and b.get('family')==name and b.get('credit')==0 and api.families()[name]['originals']==b.get('canonical_denominator') and type(b.get('observed_originals')) is int and 0<b['observed_originals']==len(b.get('originals',[]))<=b['canonical_denominator'] and b.get('other_originals_not_run')==b['canonical_denominator']-b['observed_originals'] and isinstance(b.get('roles'),dict) and isinstance(b.get('archives'),dict), 'resource source, denominator or role binding differs');keys=[]
  for row in b['originals']:api.canonical_original({'family':name,'id':row['id'],'occurrence':row['occurrence'],'source':b['source']},'stdlib');keys.append((row['id'],row['occurrence']))
  rows=[json.loads(line) for line in safe(api,e['staged_receipt']).read_bytes().splitlines()];api.require([(r['id'],r['occurrence']) for r in rows]==keys and all(r.get('family')==name and r.get('source')==b['source'] and r.get('paired') is False and r.get('credit_reason')==b['identity_blocker'] for r in rows),'resource staged originals or zero credit differ')
  api.require(len(keys)==len(set(keys)) and all(row.get('launcher_cohort') in b['cohorts'] for row in b['originals']) and all(a in files and files[a]==cfg['archive_sha256'] and cfg['outer_seal'] in files and files[cfg['outer_seal']]==cfg['outer_sha256'] for a,cfg in b['archives'].items()) and all(isinstance(loc,str) and (loc in files or loc.partition('!')[0] in b['archives'] and loc.partition('!')[1]) for loc in b['roles'].values()),'resource role closure or occurrence mapping differs')
 return m,changes

@lru_cache(maxsize=None)
def _stage(api,pin_json):
 p=json.loads(pin_json);raw_control.public_commit(api,p['commit']);m=raw_control.git_manifest(api.ROOT,p['commit'],p['manifest'],p['git_blob'],p['sha256']);api.require(raw_control.sha(safe(api,p['manifest']).read_bytes())==p['sha256'],'local resource manifest differs from public stage');return verify_stage(api,m)
def stage(api):return _stage(api,json.dumps(fixed_pin(api),sort_keys=True))
def expected(api,path,digest):
 _,changes=stage(api);item=changes.get(path);api.require(item is None or item['old_sha256']==digest,'resource preimage conflicts with older authority');return item['post_sha256'] if item else digest

@lru_cache(maxsize=None)
def activated_entries(api):
 m,_=stage(api);a=json.loads(safe(api,BASE+'/activation.json').read_bytes());api.require(set(a)=={'schema','source_stage','families','receipt_hashes'} and a['schema']=='imre.compatibility.finite-resource-activation.v1' and a['source_stage']==fixed_pin(api) and a['families']==m['bindings'] and set(a['receipt_hashes'])=={e['receipt'] for e in a['families'].values()},'resource activation differs from separately public stage')
 for e in a['families'].values():
  x=safe(api,e['receipt']).read_bytes();api.require(raw_control.sha(x)==a['receipt_hashes'][e['receipt']],'resource receipt digest differs');old=[json.loads(z) for z in safe(api,e['staged_receipt']).read_bytes().splitlines()];new=[json.loads(z) for z in x.splitlines()];api.require(len(new)==len(old) and all({k:v for k,v in x.items() if k not in {'paired','credit_reason'}}=={k:v for k,v in y.items() if k not in {'paired','credit_reason'}} for x,y in zip(old,new)),'resource activation changed an observed original')
 return a
def entry_for(api,name):return activated_entries(api)['families'].get(name)
def effective_binding(api,e):return json.loads(safe(api,e['binding']).read_bytes())

@lru_cache(maxsize=None)
def _roles(api,entry_json):
 e=json.loads(entry_json);m,_=stage(api);api.require(e in m['bindings'].values(),'resource entry not staged');b=effective_binding(api,e);return b,set(b['roles'].values())|{e['binding'],e['staged_receipt'],e['receipt']}
def member_bytes(api,b,loc):
 arc,_,tail=loc.partition('!');member,sep,pointer=tail.partition('#');api.require(arc in b['archives'] and member,'resource archive locator differs');members=prior.archive_members(api,arc,json.dumps(b['archives'][arc],sort_keys=True));api.require(member in members,'resource member is not sealed');value=members[member]
 if sep:
  api.require(pointer.startswith('/') and '~' not in pointer,'invalid resource JSON projection');x=json.loads(value)
  for part in pointer[1:].split('/'):
   if isinstance(x,list):api.require(part.isdecimal() and str(int(part))==part and int(part)<len(x),'invalid resource array projection');x=x[int(part)]
   else:api.require(isinstance(x,dict) and part in x,'invalid resource object projection');x=x[part]
  value=json.dumps(x,sort_keys=True,separators=(',',':')).encode()
 return value
def digest(api,e,loc):
 b,allowed=_roles(api,json.dumps(e,sort_keys=True));api.require(loc in allowed,'cross-entry or unbound resource role')
 if loc==e['receipt']:return activated_entries(api)['receipt_hashes'][loc]
 if '!' in loc:return raw_control.sha(member_bytes(api,b,loc))
 m,_=stage(api);return m['files'][loc]
def read(api,e,loc):
 d=digest(api,e,loc);b,_=_roles(api,json.dumps(e,sort_keys=True));v=member_bytes(api,b,loc) if '!' in loc else safe(api,loc).read_bytes();api.require(raw_control.sha(v)==d,'resource artifact digest differs');return v
def artifact(api,e,loc):p=safe(api,loc);api.require(raw_control.sha(p.read_bytes())==digest(api,e,loc),'resource artifact differs');return p
def donor(api,e,product):
 transfer=json.loads(read(api,e,effective_binding(api,e)['roles']['transfer']));api.require(transfer['source_revision']==REVISION and {k:transfer[k+'_sha256'] for k in ('cli','runtime','clang')}==product,'resource transfer tuple differs');entries=prior.activated_entries(api)['families'];matches=[]
 for candidate in entries.values():
  b=prior.effective_binding(api,candidate);record=json.loads(prior.read(api,candidate,b['roles']['assessment']));
  if record.get('product')==product:matches.append(candidate)
 api.require(len(matches)==1,'resource donor tuple is not uniquely public');return prior.donor(api,matches[0],product)
predecessor_stock=prior.predecessor_stock
