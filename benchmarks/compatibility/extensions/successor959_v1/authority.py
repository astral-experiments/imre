"""Separately public, append-only successor authority; never trusts a local manifest."""
from functools import lru_cache
import json
import subprocess
from pathlib import Path

import raw_control
import source_stage_pins

REVISION = '95908fb0acadc2a67304a8080e584deaef395746'
PREDECESSOR = {'commit':'956047d40b9f74b05b005beb0b8bb179abe37818', 'manifest':'benchmarks/compatibility/revisions/95908fb0acadc2a67304a8080e584deaef395746/controls/unified-source-stage.json', 'git_blob':'936fdf2ded35588b6e3fcb5c22d9e234b12b5aab', 'sha256':'d885e272968c9e831bead7b80e1757c70fbe215d4e979a8851e7c658e824e5b2'}
ACTIVATION_COMMIT = '7c41f0b1456582d754a0182af2ecb823b314e2ed'
BASE = 'benchmarks/compatibility/extensions/successor959_v1'
MANIFEST = BASE + '/manifest.json'


def fixed_pin(api):
    pin = getattr(source_stage_pins, 'REVISION_959_SUCCESSOR', None)
    api.require(isinstance(pin, dict) and set(pin) == {'commit','manifest','git_blob','sha256'}
                and pin['manifest'] == MANIFEST and api.HEX40.fullmatch(pin['commit'])
                and api.HEX40.fullmatch(pin['git_blob']) and api.HEX64.fullmatch(pin['sha256'])
                and pin['commit'] not in {PREDECESSOR['commit'], ACTIVATION_COMMIT},
                'successor requires a separately reviewed public commit/blob/digest')
    return pin


def safe_path(api, value):
    api.require(isinstance(value,str) and value and not Path(value).is_absolute()
                and '..' not in Path(value).parts and Path(value).as_posix() == value,
                'unconfined successor path')
    lexical=api.ROOT/value
    api.require(not any(part.is_symlink() for part in (lexical,*lexical.parents) if part.is_relative_to(api.ROOT)), 'successor symlink path')
    return api.repository_file(value, api.ROOT)


def verify_stage(api, manifest):
    api.require(manifest.get('schema') == 'imre.compatibility.successor-stage.v1'
                and manifest.get('imre_revision') == REVISION and manifest.get('predecessor') == PREDECESSOR and source_stage_pins.REVISION_959 == PREDECESSOR,
                'immutable public956 predecessor differs')
    raw_control.public_commit(api, PREDECESSOR['commit'])
    previous = raw_control.git_manifest(api.ROOT, PREDECESSOR['commit'], PREDECESSOR['manifest'], PREDECESSOR['git_blob'], PREDECESSOR['sha256'])
    prior = manifest.get('prior_activation', {})
    api.require(prior.get('commit') == ACTIVATION_COMMIT and prior.get('path') == f'benchmarks/compatibility/revisions/{REVISION}/activation.json', 'public7c41 activation differs')
    raw_control.public_commit(api, ACTIVATION_COMMIT)
    activation = raw_control.git_manifest(api.ROOT, ACTIVATION_COMMIT, prior['path'], prior['git_blob'], prior['sha256'])
    api.require(safe_path(api,prior['path']).read_bytes() == raw_control.git(api.ROOT,'show','--no-textconv',ACTIVATION_COMMIT+':'+prior['path']), 'live predecessor activation changed')
    files = manifest.get('files'); patches = manifest.get('patches'); bindings = manifest.get('bindings')
    api.require(isinstance(files,dict) and isinstance(patches,list) and isinstance(bindings,dict) and set(bindings)=={'stdlib'}, 'invalid successor shared map or bindings')
    ext=(api.ROOT/BASE).resolve();api.require(ext.is_dir() and ext.is_relative_to(api.ROOT.resolve()), "successor extension directory escapes repository")
    api.require(not any(p.is_symlink() for p in ext.rglob('*')), 'unbound successor symlink')
    actual = {p.relative_to(api.ROOT).as_posix() for p in ext.rglob("*") if p.is_file() and p not in {ext/"manifest.json",ext/"activation.json"}}
    api.require(set(files) == actual and all(loc.startswith(BASE+'/') and api.HEX64.fullmatch(digest) and raw_control.sha(safe_path(api,loc).read_bytes()) == digest for loc,digest in files.items()), 'successor shared map is incomplete, unbound or altered')
    changed = {};target_set={f'benchmarks/compatibility/{name}' for name in ('scorecard.py','raw_control.py','script_control.py','test_revision959_stage.py')}
    for patch in patches:
        api.require(patch.get('path') in files and files[patch['path']] == patch.get('sha256'), 'patch digest differs')
        declared=[x['path'] for x in patch.get('files',[])];numstat=subprocess.check_output(['git','apply','--unidiff-zero','--numstat',str(safe_path(api,patch['path']))],cwd=api.ROOT,text=True);observed=[line.split('\t',2)[2] for line in numstat.splitlines()]
        api.require(len(declared)==len(set(declared))==len(observed)==len(set(observed)) and set(declared)==set(observed)==target_set, 'patch has extra/missing/unapproved paths')
        for item in patch.get('files',[]):
            path=item['path']; original=raw_control.git(api.ROOT,'show','--no-textconv',PREDECESSOR['commit']+':'+path)
            oid=raw_control.git(api.ROOT,'rev-parse',PREDECESSOR['commit']+':'+path).decode().strip()
            api.require(path not in changed and previous['sources'].get(path)==item['old_sha256']==raw_control.sha(original) and item['old_git_blob']==oid and api.HEX64.fullmatch(item['post_sha256']), 'patch old public blob/source or successor hash differs')
            changed[path]=item['post_sha256']
    shared={**previous['sources'],**previous['artifacts'],**{v['path']:v['sha256'] for group in previous['staged'].values() for v in group.values()}}
    for group in ('families','script_shards'):
        for entry in activation[group].values():
            receipt=entry['receipt'];public=raw_control.git(api.ROOT,'show','--no-textconv',ACTIVATION_COMMIT+':'+receipt);live=safe_path(api,receipt).read_bytes()
            api.require(live==public and {json.loads(x)['id'] for x in live.splitlines() if json.loads(x).get('paired')}=={json.loads(x)['id'] for x in public.splitlines() if json.loads(x).get('paired')}, 'predecessor receipt bytes or paired IDs differ')
            for path,digest in entry['files'].items():
                api.require(path not in shared or shared[path]==digest, 'predecessor digest conflict');shared[path]=digest
    for path,digest in shared.items():
        expected=changed.get(path,digest)
        api.require(raw_control.sha(safe_path(api,path).read_bytes())==expected, 'predecessor artifact or patched source digest differs: '+path)
    inputs=manifest.get('inputs');shared_roles=manifest.get('shared_roles');used=set();references=set()
    api.require(isinstance(inputs,dict) and inputs and len(set(inputs.values()))==len(inputs) and isinstance(shared_roles,dict), 'invalid successor input index')
    for key,loc in inputs.items():
        kind,sep,identity=key.partition('/')
        api.require(sep and identity and kind in bindings and loc in files, 'unbound or invalid successor input')
        binding=json.loads(safe_path(api,loc).read_bytes());scope=binding.get('scope','');roles=binding.get('roles',{});private=set(roles.values());actual_scope={x for x in files if x.startswith(scope+'/')}
        api.require(kind=='stdlib' and identity in api.families() and binding.get('family')==identity and binding.get('originals') and binding.get('canonical_denominator')==api.families()[identity]['originals'], 'unknown canonical finite family or denominator')
        for item in binding['originals']:api.canonical_original({'family':identity,'id':item['id'],'occurrence':item['occurrence'],'source':binding['source']},kind)
        api.require(binding.get('kind')==kind and binding.get('schema')=='imre.compatibility.successor-input.v1' and scope.startswith(BASE+'/evidence/') and isinstance(roles,dict) and len(private)==len(roles) and private==actual_scope and not private & used and all(safe_path(api,x).is_relative_to((api.ROOT/scope).resolve()) for x in private) and len(binding['shared_roles'])==len(set(binding['shared_roles'])) and set(binding['shared_roles'])<=set(shared_roles), 'input role closure or collision differs')
        used |= private
    api.require(len(set(shared_roles.values()))==len(shared_roles) and all(x in files or x in shared for x in shared_roles.values()) and not set(shared_roles.values()) & used, 'shared role collision or unbound path')
    for kind,entries in bindings.items():
        api.require(isinstance(entries,dict), 'invalid successor binding group')
        for identity,entry in entries.items():
            key=kind+'/'+identity;loc=inputs.get(key)
            api.require(key not in references and set(entry)=={'adapter','receipt','staged_receipt','binding'} and entry['adapter']=='raw-control-v2' and entry['binding']==loc and entry['staged_receipt'] in files and entry['receipt']==f'benchmarks/compatibility/revisions/{REVISION}/verified/successor-v1/stdlib/{identity}.jsonl', 'minimal successor receipt/binding refs differ');references.add(key)
    api.require(references==set(inputs), 'unbound input or extra binding reference')
    return manifest, activation, {**shared,**files}


@lru_cache(maxsize=None)
def _authority(api, pin_json):
    pin=json.loads(pin_json);raw_control.public_commit(api,pin['commit'])
    manifest=raw_control.git_manifest(api.ROOT,pin['commit'],pin['manifest'],pin['git_blob'],pin['sha256'])
    api.require(raw_control.sha(safe_path(api,pin['manifest']).read_bytes())==pin['sha256'], 'local successor manifest differs from public object')
    return verify_stage(api,manifest)


def authority(api):
    return _authority(api,json.dumps(fixed_pin(api),sort_keys=True))


def legacy_file(api, path, digest):
    manifest,_,shared=authority(api)
    api.require(shared.get(path)==digest, 'predecessor file not bound by successor authority')


@lru_cache(maxsize=None)
def activated_entries(api):
    manifest,_,_=authority(api);obj=json.loads(safe_path(api,BASE+'/activation.json').read_bytes())
    api.require(set(obj)=={'schema','source_stage','families','receipt_hashes'} and obj['schema']=='imre.compatibility.successor-activation.v1' and obj['source_stage']==fixed_pin(api) and obj['families']==manifest['bindings']['stdlib'], 'invalid top-level successor activation')
    entries=list(obj['families'].values());api.require(set(obj['receipt_hashes'])=={e['receipt'] for e in entries}, 'successor receipt hash inventory differs')
    for e in entries:
        receipt=safe_path(api,e['receipt']);candidate=safe_path(api,e['staged_receipt']);api.require(raw_control.sha(receipt.read_bytes())==obj['receipt_hashes'][e['receipt']], 'activated receipt hash differs')
        original=[json.loads(x) for x in candidate.read_bytes().splitlines()];actual=[json.loads(x) for x in receipt.read_bytes().splitlines()]
        api.require(len(actual)==len(original) and all({k:v for k,v in a.items() if k not in {'paired','credit_reason'}}=={k:v for k,v in b.items() if k not in {'paired','credit_reason'}} for a,b in zip(actual,original)), 'successor receipt changed an observed original')
    return obj


def overlay(api, original):
    _,prior,_=authority(api);api.require(original==prior, 'predecessor activation object differs');activated_entries(api)
    return original


def entry_for(api, name):
    return activated_entries(api)['families'].get(name)


def digest(api, entry, locator):
    manifest,_,shared=authority(api)
    api.require(any(e==entry for group in manifest['bindings'].values() for e in group.values()), 'entry is not exactly staged')
    binding=json.loads(safe_path(api,entry['binding']).read_bytes());allowed=set(binding['roles'].values())|{manifest['shared_roles'][key] for key in binding['shared_roles']}|{entry['binding'],entry['staged_receipt']}
    if locator==entry['receipt']:return activated_entries(api)['receipt_hashes'][locator]
    api.require(locator in allowed and locator in shared, 'cross-entry or unrelated shared-map artifact')
    return shared[locator]


def artifact(api, entry, locator):
    path=safe_path(api,locator);api.require(raw_control.sha(path.read_bytes())==digest(api,entry,locator), 'entry artifact digest differs')
    return path


def donor(api, entry, product):
    manifest,_,_=authority(api);binding=json.loads(artifact(api,entry,entry['binding']).read_bytes());previous=raw_control.git_manifest(api.ROOT,PREDECESSOR['commit'],PREDECESSOR['manifest'],PREDECESSOR['git_blob'],PREDECESSOR['sha256']);name=binding['donor_tuple'];record=previous['build_tuples'].get(name)
    api.require(record is not None and product=={'cli':record['cli_sha256'],'runtime':record['runtime_sha256'],'clang':record['clang_sha256']}, 'cross-tuple or unaudited donor product')
    roles={key:manifest['shared_roles'][key] for key in binding['shared_roles'] if key.startswith('donor_')};data=lambda name:artifact(api,entry,roles['donor_'+name]).read_bytes()
    setup=json.loads(data('setup'));actual=raw_control.custody_tuple(api,setup,data('log').decode(),{'imre.d':data('cli_dep'),'libimre_runtime.d':data('runtime_dep')},data('generated'),api.require)
    transfer=json.loads(data('transfer'));api.require(actual=={'imre':product['cli'],'libimre_runtime.a':product['runtime']} and transfer['cli_sha256']==product['cli'] and transfer['runtime_sha256']==product['runtime'] and transfer['clang_sha256']==product['clang'], 'donor custody/transfer tuple differs')
    return actual


@lru_cache(maxsize=None)
def predecessor_stock(api):
    """Read interpreter authority from the independently public predecessor source."""
    path='benchmarks/compatibility/script_control.py';raw=raw_control.git(api.ROOT,'show','--no-textconv',PREDECESSOR['commit']+':'+path);previous=raw_control.git_manifest(api.ROOT,PREDECESSOR['commit'],PREDECESSOR['manifest'],PREDECESSOR['git_blob'],PREDECESSOR['sha256']);api.require(raw_control.sha(raw)==previous['sources'][path], 'predecessor stock authority source differs');nodes=[n for n in api.ast.parse(raw).body if isinstance(n,api.ast.Assign) and len(n.targets)==1 and isinstance(n.targets[0],api.ast.Name) and n.targets[0].id=='STOCK_PYTHON_SHA256'];api.require(len(nodes)==1,'predecessor stock identity is ambiguous');digest=api.ast.literal_eval(nodes[0].value);cohort=json.loads(raw_control.git(api.ROOT,'show','--no-textconv',PREDECESSOR['commit']+':benchmarks/compatibility/cohort.json'));api.require(api.HEX64.fullmatch(digest) and cohort['cpython_version']==api.COHORT['cpython_version'], 'predecessor stock version differs');return digest,'Python '+cohort['cpython_version']
