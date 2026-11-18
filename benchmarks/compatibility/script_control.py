"""Revision-pinned script adapter for the existing compatibility scorecard.

A source-stage draft cannot authorize itself. A later reviewed source change must
pin the independently public squash and its manifest before any paired row can
be accepted.
"""
from __future__ import annotations

import raw_control
import hashlib
import json
import source_stage_pins

PIN = '95908fb0acadc2a67304a8080e584deaef395746'
ORIGINAL_AUTHORITY = '880648b620f57905a495eb51fbb4600755ee0c54'
MANIFEST = f'benchmarks/compatibility/revisions/{PIN}/controls/unified-source-stage.json'


PREDECESSOR = {'commit':'7c790c02b96b422b2873fc7e05d3c601ec5ba999', 'manifest':MANIFEST,
               'git_blob':'aac61fb78e490989b9a81b2124dff6342d5a1969',
               'sha256':'8b69644e5cb93b0749accb06cce27457ba4cc361fb30d71d341ad631d878c5b6'}


def verify_predecessor(api, manifest):
    """Require the exact public first stage and preserve every frozen evidence object."""
    api.require(manifest.get('predecessor') == PREDECESSOR, '959 predecessor differs from public first stage')
    raw_control.public_commit(api, PREDECESSOR['commit'])
    previous = raw_control.git_manifest(api.ROOT, **{'commit':PREDECESSOR['commit'], 'path':PREDECESSOR['manifest'], 'blob':PREDECESSOR['git_blob'], 'digest':PREDECESSOR['sha256']})
    api.require(set(manifest) == set(previous) | {'predecessor'} and set(manifest.get('sources',{})) == set(previous['sources'])
                and all(manifest[k] == v for k,v in previous.items() if k != 'sources'), '959 predecessor frozen stage or evidence set changed')


def authority(api, entry):
    fixed = source_stage_pins.REVISION_959
    api.require(isinstance(fixed, dict) and set(fixed) == {'commit','manifest','git_blob','sha256'}
                and fixed['manifest'] == MANIFEST and api.HEX40.fullmatch(fixed['commit'])
                and api.HEX40.fullmatch(fixed['git_blob']) and api.HEX64.fullmatch(fixed['sha256']),
                '959 paired receipt requires a separately pinned public source stage')
    api.require(isinstance(entry,dict) and entry.get('source_stage') == fixed, 'activation differs from fixed public source stage')
    raw_control.public_commit(api,fixed['commit'])
    manifest = raw_control.git_manifest(api.ROOT,fixed['commit'],MANIFEST,fixed['git_blob'],fixed['sha256'])
    api.require(manifest.get('schema') == 'imre.compatibility.source-stage.v1'
                and manifest.get('imre_revision') == PIN and manifest.get('original_authority',{}).get('commit') == ORIGINAL_AUTHORITY,
                '959 source-stage manifest identity differs')
    verify_predecessor(api, manifest)
    return manifest


def audit_original_authority(api,index):
    """Rejoin the seven reviewed lines to the already-public 880648 authority."""
    import csv,io
    raw_control.public_commit(api,ORIGINAL_AUTHORITY)
    prefix='benchmarks/compatibility/';expected={prefix+'frozen/scripts.jsonl',*(prefix+'historical-scripts/'+name for name in ('archived-survey-7724.tsv','join.jsonl','verify.py'))}
    api.require(index.get('commit')==ORIGINAL_AUTHORITY and len(index.get('reviewed_frozen_anchors',{}))==7 and set(index.get('assets',{}))==expected and set(index.get('selected_ids',[]))=={'uv-10','uv-26','uv-29','ty-05'}, 'original authority or reviewed anchor count differs')
    for path,identity in index.get('assets',{}).items():
        raw=raw_control.git(api.ROOT,'show','--no-textconv',f'{ORIGINAL_AUTHORITY}:{path}')
        api.require(len(raw)==identity['bytes'] and raw_control.sha(raw)==identity['sha256'] and hashlib.sha1(f'blob {len(raw)}\0'.encode()+raw).hexdigest()==identity['git_blob'], 'original authority asset differs')
    frozen_path='benchmarks/compatibility/frozen/scripts.jsonl';raw=raw_control.git(api.ROOT,'show','--no-textconv',f'{ORIGINAL_AUTHORITY}:{frozen_path}');by_id={json.loads(line)['id']:line for line in raw.splitlines(keepends=True)}
    join_path='benchmarks/compatibility/historical-scripts/join.jsonl';join={x['id']:x for line in raw_control.git(api.ROOT,'show','--no-textconv',f'{ORIGINAL_AUTHORITY}:{join_path}').splitlines() if (x:=json.loads(line))}
    survey=raw_control.git(api.ROOT,'show','--no-textconv',f'{ORIGINAL_AUTHORITY}:{prefix}historical-scripts/archived-survey-7724.tsv');archived={x['id']:x for x in csv.DictReader(io.StringIO(survey.decode()),delimiter='\t')}
    for id,row in index['reviewed_frozen_anchors'].items():
        line=by_id.get(id,b'');anchor=json.loads(line) if line else {};canonical=api.canonical_originals('script').get(id,{})
        api.require(raw_control.sha(line)==row.get('raw_line_sha256') and row.get('source')==anchor.get('source')==canonical.get('source') and row.get('normalized_invocation')==anchor.get('normalized_invocation')==canonical.get('normalized_invocation')
                    and row.get('archived_original_argv')==json.loads(archived[id]['original_argv']) and all(row.get(k)==join[id].get(k) for k in ('archived_checkout','canonical_file_commit','commit_relation','argv_relation','role_mapping')), 'reviewed frozen anchor, source, argv or commit relation differs')


def require_paired_authority(api, row, entry):
    if row.get('paired'):
        manifest=authority(api,entry)
        validate_file_set(api,row,entry,manifest)


def validate_file_set(api,row,entry,manifest):
    kind=row['kind'];name=row['family'] if kind=='stdlib' else api.canonical_script_shards()[row['id']]
    stage=manifest['staged'][kind][name];binding=manifest['bindings'][kind][row['family'] if kind=='stdlib' else row['id']]
    receipt=f'benchmarks/compatibility/revisions/{PIN}/verified/{"stdlib" if kind=="stdlib" else "scripts"}/{name}.jsonl'
    sources={'binding':binding['path']} if kind=='stdlib' else {id:item['path'] for id,item in manifest['bindings']['script'].items() if item['shard']==name}
    fixed={**manifest['sources'],**manifest['artifacts'],**{item['path']:item['sha256'] for group in manifest['staged'].values() for item in group.values()}}
    api.require(set(entry)=={'adapter','receipt','staged_receipt','sources','files','source_stage'} and entry.get('adapter')==('raw-control-v1' if kind=='stdlib' else 'script-control-v1')
                and entry.get('receipt')==receipt and entry.get('staged_receipt')==stage['path'] and entry.get('sources')==sources
                and set(entry.get('files',{}))==set(fixed)|{receipt} and all(entry['files'].get(k)==v for k,v in fixed.items()), 'activation file set or binding differs from immutable 959 stage')
    for loc,digest in entry['files'].items():
        if getattr(source_stage_pins,'REVISION_959_SUCCESSOR',None) is not None:
            from extensions.successor959_v1 import authority as successor
            successor.legacy_file(api,loc,digest)
        else:
            api.require(isinstance(digest,str) and api.HEX64.fullmatch(digest) and raw_control.sha(api.repository_file(loc,api.ROOT).read_bytes())==digest, '959 source-stage bound file digest differs')
    return binding


def launch_config(api,row):
    """Resolve only the four original commands' declared fixture roles."""
    from pathlib import Path
    id=row['id'];inv=row['normalized_invocation'];stock=row['runs']['stock'];source=row['source']['path']
    api.require(Path(stock['argv'][0]).name=='python3.14' and Path(stock['cwd']).is_absolute(), 'stock Python or cwd differs')
    if id=='ty-05':expected=[stock['argv'][0],str(Path(stock['cwd'])/source)]
    else:expected=[stock['argv'][0],*inv['argv'][1:]]
    api.require(stock['argv']==expected, 'original stock argv differs')
    for tier in api.TIERS:
        run=row['runs'][tier];env=run['environment'];api.require(set(env)==set(inv['env']) and Path(run['cwd']).is_absolute(), 'script launch environment keys or cwd differ')
        for key,value in inv['env'].items():
            actual=env[key]
            if value in {'$TASK_HOME','<launch>'}:api.require(actual==str(Path(run['cwd']).parent/('home' if value=='$TASK_HOME' else 'launch')), 'script home role differs')
            elif value in {'$CACHE','$PATH'}:api.require(actual==stock['environment'][key] and bool(actual), 'script shared environment role differs')
            else:api.require(actual==value, 'script launch environment value differs')
        if tier!='stock' and id in {'uv-26','uv-29'}:api.require(run['argv']==[source,*inv['argv'][3:]], 'native original arguments differ')
        if tier!='stock' and id in {'uv-10','ty-05'}:api.require(len(run['argv'])==1 and Path(run['argv'][0]).is_absolute(), 'native saved-image invocation differs')


BWRAP_SHA256 = '573236e5328ac2ebb08f59ae3a9805b4f8d12bdef14be8af4450d5463294985f'
BUILD_TUPLES = {
    'ruff-a': ('c8d8b748b6070f5b7d3c5692139040195f8506189edf03d8df65472d30a85e22', 'e58466bb9d8c2cc8912cdc43cce917f13e9b7b82777f3f84684ad0e813234ee3'),
    'ruff-b': ('e2d0886948f766332df6370a2e3140a5be5c8b0fc108224ef7242fc38fc58bd4', '33ae5dfef5eff3c1fac5e0a5e19263daa980d7d80a5917979b426e85fa128c5c'),
}
CASES = {'uv-10': ('ruff-a',False), 'uv-26': ('ruff-b',True), 'uv-29': ('ruff-b',True), 'ty-05': ('ruff-b',False)}


def trace_bytes(file, role, gate=None):
    """Read a bound trace, checking both compressed and original identities."""
    import gzip
    path=file(role); raw=path.read_bytes()
    if path.suffix == '.gz':
        registry=json.loads(file('compressed_traces').read_text()); match=next(((key,x) for key,x in registry.items() if path.as_posix().endswith('/'+x['path'])),None);item=match[1] if match else None
        if not item or raw_control.sha(raw)!=item['compressed_sha256'] or len(raw)!=item['compressed_bytes']:
            raise ValueError('compressed gate trace digest differs')
        raw=gzip.decompress(raw)
        if raw_control.sha(raw)!=item['raw_sha256'] or len(raw)!=item['raw_bytes']:
            raise ValueError('raw gate trace digest differs')
        exported=json.loads(file('supplement_manifest').read_text()).get(match[0],{})
        if exported!={'bytes':len(raw),'sha256':raw_control.sha(raw)}:raise ValueError('gate trace differs from original supplement manifest')
    if gate is not None and raw_control.sha(raw)!=gate.get('trace_sha256'):
        raise ValueError('gate trace digest differs from capture')
    return raw


RUFF_A_TRANSCRIPT_SHA256 = 'd264e027edf743c00d4151fa8c93d1a18dd3dd1048c2d31b2822bc219493420a'
RUFF_A_COMMAND_SHA256 = 'ee84032b07ce5bef4b0c883accfd8b9df0452a4d26fb515ba0b92b9988d4b463'


def ruff_a_transcript(api,file,evidence):
    """Bind the retrospectively retrieved original Cloud build command to UV10 custody."""
    import shlex
    path=file('custody_transcript');raw=path.read_bytes();d=json.loads(raw);item=d.get('original_item',{});command=item.get('command','');cmd=command.encode()
    api.require(len(raw)==2505 and raw_control.sha(raw)==RUFF_A_TRANSCRIPT_SHA256 and evidence.get('cargo_transcript')=={'path':path.relative_to(api.ROOT).as_posix(),'sha256':RUFF_A_TRANSCRIPT_SHA256,'evidence_kind':'retrospectively-retrieved-original-cloud-command'}, 'Ruff-a retrospective transcript digest or binding differs')
    api.require(d.get('schema')=='imre.compatibility.retrospective-cloud-build-transcript.v1' and d.get('compiler_revision')==PIN and d.get('source_thread')=='6afc785b-eea0-8191-bdd0-36ee364e10ea' and d.get('original_turn')=='01a1ebb7-c93d-7541-888c-84e950c87dc6' and item.get('id')=='exec-2c4a6ab3-d566-4ca6-8706-cb3e9a52dd3c' and item.get('type')=='commandExecution' and item.get('status')=='completed' and item.get('exit_code')==0 and item.get('cwd')=='/workspace/scratch/ruff-a-95908f/imre' and item.get('stdout')=='build exit 0\n' and len(cmd)==item.get('command_bytes')==991 and raw_control.sha(cmd)==item.get('command_sha256')==RUFF_A_COMMAND_SHA256, 'Ruff-a original Cloud item or command identity differs')
    expected={name:raw_control.sha(file(role).read_bytes()) for name,role in (('prebuild.txt','custody_prebuild'),('build.log','custody_build_log'),('postbuild.txt','custody_postbuild'),('matched-artifacts.sha256','matched_artifacts'))}
    api.require(d.get('original_evidence_sha256')==expected, 'Ruff-a retrieved original evidence hashes differ')
    segment=command.rsplit('; CARGO_HOME=',1);api.require(len(segment)==2 and ' > "$lane/evidence/build.log" 2>&1;' in segment[1], 'Ruff-a original Cargo command is missing')
    words=shlex.split('CARGO_HOME='+segment[1].split(' > "$lane/evidence/build.log" 2>&1;',1)[0]);at=words.index('cargo') if 'cargo' in words else -1
    env=dict(word.split('=',1) for word in words[:at] if '=' in word);argv=words[at:];pre=file('custody_prebuild').read_text().splitlines()
    api.require(at==5 and len(env)==at and env==evidence.get('cargo_build_environment') and argv==evidence.get('cargo_build_argv')==['cargo','build','--offline','--release','--locked','-p','imre-compiler','-p','imre-runtime'] and env.get('CC')=='/usr/bin/cc' and env.get('CARGO_TARGET_DIR')=='/workspace/imre/target' and all(k+'='+env[k] in pre for k in ('CC','CARGO_TARGET_DIR')), 'Ruff-a original Cargo argv or environment differs')


def case_tuple(api,row,binding,file):
    lane,_=CASES.get(row['id'],(None,None)); expected=BUILD_TUPLES.get(lane)
    api.require(expected is not None and binding.get('build_tuple')==lane.replace('-','_') and (binding.get('compiler_sha256'),binding.get('runtime_sha256'))==expected,
                'case crossed its fixed build tuple')
    for tier in ('O0','O3'):
        tool=row['runs'][tier].get('toolchain',{});api.validate_toolchain(tool)
        api.require((tool.get('compiler_sha256'),tool.get('runtime_sha256'))==expected, 'case crossed its fixed build tuple')
    witness=file('generated_witness').read_bytes(); deps={n:file(role).read_bytes() for n,role in (('imre.d','cli_depfile'),('libimre_runtime.d','runtime_depfile'))};log=file('custody_build_log').read_bytes()
    evidence=json.loads(file('native_toolchain').read_text());api.require(row.get('toolchain_evidence')=={'path':file('native_toolchain').relative_to(api.ROOT).as_posix(),'sha256':raw_control.sha(file('native_toolchain').read_bytes())} and evidence.get('source_revision')==PIN and evidence.get('clean_source') is True and evidence.get('toolchain')==row['runs']['O0']['toolchain'] and evidence.get('build_log_sha256')==raw_control.sha(log),'case common toolchain evidence differs')
    if lane=='ruff-b':
        import shlex
        setup=json.loads(file('custody_setup').read_text());actual=raw_control.custody_tuple(api,setup,log.decode(),deps,witness,api.require)
        transfer=json.loads(file('transfer').read_text());api.require((actual['imre'],actual['libimre_runtime.a'])==expected and setup.get('clang',{}).get('sha256')==transfer.get('clang_sha256')=='8ef402d453d1ba4902e4ee0f0f847f6cfa01400c95aa43c24e97818b9c0e3f45' and (transfer.get('cli_sha256'),transfer.get('runtime_sha256'))==expected and transfer.get('source_revision')==PIN and evidence.get('build_argv')==shlex.split(setup['build_command']) and evidence.get('setup_sha256')==raw_control.sha(file('custody_setup').read_bytes()) and evidence.get('transfer_sha256')==raw_control.sha(file('transfer').read_bytes()), 'case build custody differs')
    else:
        ruff_a_transcript(api,file,evidence)
        pre=file('custody_prebuild').read_text();post=file('custody_postbuild').read_text();identities=raw_control.sha_lines('\n'.join(x for x in post.splitlines() if raw_control.SHA.fullmatch(x)))
        output='/workspace/imre/target/release/';product={n:{'sha256':identities.get(output+n)} for n in ('imre','libimre_runtime.a','imre.d','libimre_runtime.d')}
        anchor=json.loads(file('source_anchor').read_text());clang='/workspace/imre/target/release/imre-toolchain/bin/clang';api.require(PIN in pre and PIN in post and 'e6405a3243de706adcab772bf16942808d6ca6fb' in pre and (product['imre']['sha256'],product['libimre_runtime.a']['sha256'])==expected and evidence.get('source_anchor_sha256')==raw_control.sha(file('source_anchor').read_bytes()) and evidence.get('build_argv')==row['runs']['O0']['build_argv'] and anchor.get('artifacts',{}).get(clang,{}).get('sha256')=='8ef402d453d1ba4902e4ee0f0f847f6cfa01400c95aa43c24e97818b9c0e3f45', 'case clean source or output custody differs')
        raw_control.depfile_custody(api,lane,PIN,'/workspace/scratch/ruff-a-95908f/imre',product,log,deps,witness,api.require)
    return expected


def supervisor_custody(api,file):
    """Authenticate the single shared supervisor's public source and build receipt."""
    authority=json.loads(file('supervisor_authority').read_text());commit=authority.get('commit')
    api.require(commit=='4f4dafbe58cc097aa6c637b05c8e1c47eb0771ce','supervisor source authority differs')
    raw_control.public_commit(api,commit)
    for name,role in (('source.c','supervisor_source'),('commands.json','supervisor_build'),('artifacts.sha256','supervisor_identities')):
        item=authority['assets'][name];raw=raw_control.git(api.ROOT,'show','--no-textconv',f"{commit}:{item['public_path']}")
        api.require(raw==file(role).read_bytes() and raw_control.sha(raw)==item['sha256'] and hashlib.sha1(f'blob {len(raw)}\0'.encode()+raw).hexdigest()==item['git_blob'],'supervisor public source or build artifact differs')
    command=json.loads(file('supervisor_build').read_text())['supervisor_build'];argv=command.get('argv',[]);identities=raw_control.sha_lines(file('supervisor_identities').read_text())
    api.require(len(argv)==7 and argv[:5]==['/usr/bin/cc','-O2','-Wall','-Wextra','-o'] and command.get('exit_code')==0
                and identities.get(argv[5])=='36cce31bde9b0e1e7847d20d4a0821b2b509dca186d0b7c0231bb3e5fb14c88a' and identities.get(argv[6])==raw_control.sha(file('supervisor_source').read_bytes()), 'supervisor build custody differs')


ENV_SHA256 = 'eb1c64bbc9908afcdf8cb48524f17112d8dd76a249f80dee2f01da14747b3b6e'
ENV_SEAL_SHA256 = '796a432279ae047ce261db610d79eeb6c062f8af451d8c5ce14d069a669c162e'


def env_seal(api,id,file):
    seal=file('env_seal').read_bytes();api.require(raw_control.sha(seal)==ENV_SEAL_SHA256,'env-parity seal differs');info=json.loads(seal);lane='uv10' if id=='uv-10' else 'ty' if id=='ty-05' else 'uvb';manifest=file('lane_manifest').read_bytes()
    api.require(info.get('compiler_revision')==PIN and info.get('credit')==0 and set(info.get('cases',[]))==set(CASES) and info.get('env_trampoline')=={'path':'/usr/bin/env','sha256':ENV_SHA256} and info['retrospective_generated_witness']['sha256']==raw_control.sha(file('generated_witness').read_bytes()) and raw_control.sha(manifest)==info['lanes'][lane]['manifest_sha256'], 'env-parity authority, tuple or manifest differs')
    exported=json.loads(manifest)
    for role in ('withdraw_controller','O0_meta','O0_trace','O3_meta','O3_trace'):
        path=file(role);rel=path.as_posix().split('/env-parity/'+lane+'/',1)[-1];item=exported.get(rel,{});api.require(item.get('bytes')==path.stat().st_size and item.get('sha256')==raw_control.sha(path.read_bytes()),'selected env-parity asset differs from sealed lane')


def env_content(api,lines,pid,cwd,profile,policy):
    from collections import Counter
    from pathlib import Path
    expected=Counter((x['lexical'],x['kernel'],x['flags'],x['mode']) for x in policy['env_open_profiles'][profile]);actual=Counter()
    for e in api.successful_open_paths('\n'.join(lines),cwd,kernel_events=True):
        api.require(e['pid']==pid and e['operation'] in {'open','openat'} and not any(raw_control.payload_alias(x,task=True) for x in (e['lexical'],str(e['path']),str(e['returned']))) and '..' not in Path(e['lexical']).parts,'unauthorized env trampoline open')
        actual[(str(e['path']),str(e['returned']),e['flags'],e['mode'])]+=1
    api.require(actual==expected,'unauthorized env trampoline content open or census')


def script_withdrawal(api,id,meta,trace,smoke,smoke_trace,build,policy):
    """Check only the named case's exact bwrap/env protocol and content policy."""
    import re
    from pathlib import Path
    check=api.require;check(id in CASES and id in policy.get('script_cases',{}),'unrecognized script source policy');supervised=id!='uv-10';lines=trace.splitlines();seq=raw_control.exec_sequence(lines,check,4 if supervised else 3);supervisor=seq[0] if supervised else None;wrapper,env,target=seq[-3:];image=target[2];cwd=meta['cwd'];source=build['argv'][2];original=str(Path(build.get('cwd',cwd))/source) if not Path(source).is_absolute() else source;mask=str(Path(original).parent);canonical=api.canonical_originals('script')[id];sourcepath=canonical['source']['path'];args=canonical['normalized_invocation']['argv'];argv0=CASES[id][1];extra=args[args.index(sourcepath)+1:] if argv0 else [];envargs=['/usr/bin/env','-u','PWD',*(['--argv0='+sourcepath] if argv0 else []),image,*extra];expected=['/usr/bin/bwrap','--ro-bind','/','/','--tmpfs',mask,*(['--bind',cwd,cwd] if supervised else []),'--chdir',cwd,'--die-with-parent','--',*envargs];child=[sourcepath,*extra] if argv0 else [image]
    check(wrapper[2:]==('/usr/bin/bwrap',expected) and env[2:]==('/usr/bin/env',envargs) and env[1]==target[1] and target[3]==child and meta.get('bwrap_sha256')==BWRAP_SHA256 and meta.get('env_trampoline_sha256')==ENV_SHA256,'script wrapper, env trampoline, argv or mask differs')
    declared=meta['environment'];check(raw_control.exec_environment(lines[wrapper[0]],check)==declared and raw_control.exec_environment(lines[env[0]],check)=={**declared,'PWD':cwd} and raw_control.exec_environment(lines[target[0]],check)==declared==meta.get('actual_image_environment'),'script image child environment differs from declared launch')
    check(meta.get('controller_sha256') is not None, 'missing as-used controller identity')
    if supervised:
        entry=str(Path(cwd)/sourcepath);stash=str(Path(cwd).parent/'withdrawn.source');check(supervisor[3]==[supervisor[2],entry,stash,'-','-','/usr/bin/bwrap',*expected] and meta.get('argv',[])[-len(supervisor[3]):]==supervisor[3] and meta.get('supervisor_sha256')=='36cce31bde9b0e1e7847d20d4a0821b2b509dca186d0b7c0231bb3e5fb14c88a' and raw_control.exec_environment(lines[supervisor[0]],check)==declared,'script supervisor invocation or environment differs')
        present=raw_control.stat(lines,supervisor[1],entry,False,supervisor[0]+1);missing=raw_control.stat(lines,supervisor[1],stash,True,present+1);moved=raw_control.find(lines,rf'^{supervisor[1]}\s+rename\({re.escape(json.dumps(entry))}, {re.escape(json.dumps(stash))}\) = 0$',missing+1);absent=raw_control.stat(lines,supervisor[1],entry,True,moved+1);check(absent<wrapper[0]<env[0]<target[0],'script source was not withdrawn before exact exec')
    else:check(meta.get('argv',[])[-len(expected):]==expected and meta.get('protected_parent_source_intact') is True,'uv10 exact bwrap source protocol differs')
    end=raw_control.find(lines,rf'^{target[1]}\s+exit_group\(0\)',target[0]);wait=raw_control.find(lines,rf'^{wrapper[1]}\s+wait4\(-1, .*WEXITSTATUS\(s\) == 0.*= {target[1]}$',end+1);wrapper_end=raw_control.find(lines,rf'^{wrapper[1]}\s+exit_group\(0\)',wait+1)
    if supervised:
        parent=raw_control.find(lines,rf'^{supervisor[1]}\s+<\.\.\. wait4 resumed>.*WEXITSTATUS\(s\) == 0.*= {wrapper[1]}$',wrapper_end+1);raw_control.find(lines,rf'^{supervisor[1]}\s+rename\({re.escape(json.dumps(stash))}, {re.escape(json.dumps(entry))}\) = 0$',parent+1)
    sl=smoke_trace.splitlines();sms=raw_control.exec_sequence(sl,check,3 if id=='uv-10' else 2);a,b=sms[0],sms[-1];exact=['/usr/bin/bwrap','--ro-bind','/','/','--tmpfs',mask,*(['--chdir',cwd,'--die-with-parent','--','/usr/bin/env','-u','PWD'] if id=='uv-10' else ['--']),'/usr/bin/stat',original]
    check(a[2:]==('/usr/bin/bwrap',exact) and b[2:]==('/usr/bin/stat',['/usr/bin/stat',original]) and smoke.get('bwrap_sha256')==BWRAP_SHA256 and smoke.get('exit_code')==1 and smoke.get('trace_sha256')==raw_control.sha(smoke_trace.encode()),'script exact-entry mask smoke differs')
    if id=='uv-10':check(sms[1][2:]==('/usr/bin/env',['/usr/bin/env','-u','PWD','/usr/bin/stat',original]) and sms[1][1]==b[1],'uv10 mask smoke env trampoline differs')
    raw_control.find(sl,rf'^{b[1]}\s+statx\(.*{re.escape(json.dumps(original))}.*= -1 ENOENT\b',b[0]+1);env_content(api,lines[env[0]:target[0]],env[1],cwd,'en_US' if id=='uv-10' else 'C',policy);raw_control.native_content(api,lines[target[0]:end+1],target[1],image,cwd,(policy,id,original,mask),check)
    check(meta.get('image_sha256')==build.get('image_sha256') and meta.get('trace_sha256')==raw_control.sha(trace.encode()) and meta.get('exit_code')==0,'script image, trace or exit differs');return lines[target[0]],target


def gate_control(api,id,tier,mode,file,build,expected):
    from pathlib import Path
    meta=json.loads(file(f'{mode}_{tier}_json').read_text());trace=trace_bytes(file,f'{mode}_{tier}_trace',meta).decode();first,child=raw_control.execs(trace.splitlines(),api.require);cmd=child[3];diagnostic=file(f'{mode}_{tier}_stderr').read_text();absent=meta.get('image_missing',meta.get('output_missing'))
    compiler,runtime=build['argv'][0],api.runtime_argument(build['argv'])
    if id=='uv-10':
        compiler='/workspace/scratch/ruff-a-95908f/matched-release/imre';runtime='/workspace/scratch/ruff-a-95908f/matched-release/libimre_runtime.a';identities=raw_control.sha_lines(file('matched_artifacts').read_text())
        api.require((identities.get(compiler),identities.get(runtime))==expected,'uv10 matched gate artifacts differ from case tuple')
    api.require(meta.get('probe_exit')==0 and meta.get('compiler_exit')==1 and absent is True and raw_control.sha(file(f'{mode}_{tier}_stderr').read_bytes())==meta.get('stderr_sha256') and file(f'{mode}_{tier}_stdout').read_bytes()==b'' and meta.get('stdout_sha256')==raw_control.sha(b'')
                and cmd[0]==compiler and api.runtime_argument(cmd)==runtime and api.option(cmd,'--opt-level')==tier[1:] and meta.get('probe_sha256')=='1294e97884c87cd49368a05ebbcd89962081b52663a2e9d955f8bd5ecded2a36' and meta.get('probe_source_sha256')==raw_control.sha(file('gate_probe_source').read_bytes())=='fa57a3d90c735b76210df621af5e32619afb282729922246da93c836d9437cd2', 'script gate child, image, tuple or digest differs')
    if mode=='direct' or id=='uv-10':
        raw_control.trace_direct_gate(trace,cmd,first[2],diagnostic,api.require,expected_exit=1)
        if id=='uv-10':
            opened={str(e['path']) for e in api.successful_open_paths('\n'.join(trace.splitlines()[child[0]:]),str(Path(cmd[2]).parent),kernel_events=True) if e['pid']==child[1]}
            required={str(Path(cmd[2]).parent/'uv10_subject.py')} | ({str(Path(cmd[2]).parent/'bad_status.py')} if mode=='imported' else set())
            api.require(required<=opened,'uv10 gate did not load the exact subject and imported poison')
    else:
        module='control_poison.py' if id=='ty-05' else 'architecture_control.py';poison=str(Path(cmd[2]).parent/(module if id=='ty-05' else '../'+module));poison=str(Path(poison).resolve())
        raw_control.trace_gate(trace,cmd,first[2],poison,diagnostic,api.require,expected_exit=1)


STOCK_SEAL_SHA256 = '3120de9627193e649c53230b7d4ac2f299598fa7a9a63e38c5904c197d5a27a9'
STOCK_PYTHON_SHA256 = 'c27397da5541f67655be462e128ca3d3eb6e0a8eb0d39036896c9042529f4304'


def stock_trace_bytes(file):
    import gzip
    return gzip.decompress(file('stock_trace').read_bytes())


def fresh_stock_seal(api, case, file):
    """Authenticate the fresh capture independently of the historical 880648 survey."""
    base=api.DATA_ROOT/'evidence/scripts/stock-before';seal=file('stock_seal').read_bytes()
    api.require(case in {'uv-26','uv-29'} and file('stock_seal')==base/'COMPLETE.json'
                and raw_control.sha(seal)==STOCK_SEAL_SHA256, 'fresh stock seal or case differs')
    info=json.loads(seal);manifest=file('stock_manifest').read_bytes()
    api.require(info.get('schema')=='imre.compatibility.unified959-fresh-stock.v1' and info.get('credit')==0
                and info.get('compiler_revision')==PIN and info.get('authority')==ORIGINAL_AUTHORITY
                and set(info.get('cases',[]))=={'uv-26','uv-29'} and file('stock_manifest')==base/'export-manifest.json'
                and raw_control.sha(manifest)==info.get('manifest_sha256'), 'fresh stock manifest identity differs')
    exported=json.loads(manifest)
    for role,name,exact in (('stock_controller','capture/controller.py',True),('stock_meta',case+'/result.json',True),
                            ('stock_stdout',case+'/stdout',False),
                            ('stock_stderr',case+'/stderr',False),('stock_effects',case+'/effects.json',False)):
        path=file(role);raw=path.read_bytes()
        api.require((not exact or path==base/name) and exported.get(name)=={'bytes':len(raw),'sha256':raw_control.sha(raw)},
                    'fresh stock sealed asset or case differs: '+role)
    trace=stock_trace_bytes(file)
    api.require(file('stock_trace')==base/case/'stock.trace.gz' and exported.get(case+'/stock.trace')=={'bytes':len(trace),'sha256':raw_control.sha(trace)}, 'fresh stock raw trace or case differs')


def fresh_stock_observation(api, row, file):
    """Rejoin the raw stock exec, source, full snapshots and derived effects to its frozen original."""
    from pathlib import Path
    case=row['id'];meta=json.loads(file('stock_meta').read_text());observed=row['runs']['stock'];trace=stock_trace_bytes(file).decode()
    lines=trace.splitlines();child=raw_control.exec_sequence(lines,api.require,1)[0];interpreter=meta.get('interpreter',{})
    public=raw_control.git(api.ROOT,'show','--no-textconv',f'{ORIGINAL_AUTHORITY}:benchmarks/compatibility/frozen/scripts.jsonl')
    matches=[line for line in public.splitlines(keepends=True) if json.loads(line).get('id')==case]
    api.require(len(matches)==1 and raw_control.sha(matches[0])==meta.get('anchor_raw_line_sha256'), 'fresh stock frozen anchor or case differs')
    anchor=json.loads(matches[0]);frozen=api.validate_frozen_anchor(anchor);source=row['source'];before=meta.get('before',{});after=meta.get('after',{})
    api.require(meta.get('compiler_revision')==PIN and meta.get('historical_authority')==ORIGINAL_AUTHORITY
                and meta.get('source')==source==anchor.get('source') and meta.get('original_normalized_invocation')==row['normalized_invocation']==anchor.get('normalized_invocation')
                and raw_control.sha(file('source').read_bytes())==meta.get('source_sha256')==source['sha256'], 'fresh stock source, invocation or authority differs')
    api.require({k:v.get('sha256') for k,v in before.items()}==frozen['preimages']==row['fixture_provenance']['preimages']['stock']
                and before.get(source['path'],{}).get('sha256')==source['sha256'] and before[source['path']].get('bytes')==file('source').stat().st_size
                and after.get(source['path'])==before.get(source['path']), 'fresh stock before snapshot or source differs')
    api.require(child[2]==interpreter.get('path')==interpreter.get('resolved_path')==observed['argv'][0]
                and interpreter.get('sha256')==STOCK_PYTHON_SHA256 and interpreter.get('version')=='Python 3.14.5'
                and child[3]==meta.get('actual_child_argv')==meta.get('argv')==observed['argv']
                and raw_control.exec_environment(lines[child[0]],api.require)==meta.get('actual_child_environment')==meta.get('environment')==observed['environment']
                and meta.get('cwd')==observed['cwd'] and meta.get('controller_sha256')==raw_control.sha(file('stock_controller').read_bytes())
                and meta.get('trace_sha256')==raw_control.sha(trace.encode()) and meta.get('trace_bytes')==len(trace.encode())
                and meta.get('exit_code')==observed.get('exit_code')==0 and meta.get('timed_out') is False, 'fresh stock raw argv, environment, interpreter or capture differs')
    source_path=Path(observed['cwd'])/source['path'];source_lines=[line for line in lines[child[0]:] if json.dumps(str(source_path)) in line]
    events=api.successful_open_paths('\n'.join(source_lines),observed['cwd'],kernel_events=True)
    api.require(any(e['pid']==child[1] and e['path']==e['returned']==source_path and e['flags'] in {'O_RDONLY','O_RDONLY|O_CLOEXEC'} for e in events), 'fresh stock child did not open the original source')
    raw_control.find(lines,rf'^{child[1]}\s+exit_group\(0\)',child[0]+1)
    effect={k:[before.get(k),after.get(k)] for k in sorted(set(before)|set(after)) if before.get(k)!=after.get(k)}
    expected=json.dumps(effect,sort_keys=True,separators=(',',':')).encode();reference=frozen['reference']
    api.require(file('stock_effects').read_bytes()==expected==reference['effects_manifest'].encode(), 'fresh stock effect mapping differs from snapshots or frozen reference')
    for role,field in (('stock_stdout','stdout'),('stock_stderr','stderr'),('stock_effects','effects')):
        path=file(role);raw=path.read_bytes();receipt_key='effects_manifest_path' if field=='effects' else field+'_path';digest=raw_control.sha(raw)
        api.require(path==api.repository_file(observed[receipt_key],api.DATA_ROOT) and digest==meta.get(field+'_sha256')==observed.get(field+'_sha256')==reference[field+'_sha256'], 'fresh stock raw stream or effect digest differs')
    for tier in ('O0','O3'):
        path=file(tier+'_effects');api.require(path==api.repository_file(row['runs'][tier]['effects_manifest_path'],api.DATA_ROOT)
                and path.read_bytes()==expected and raw_control.sha(expected)==row['runs'][tier]['effects_sha256'], 'fresh stock/native effect mapping differs')


def strict_case(api,row,binding,entry):
    from pathlib import Path
    id=row['id'];f=raw_control.bound_artifacts(api,entry,binding.get('artifacts'));obj=lambda role:json.loads(f(role).read_text());text=lambda role:f(role).read_text();launch_config(api,row)
    api.require(binding.get('schema')=='imre.compatibility.script-control.v1' and binding.get('id')==id and binding.get('shard')==api.canonical_script_shards()[id] and binding.get('strict_source_control')=='exact-bwrap-withdraw-v1', 'script identity or strict source mode differs')
    expected=case_tuple(api,row,binding,f);source=f('source').read_bytes();api.require(raw_control.sha(source)==row['source']['sha256']==row['launcher_id'], 'script source or launcher digest differs')
    env_seal(api,id,f)
    if id!='uv-10':supervisor_custody(api,f)
    if id=='uv-10':
        direct=b'from uv10_subject import main\n\ndef launch() -> int:\n    status: int = main()\n    status = True\n    return status\n';imported=direct.replace(b'from uv10_subject import main\n',b'from uv10_subject import main\nfrom bad_status import BAD_STATUS\n').replace(b'status = True',b'status = BAD_STATUS')
        api.require(f('subject').read_bytes()==source and f('direct_source').read_bytes()==direct and f('imported_source').read_bytes()==imported and f('poison').read_bytes()==b'BAD_STATUS: bool = True\n','uv10 declared gate source differs')
    else:
        api.require(f('direct_source').read_bytes()==b'status: int = True\n' and f('poison').read_bytes()==b'status: int = True\n', 'script annotation poison source differs')
        imported=f('imported_source').read_bytes();module=b'control_poison' if id=='ty-05' else b'architecture_control';api.require(imported.count(b'import '+module+b'\n')==1 and imported.replace(b'\nimport '+module+b'\n',b'',1)==source, 'script imported-source delta differs')
    envs=[]
    for tier in ('O0','O3'):
        rawbuild=obj('report')['builds'][tier] if id=='ty-05' else obj(tier+'_build');build=rawbuild.get('build',rawbuild);run=row['runs'][tier];api.require(run.get('build_argv')==build.get('argv') and build.get('exit_code')==0 and run.get('image_sha256')==build.get('image_sha256'), 'script exact native build argv or image differs')
        for stream in ('stdout','stderr'):
            raw=(build.get(stream,'').encode() if id=='ty-05' else f(tier+'_build_'+stream).read_bytes());api.require(raw_control.sha(raw)==build.get(stream+'_sha256'),'script raw build output digest differs')
        meta=obj(tier+'_meta');trace=text(tier+'_trace');smoke=obj(tier+'_mask_smoke' if id in {'uv-26','uv-29'} else 'mask_smoke');st=text(tier+'_mask_smoke_trace' if id in {'uv-26','uv-29'} else 'mask_smoke_trace');line,target=script_withdrawal(api,id,meta,trace,smoke,st,build,obj('policy'))
        effects=meta.get('effects_sha256') or raw_control.sha(json.dumps(meta.get('effects'),sort_keys=True,separators=(',',':')).encode())
        api.require(run['argv']==target[3] and all(run.get(k)==meta.get(k) for k in ('cwd','environment','exit_code','stdout_sha256','stderr_sha256','image_sha256')) and effects==run.get('effects_sha256') and meta.get('controller_sha256')==raw_control.sha(f('withdraw_controller').read_bytes()), 'script raw launch, controller, environment, streams or effects differ')
        before=meta.get('preimages',meta.get('before',meta.get('fixture_before',{})));api.require({k:v['sha256'] if isinstance(v,dict) else v for k,v in before.items()}==row['fixture_provenance'].get('preimages',{}).get(tier), 'script native preimages differ')
        for mode in ('direct','imported'):gate_control(api,id,tier,mode,f,build,expected)
        envs.append((line,run['environment']))
    if id in {'uv-26','uv-29'}:
        fresh_stock_seal(api,id,f);fresh_stock_observation(api,row,f)
    stock=obj('stock_meta');observed=row['runs']['stock'];api.require(stock.get('argv')==observed['argv'] and stock.get('cwd')==observed['cwd'] and stock.get('environment',stock.get('env'))==observed['environment'] and all(stock.get(k)==observed.get(k) for k in ('exit_code','stdout_sha256','stderr_sha256')) and stock.get('effects_sha256',stock.get('comparison',{}).get('effects_sha256'))==observed.get('effects_sha256'),'script raw stock invocation, environment, streams or effects differ')
    stock_before=stock.get('fixture_before',stock.get('before',stock.get('preimage_snapshot',{})));api.require(stock_before and {k:v['sha256'] if isinstance(v,dict) else v for k,v in stock_before.items() if not isinstance(v,dict) or 'sha256' in v}==row['fixture_provenance'].get('preimages',{}).get('stock'), 'script stock fixture-before witness is missing or differs')
    for line,declared in envs:
        actual=raw_control.exec_environment(line,api.require);api.require(actual==declared,'script image child environment differs from declared launch')


def validate_record(api,row,eligible,check,entry):
    import json
    manifest=authority(api,entry);ref=validate_file_set(api,row,entry,manifest);binding=json.loads(api.checked_file(api.repository_file(ref['path'],api.DATA_ROOT),ref['sha256']).read_text());index=api.repository_file(manifest['original_authority']['index'],api.DATA_ROOT);audit_original_authority(api,json.loads(index.read_text()))
    staged=api.repository_file(entry['staged_receipt'],api.DATA_ROOT);original=next((json.loads(line) for line in staged.read_text().splitlines() if json.loads(line).get('id')==row['id']),None)
    check(original is not None and all(row.get(k)==v for k,v in original.items() if k not in {'paired','credit_reason'}), 'verified script differs from immutable staged command or observations')
    check(type(row.get('paired')) is bool and (not row['paired'] or eligible),'script pair requires exact frozen stock/O0/O3 observations')
    if row['paired']:strict_case(api,row,binding,entry)
