"""Offline source/evidence validator. Does not execute any workload or compiler."""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import re
import zipfile

ROOT=Path(__file__).resolve().parent

def require(ok, message):
    if not ok: raise ValueError(message)

def sha(b): return hashlib.sha256(b).hexdigest()
def blob(b): return hashlib.sha1(f"blob {len(b)}\0".encode()+b).hexdigest()
def load(p): return json.loads(p.read_bytes())
def safe(base, relative):
    p=(base/relative).resolve(); require(p.is_relative_to(base.resolve()),f"unsafe path {relative}"); return p

def schema_check(v,s,root,path='$'):
    if '$ref' in s:
        node=root
        for k in s['$ref'].removeprefix('#/').split('/'):node=node[k]
        return schema_check(v,node,root,path)
    if 'const' in s:require(v==s['const'],f'{path}: const')
    typ=s.get('type'); types={'object':dict,'array':list,'string':str,'integer':int}
    if typ:require(isinstance(v,types[typ]) and not (typ=='integer' and isinstance(v,bool)),f'{path}: {typ}')
    if isinstance(v,dict):
        require(all(k in v for k in s.get('required',[])),f'{path}: missing property')
        require(len(v)>=s.get('minProperties',0),f'{path}: minProperties'); props=s.get('properties',{});extra=s.get('additionalProperties',True)
        for k,x in v.items():
            if k in props:schema_check(x,props[k],root,path+'.'+k)
            elif extra is False:raise ValueError(f'{path}: extra {k}')
            elif isinstance(extra,dict):schema_check(x,extra,root,path+'.'+k)
    if isinstance(v,list):
        require(len(v)>=s.get('minItems',0) and len(v)<=s.get('maxItems',10**9),f'{path}: array length')
        for i,x in enumerate(v):schema_check(x,s.get('items',{}),root,f'{path}[{i}]')
    if isinstance(v,str) and 'pattern' in s:require(re.search(s['pattern'],v) is not None,f'{path}: pattern')
    if isinstance(v,int) and 'minimum' in s:require(v>=s['minimum'],f'{path}: minimum')

def normalize(value,roles):
    if isinstance(value,str):
        for role,actual in sorted(roles.items(),key=lambda kv:-len(kv[1])):value=value.replace(actual,role)
        return value
    if isinstance(value,list):return [normalize(x,roles) for x in value]
    if isinstance(value,dict):return {k:normalize(x,roles) for k,x in value.items()}
    return value

def normalized_effects(effects,roles):
    result=normalize(effects,roles)
    journal="".join(json.dumps(p,sort_keys=True)+"\n" for p in result['processes']).encode()
    for item in result['files']['created']:
        if item['path']=='cargo.log':item.update(bytes=len(journal),sha256=sha(journal))
    return result

def check_fact(p,f):
    b=p.read_bytes();require(len(b)==f['bytes'] and sha(b)==f['sha256'],f'bad fact: {p}')

def main():
    schema=load(ROOT/'reference.schema.json');sources=load(ROOT/'sources.json');deps=load(ROOT/'dependencies.json');members=load(ROOT/'fixtures/rich-members.json')
    require(set(sources)=={'ruff-publisher','uv-publisher','uv-rich','ty-readme'},'unexpected corpus inventory')
    for case,s in sources.items():
        b=safe(ROOT,s['vendored']).read_bytes();require(len(b)==s['bytes'] and sha(b)==s['sha256'] and blob(b)==s['git_blob'],f'{case}: source object')
    with zipfile.ZipFile(ROOT/'fixtures/rich-imports.zip') as z:
        require(z.namelist()==[m['path'] for m in members['members']],'Rich member inventory')
        for m in members['members']:
            b=z.read(m['path']);require(len(b)==m['bytes'] and sha(b)==m['sha256'] and blob(b)==m['git_blob'],f"Rich member {m['path']}")
    require(deps['wheel_bytes_downloaded'] is False and deps['actual_stock_input']=='fixtures/rich-imports.zip','dependency source scope')
    for d in deps['packages']:
        require(re.fullmatch('[0-9a-f]{64}',d['wheel_sha256']) and re.fullmatch('[0-9a-f]{40}',d['lock_provenance']['git_blob']) and d['wheel_bytes']>0,'wheel provenance')
    for case,source in sources.items():
        base=ROOT/'references'/case;ref=load(base/'reference.json');schema_check(ref,schema,schema);require(ref['case']==case and ref['source']==source,'case/source binding')
        raw=base/'raw';expected={f'raw/{p.name}' for p in raw.iterdir()}|{'effects.normalized.json'};require(set(ref['evidence'])==expected,'evidence inventory')
        for name,f in ref['evidence'].items():require(name==f['path'],'fact path');check_fact(safe(base,name),f)
        inv=load(raw/'invocation.json');require(normalize(inv['invocation'],inv['roles'])==ref['invocation'],'invocation normalization')
        require(ref['invocation']['argv'][0]=='$PYTHON' and ref['invocation']['cwd']=='$WORK','invocation scope')
        allowed={'HOME','LANG','LC_ALL','PATH','PYTHONDONTWRITEBYTECODE','PYTHONHASHSEED','PYTHONNOUSERSITE','TZ','CARGO_LOG','NO_PROXY','PYTHONPATH','TERM','NO_COLOR','COLUMNS'}
        require(set(inv['invocation']['env'])<=allowed,'unexpected ambient environment key')
        require(load(raw/'wait.json')=={'exit_code':ref['observation']['exit_code'],'timed_out':False},'wait binding')
        for f in ref['fixture']['external_inputs']:
            if f['path']=='$PYTHON':require(f['sha256']==ref['tool']['executable_sha256'],'Python tool identity')
            else:check_fact(safe(ROOT,f['path'].removeprefix('$CORPUS/')),f)
        before=load(raw/'before.json');after=load(raw/'after.json');orig={x['path']:x for x in before};final={x['path']:x for x in after}
        require(len(orig)==len(before) and len(final)==len(after),'duplicate fixture path')
        inputs=ref['fixture'].get('project_inputs',[])
        if case=='ty-readme':require(inputs==load(ROOT/'ty-inputs.json') and len(inputs)==2,'ty project input catalog')
        else:require(not inputs,'unexpected project inputs')
        require(set(orig)=={'home','scripts',source['path']}|{x['path'] for x in inputs},'complete initial work tree')
        for item in inputs:
            b=safe(ROOT,item['vendored']).read_bytes();require(item['repo']==source['repo'] and item['commit']==source['commit'] and len(b)==item['bytes'] and sha(b)==item['sha256'] and blob(b)==item['git_blob'],'project input object')
            require(orig[item['path']]['sha256']==item['sha256'] and orig[item['path']]['bytes']==item['bytes'],'project input preimage')
        require(orig[source['path']]['sha256']==source['sha256'] and orig[source['path']]['bytes']==source['bytes'],'source preimage')
        changes={'created':[x for p,x in final.items() if p not in orig],'deleted':[x for p,x in orig.items() if p not in final],'modified':[x for p,x in final.items() if p in orig and x!=orig[p]]}
        events=[json.loads(l) for l in (raw/'registry.jsonl').read_bytes().splitlines()];procs=[json.loads(l) for l in (raw/'processes.jsonl').read_bytes().splitlines()];effects={'registry':events,'processes':procs,'files':changes}
        require(normalized_effects(effects,inv['roles'])==load(base/'effects.normalized.json'),'effects normalization')
        if case.endswith('publisher'):
            require(ref['invocation']['argv'][3:]==['--api-url','$REGISTRY_ORIGIN/api/v1','--cargo','$PYTHON','$CORPUS/fixtures/cargo_stub.py','--no-verify'],'publisher must use closed fixture tools')
            require(all(p['env']==inv['invocation']['env'] for p in procs),'subprocess environment binding')
            require([e['response']['status'] for e in events]==[200,404] and [p['argv'][0] for p in procs]==['metadata','publish'],'closed publisher fixture')
            require(set(final)-set(orig)=={'cargo.log','published.txt'},'publisher write set')
            require(sha((raw/'processes.jsonl').read_bytes())==final['cargo.log']['sha256'],'process journal effect')
            require(sha((raw/'published.txt').read_bytes())==final['published.txt']['sha256'],'publish marker effect')
        elif case=='ty-readme':
            require(not events and not procs and not changes['created'] and not changes['deleted'] and [x['path'] for x in changes['modified']]==['README.md'],'ty write set')
            require(sha((raw/'README.md').read_bytes())==final['README.md']['sha256'],'ty output effect')
            require((raw/'stdout').read_bytes()==b'','ty unexpectedly wrote stdout')
        else:require(not events and not procs and not any(changes.values()),'Rich unexpectedly had effects')
        require(ref['observation']['exit_code']==0 and (raw/'stderr').read_bytes()==b'','stock failure')
    print(f"validated {len(sources)} supplemental stock references, {len(members['members'])} Rich source members; no workload executed")
if __name__=='__main__':main()
