"""Separately public, append-only successor authority; never trusts a local manifest."""
from functools import lru_cache
import json
import subprocess
import tarfile
from pathlib import Path

import raw_control
import source_stage_pins

REVISION = '95908fb0acadc2a67304a8080e584deaef395746'
PREDECESSOR = {'commit':'956047d40b9f74b05b005beb0b8bb179abe37818', 'manifest':'benchmarks/compatibility/revisions/95908fb0acadc2a67304a8080e584deaef395746/controls/unified-source-stage.json', 'git_blob':'936fdf2ded35588b6e3fcb5c22d9e234b12b5aab', 'sha256':'d885e272968c9e831bead7b80e1757c70fbe215d4e979a8851e7c658e824e5b2'}
ACTIVATION_COMMIT = '7c41f0b1456582d754a0182af2ecb823b314e2ed'
BASE = 'benchmarks/compatibility/extensions/successor959_v1'
MANIFEST = BASE + '/manifest.json'
STAGE_PREDECESSOR = {'commit':'f1788f4337076e2a9cb8a004d601303ec1088d55','manifest':MANIFEST,'git_blob':'e80b9bd77366bdf787b1039baffdc47655cb463b','sha256':'aac52bf31c07ef5758cc32b05b0962dc08b7e3b518198971770e9fc41376c47b'}


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


def approved_digest(api,path,digest):
    if getattr(source_stage_pins,'REVISION_959_RESOURCE',None) is None:return digest
    from extensions.data_resource959_v1 import authority as resource
    return resource.expected(api,path,digest)


def verify_stage(api, manifest):
    api.require(manifest.get('schema') == 'imre.compatibility.successor-stage.v1'
                and manifest.get('imre_revision') == REVISION and manifest.get('predecessor') == PREDECESSOR and source_stage_pins.REVISION_959 == PREDECESSOR,
                'immutable public956 predecessor differs')
    api.require(manifest.get('predecessor_stage')==STAGE_PREDECESSOR,'immutable f178 predecessor stage differs')
    raw_control.public_commit(api,STAGE_PREDECESSOR['commit']);prior_stage=raw_control.git_manifest(api.ROOT,STAGE_PREDECESSOR['commit'],MANIFEST,STAGE_PREDECESSOR['git_blob'],STAGE_PREDECESSOR['sha256'])
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
    api.require(set(files) == actual and all(loc.startswith(BASE+'/') and api.HEX64.fullmatch(digest) and raw_control.sha(safe_path(api,loc).read_bytes()) == approved_digest(api,loc,digest) for loc,digest in files.items()), 'successor shared map is incomplete, unbound or altered')
    amended={BASE+'/'+x for x in ('authority.py','check_stage.py','generate.py','patches/validators.patch')}
    api.require(set(prior_stage['files'])<=set(files) and all((loc in amended and raw_control.sha(raw_control.git(api.ROOT,'show','--no-textconv',STAGE_PREDECESSOR['commit']+':'+loc))==digest) or files[loc]==digest for loc,digest in prior_stage['files'].items()), 'f178 source/evidence bytes removed or changed outside the amendment')
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
        expected=approved_digest(api,path,changed.get(path,digest))
        api.require(raw_control.sha(safe_path(api,path).read_bytes())==expected, 'predecessor artifact or patched source digest differs: '+path)
    inputs=manifest.get('inputs');shared_roles=manifest.get('shared_roles');used=set();references=set()
    api.require(isinstance(inputs,dict) and inputs and len(set(inputs.values()))==len(inputs) and isinstance(shared_roles,dict), 'invalid successor input index')
    for key,loc in inputs.items():
        kind,sep,identity=key.partition('/')
        api.require(sep and identity and kind in bindings and loc in files, 'unbound or invalid successor input')
        delta=json.loads(safe_path(api,loc).read_bytes());base_loc=delta.get('base_binding');api.require(delta.get('schema')=='imre.compatibility.successor-identity-input.v1' and delta.get('kind')==kind and delta.get('family')==identity and prior_stage['inputs'].get(key)==base_loc and files.get(base_loc)==prior_stage['files'].get(base_loc),'identity input differs from public f178 binding')
        binding=json.loads(safe_path(api,base_loc).read_bytes());scope=binding.get('scope','');roles=binding.get('roles',{});private=set(roles.values());actual_scope={x for x in files if x.startswith(scope+'/')}
        api.require(kind=='stdlib' and identity in api.families() and binding.get('family')==identity and binding.get('originals') and binding.get('canonical_denominator')==api.families()[identity]['originals'], 'unknown canonical finite family or denominator')
        for item in binding['originals']:api.canonical_original({'family':identity,'id':item['id'],'occurrence':item['occurrence'],'source':binding['source']},kind)
        api.require(binding.get('kind')==kind and binding.get('schema')=='imre.compatibility.successor-input.v1' and scope.startswith(BASE+'/evidence/') and isinstance(roles,dict) and len(private)==len(roles) and private==actual_scope and not private & used and all(safe_path(api,x).is_relative_to((api.ROOT/scope).resolve()) for x in private) and len(binding['shared_roles'])==len(set(binding['shared_roles'])) and set(binding['shared_roles'])<=set(shared_roles), 'input role closure or collision differs')
        replacements=delta.get('replace_roles',{});extra=delta.get('identity_roles',{});members=list(replacements.values())+list(extra.values());api.require(isinstance(replacements,dict) and set(replacements)<=set(roles) and isinstance(extra,dict) and not set(extra)&set(roles) and len(members)==len(set(members)) and isinstance(delta.get('archives'),dict) and all(a in files and cfg.get('outer_seal') in files for a,cfg in delta['archives'].items()) and all(isinstance(x,str) and x.partition('!')[0] in delta['archives'] and x.partition('!')[1] for x in members),'identity role closure or collision differs')
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


def effective_binding(api,entry):
    delta=json.loads(safe_path(api,entry['binding']).read_bytes());base=json.loads(safe_path(api,delta['base_binding']).read_bytes());roles={**base['roles'],**delta['replace_roles'],**delta['identity_roles']};return {**base,'roles':roles,'base_roles':base['roles'],'identity':delta,'identity_blocker':delta['credit_reason'],'cohorts':{k:{**v,'identity':True} for k,v in base['cohorts'].items()}}


@lru_cache(maxsize=None)
def archive_members(api,archive,config_json):
    """Resolve immutable safe archive members under a declared versioned seal/manifest format."""
    cfg=json.loads(config_json);raw=safe_path(api,archive).read_bytes();outer=safe_path(api,cfg['outer_seal']).read_bytes();api.require(raw_control.sha(outer)==cfg['outer_sha256'],'identity outer seal differs');obj=json.loads(outer);outer_format=cfg.get('outer_format','single-archive-v1');manifest_format=cfg.get('manifest_format','files-map-v1')
    if outer_format=='single-archive-v1':api.require(obj.get('credit')==0 and obj.get('files')==[{'name':Path(archive).name,'bytes':len(raw),'sha256':raw_control.sha(raw)}], 'identity archive differs from outer seal')
    elif outer_format=='archive-fields-v1':api.require(obj.get('credit')==0 and Path(obj.get('archive_path','')).name==Path(archive).name and obj.get('archive_bytes')==len(raw) and obj.get('archive_sha256')==raw_control.sha(raw) and obj.get('member_list_sha256')==cfg['manifest_sha256'], 'identity archive fields differ from outer seal')
    elif outer_format == 'external-files-map-v1':
        api.require(
            obj.get('credit') == 0 and obj.get('archive_bytes') == len(raw)
            and obj.get('archive_sha256') == raw_control.sha(raw) == cfg['archive_sha256']
            and isinstance(obj.get('files'), dict),
            'identity externally sealed archive differs',
        )
    else:api.require(outer_format=='member-manifest-v1' and obj.get('credit')==0 and cfg.get('archive_sha256')==raw_control.sha(raw), 'identity archive or manifest-only seal differs')
    with tarfile.open(safe_path(api,archive)) as t:
        result={}
        for x in t.getmembers():
            if x.name=='.' and x.isdir():continue
            name=x.name.removeprefix('./');api.require(x.isreg() and name and name not in result and not Path(name).is_absolute() and '..' not in Path(name).parts and Path(name).as_posix()==name and x.name in {name,'./'+name},'unsafe, duplicate or nonregular identity archive member');result[name]=t.extractfile(x).read()
    if outer_format == 'external-files-map-v1':
        member = None
        m = obj
        extra = set()
    else:
        member = cfg['manifest_member']
        api.require(
            member in result and raw_control.sha(result[member]) == cfg['manifest_sha256'],
            'identity member manifest differs',
        )
        m = json.loads(result[member])
        extra = {member} | ({cfg['additional_seal']} if cfg.get('additional_seal') else set())
    if manifest_format=='files-map-v1':files=m.get('files');api.require(isinstance(files,dict) and m.get('schema')==cfg['manifest_schema'],'identity files-map manifest schema differs')
    else:
        api.require(manifest_format=='members-list-v1' and isinstance(m.get('members'),list),'unknown identity member manifest format');items=m['members'];api.require(all(isinstance(v,dict) and set(v)=={'path','bytes','sha256'} and isinstance(v['path'],str) for v in items) and len({v['path'] for v in items})==len(items),'duplicate or malformed identity list member');files={v['path']:{'bytes':v['bytes'],'sha256':v['sha256']} for v in items}
    api.require(m.get('credit')==0 and len(files)==cfg['members'] and set(result)==set(files)|extra and all(result.get(k) is not None and v=={'bytes':len(result[k]),'sha256':raw_control.sha(result[k])} for k,v in files.items()), 'identity archive contains unbound or altered members')
    if outer_format=='member-manifest-v1':api.require(outer==result[member], 'outer and archived member manifests differ')
    if outer_format=='archive-fields-v1':api.require(obj.get('member_count')==len(result),'identity archive member count differs')
    if cfg.get('additional_seal'):api.require(json.loads(result[cfg['additional_seal']]).get('manifest_sha256')==cfg['manifest_sha256'],'identity inner seal differs')
    return result


def digest(api, entry, locator):
    manifest,_,shared=authority(api);api.require(any(e==entry for group in manifest['bindings'].values() for e in group.values()), 'entry is not exactly staged');delta=json.loads(safe_path(api,entry['binding']).read_bytes());base=json.loads(safe_path(api,delta['base_binding']).read_bytes());allowed=set(base['roles'].values())|{manifest['shared_roles'][key] for key in base['shared_roles']}|{entry['binding'],entry['staged_receipt'],delta['base_binding'],*delta['archives'],*(x['outer_seal'] for x in delta['archives'].values())}|set(delta['replace_roles'].values())|set(delta['identity_roles'].values())
    if locator==entry['receipt']:return activated_entries(api)['receipt_hashes'][locator]
    api.require(locator in allowed,'cross-entry or unbound role')
    if '!' in locator:
        arc,member=locator.split('!',1);api.require(arc in delta['archives'],'archive role rebound');members=archive_members(api,arc,json.dumps(delta['archives'][arc],sort_keys=True));api.require(member in members,'unbound archive member');return raw_control.sha(members[member])
    api.require(locator in shared,'unbound shared-map artifact');return shared[locator]


def read(api,entry,locator):
    expected=digest(api,entry,locator)
    if '!' in locator:
        delta=json.loads(safe_path(api,entry['binding']).read_bytes());arc,member=locator.split('!',1);raw=archive_members(api,arc,json.dumps(delta['archives'][arc],sort_keys=True))[member]
    else:raw=safe_path(api,locator).read_bytes()
    api.require(raw_control.sha(raw)==expected,'entry artifact digest differs');return raw


def artifact(api, entry, locator):
    path=safe_path(api,locator);api.require(raw_control.sha(path.read_bytes())==digest(api,entry,locator), 'entry artifact digest differs')
    return path


def donor(api, entry, product):
    manifest,_,_=authority(api);binding=effective_binding(api,entry);previous=raw_control.git_manifest(api.ROOT,PREDECESSOR['commit'],PREDECESSOR['manifest'],PREDECESSOR['git_blob'],PREDECESSOR['sha256']);name=binding['donor_tuple'];record=previous['build_tuples'].get(name)
    api.require(record is not None and product=={'cli':record['cli_sha256'],'runtime':record['runtime_sha256'],'clang':record['clang_sha256']}, 'cross-tuple or unaudited donor product')
    roles={key:manifest['shared_roles'][key] for key in binding['shared_roles'] if key.startswith('donor_')};data=lambda name:artifact(api,entry,roles['donor_'+name]).read_bytes()
    setup=json.loads(data('setup'));actual=raw_control.custody_tuple(api,setup,data('log').decode(),{'imre.d':data('cli_dep'),'libimre_runtime.d':data('runtime_dep')},data('generated'),api.require)
    transfer=json.loads(data('transfer'));api.require(actual=={'imre':product['cli'],'libimre_runtime.a':product['runtime']} and transfer['cli_sha256']==product['cli'] and transfer['runtime_sha256']==product['runtime'] and transfer['clang_sha256']==product['clang'], 'donor custody/transfer tuple differs')
    return actual


@lru_cache(maxsize=None)
def predecessor_stock(api):
    """Read interpreter authority from the independently public predecessor source."""
    path='benchmarks/compatibility/script_control.py';raw=raw_control.git(api.ROOT,'show','--no-textconv',PREDECESSOR['commit']+':'+path);previous=raw_control.git_manifest(api.ROOT,PREDECESSOR['commit'],PREDECESSOR['manifest'],PREDECESSOR['git_blob'],PREDECESSOR['sha256']);api.require(raw_control.sha(raw)==previous['sources'][path], 'predecessor stock authority source differs');nodes=[n for n in api.ast.parse(raw).body if isinstance(n,api.ast.Assign) and len(n.targets)==1 and isinstance(n.targets[0],api.ast.Name) and n.targets[0].id=='STOCK_PYTHON_SHA256'];api.require(len(nodes)==1,'predecessor stock identity is ambiguous');digest=api.ast.literal_eval(nodes[0].value);cohort=json.loads(raw_control.git(api.ROOT,'show','--no-textconv',PREDECESSOR['commit']+':benchmarks/compatibility/cohort.json'));api.require(api.HEX64.fullmatch(digest) and cohort['cpython_version']==api.COHORT['cpython_version'], 'predecessor stock version differs');return digest,'Python '+cohort['cpython_version']
