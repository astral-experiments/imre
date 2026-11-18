"""Digest-bound finite-unittest raw control adapter for snapshot receipts.

The binding describes locations, not control verdicts. Credit is derived from
raw process/file traces and independently anchored original result events.
"""
from __future__ import annotations

import ast
import hashlib
import json
import posixpath
import re
import subprocess
import tarfile
import zipfile
from collections import Counter
from functools import lru_cache
from pathlib import Path

from collect_unittest_results import parse

EXEC = re.compile(r'^(\d+)\s+execve\(("(?:[^"\\]|\\.)*"), (\[.*\]), .*\) = 0$')
OPEN = re.compile(r'^\d+\s+(?:open|openat|openat2|creat)\(.*?("(?:[^"\\]|\\.)*")')
SHA = re.compile(r'^([0-9a-f]{64})  (.+)$')


def sha(data): return hashlib.sha256(data).hexdigest()


def stock_source_bytes(path, member, source, require):
    """Read the pinned source file or its exact regular archive member."""
    if member is None:
        return path.read_bytes()
    require(member in {source['path'], f"cpython-{source['commit']}/{source['path']}"},
            'stock source archive member path differs from the pinned source')
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            entries = [entry for entry in archive.infolist() if entry.filename == member]
            require(len(entries) == 1 and not entries[0].is_dir()
                    and (entries[0].external_attr >> 16) & 0o170000 in {0, 0o100000},
                    'stock source archive member is missing, duplicated or not a regular file')
            return archive.read(entries[0])
    try:
        with tarfile.open(path, 'r:*') as archive:
            entries = [entry for entry in archive.getmembers() if entry.name == member]
            require(len(entries) == 1 and entries[0].isreg(),
                    'stock source archive member is missing, duplicated or not a regular file')
            return archive.extractfile(entries[0]).read()
    except tarfile.TarError as exc:
        raise ValueError('stock source archive format is invalid') from exc


def validate_stock_source(path, member, source, capture, require):
    """Anchor actual archive and member bytes to the immutable original source."""
    archived = stock_source_bytes(path, member, source, require)
    blob = hashlib.sha1(f'blob {len(archived)}\0'.encode() + archived).hexdigest()
    require(sha(archived) == source['sha256'] and blob == source['git_blob'],
            'stock source archive member differs from the pinned source')
    if capture is not None:
        require(capture.get('source_commit') == source['commit']
                and capture.get('source_archive_sha256') == sha(path.read_bytes())
                and ('source' not in capture or capture['source'] == source)
                and ('source_sha256' not in capture or capture['source_sha256'] == source['sha256'])
                and ('source_member' not in capture or capture['source_member'] == member),
                'fresh stock capture metadata or raw stdout differs: stock source identity or archive digest differs')


def sha_lines(text):
    rows = [SHA.fullmatch(line) for line in text.splitlines()]
    if not rows or any(m is None for m in rows): raise ValueError('invalid raw sha256sum evidence')
    result = {m[2]: m[1] for m in rows}
    if len(result) != len(rows): raise ValueError('duplicate raw sha256sum path')
    return result


def exec_sequence(lines, require, count=2):
    calls = [(i, line) for i, line in enumerate(lines) if re.search(r'\bexecve(?:at)?\(', line)]
    require(len(calls) == count, 'raw control must execute exactly supervisor and target; a native child exec is unverified'
            if count == 2 else 'raw control has an unverified exec sequence')
    result = []
    for i, line in calls:
        match = EXEC.fullmatch(line)
        require(match is not None, 'raw exec attempt failed, was truncated, or used execveat')
        result.append((i, match[1], json.loads(match[2]), json.loads(match[3])))
    if count > 1: require(result[0][1] != result[-1][1], 'supervisor and target PID must differ')
    return result


def execs(lines, require):
    return exec_sequence(lines, require, 2)


def exec_environment(line, require):
    """Read a complete environment from a verbose strace execve record."""
    try:
        values = json.loads(line.rsplit('], ', 1)[1].removesuffix(') = 0'))
        pairs = [value.split('=', 1) for value in values]
    except (IndexError, ValueError, TypeError, AttributeError):
        require(False, 'raw exec environment is absent or truncated')
    require(isinstance(values, list) and all(len(pair) == 2 for pair in pairs)
            and len({pair[0] for pair in pairs}) == len(pairs), 'raw exec environment is invalid or duplicated')
    return dict(pairs)


def find(lines, pattern, start=0):
    for i in range(start, len(lines)):
        if re.search(pattern, lines[i]): return i
    raise ValueError('missing raw trace operation: ' + pattern)


def stat(lines, pid, path, absent, start=0):
    tail = r'= -1 ENOENT\b' if absent else r'= 0$'
    return find(lines, rf'^{pid}\s+newfstatat\(AT_FDCWD(?:<[^<>]+>)?, {re.escape(json.dumps(path))}, .*{tail}', start)


def exits(lines, parent, child, code, start):
    end = find(lines, rf'^{child}\s+exit_group\({code}\)', start)
    waited = find(lines, rf'^{parent}\s+(?:<\.\.\. wait4 resumed>|wait4\({child},).*WEXITSTATUS\(s\) == {code}.*= {child}$', end + 1)
    return waited


def trace_gate(text, command, probe, poison_path, diagnostic, require, expected_exit=None):
    lines = text.splitlines()
    first, target = execs(lines, require)
    out = command[command.index('-o') + 1]
    require(first[2:] == (probe, [probe, 'gate', out, *command]) and target[2:] == (command[0], command), 'raw gate invocation differs from matched O0 build')
    before = stat(lines, first[1], out, True, first[0] + 1)
    require(before < target[0], 'gate output was not absent before build')
    for path in (command[2], poison_path):
        found = False
        for line in lines[target[0]+1:]:
            m = OPEN.match(line)
            if m and line.startswith(target[1] + ' ') and json.loads(m[1]) == path and re.search(r'= [0-9]+(?:<[^>]*>)?$', line): found = True
        require(found, 'gate compiler did not open the actual launcher and imported poison')
    m = re.search(rf'^{target[1]}\s+exit_group\(([1-9][0-9]*)\)', text, re.M)
    require(m is not None, 'gate compiler did not exit nonzero')
    require(expected_exit is None or int(m[1]) == expected_exit, 'gate compiler exit differs')
    waited = exits(lines, first[1], target[1], int(m[1]), target[0])
    after = stat(lines, first[1], out, True, waited + 1)
    find(lines, rf'^{first[1]}\s+exit_group\(0\)', after + 1)
    require(poison_path in diagnostic and re.search(r'expected\s+int,\s*got\s+bool', diagnostic), 'missing imported int/bool diagnostic')


def trace_direct_gate(text, command, probe, diagnostic, require, expected_exit=None):
    lines = text.splitlines(); first, target = execs(lines, require)
    out = command[command.index('-o') + 1]
    require(first[2:] == (probe, [probe, 'gate', out, *command]) and target[2:] == (command[0], command),
            'raw direct gate invocation differs')
    before = stat(lines, first[1], out, True, first[0] + 1)
    require(before < target[0] and any(
        (m := OPEN.match(line)) and line.startswith(target[1] + ' ') and json.loads(m[1]) == command[2]
        and re.search(r'= [0-9]+(?:<[^>]*>)?$', line) for line in lines[target[0]+1:]), 'direct gate did not open launcher')
    m = re.search(rf'^{target[1]}\s+exit_group\(([1-9][0-9]*)\)', text, re.M)
    require(m is not None, 'direct gate compiler did not exit nonzero')
    require(expected_exit is None or int(m[1]) == expected_exit, 'direct gate compiler exit differs')
    waited = exits(lines, first[1], target[1], int(m[1]), target[0])
    after = stat(lines, first[1], out, True, waited + 1)
    find(lines, rf'^{first[1]}\s+exit_group\(0\)', after + 1)
    require(command[2] in diagnostic and re.search(r'expected\s+int,\s*got\s+bool', diagnostic),
            'missing direct int/bool diagnostic')


def payload_alias(path, *, task=False):
    task_part = r'(?:task/[0-9]+/)?' if task else ''
    return re.search(r'(?:^|/)proc/(?:[0-9]+|self|thread-self)/'+task_part+r'fd/[0-9]+(?:/|$)|(?:^|/)dev/fd/[0-9]+(?:/|$)', path) is not None


def trace_withdraw(api, text, image, probe, tree, stash, leaf, cwd, require, extra_sources=(), strict_payloads=False, content_policy=None):
    """Allow only ENOENT withdrawn-path probes; never allow Python probes or child execs."""
    from posixpath import normpath
    lines = text.splitlines()
    first, target = execs(lines, require)
    require(first[2:] == (probe, [probe, 'withdraw', tree, stash, leaf, image])
        and target[2:] == (image, [image]), 'withdrawal did not run the saved image directly')
    pre = stat(lines, first[1], leaf, False, first[0]+1)
    missing_stash = stat(lines, first[1], stash, True, pre+1)
    mv = find(lines, rf'^{first[1]}\s+rename\({re.escape(json.dumps(tree))}, {re.escape(json.dumps(stash))}\) = 0$', missing_stash+1)
    a = stat(lines, first[1], tree, True, mv+1)
    b = stat(lines, first[1], leaf, True, a+1)
    require(b < target[0], 'test source tree was not absent before native exec')
    waited = exits(lines, first[1], target[1], 0, target[0])
    a = stat(lines, first[1], tree, True, waited+1)
    b = stat(lines, first[1], leaf, True, a+1)
    restored = find(lines, rf'^{first[1]}\s+rename\({re.escape(json.dumps(stash))}, {re.escape(json.dumps(tree))}\) = 0$', b+1)
    exists = stat(lines, first[1], leaf, False, restored+1)
    find(lines, rf'^{first[1]}\s+exit_group\(0\)', exists+1)
    hide = stat(lines, first[1], leaf, True, mv+1)
    for source in extra_sources:
        source_stash = source + '.raw-control-stash'
        present = stat(lines, first[1], source, False, hide+1)
        absent_stash = stat(lines, first[1], source_stash, True, present+1)
        moved = find(lines, rf'^{first[1]}\s+rename\({re.escape(json.dumps(source))}, {re.escape(json.dumps(source_stash))}\) = 0$', absent_stash+1)
        hide = stat(lines, first[1], source, True, moved+1)
        require(hide < target[0], 'extra source was not withdrawn before native exec')
    show = b
    for source in reversed(extra_sources):
        source_stash = source + '.raw-control-stash'
        missing = stat(lines, first[1], source, True, show+1)
        moved = find(lines, rf'^{first[1]}\s+rename\({re.escape(json.dumps(source_stash))}, {re.escape(json.dumps(source))}\) = 0$', missing+1)
        show = stat(lines, first[1], source, False, moved+1)
        require(show < restored, 'extra source was not restored after native exec')
    require(isinstance(cwd, str) and Path(cwd).is_absolute(), 'native cwd missing')
    stem = Path(leaf).stem.lower()
    extra_paths = {Path(p) for source in extra_sources for p in (source, source+'.raw-control-stash')}
    launcher_stem = Path(extra_sources[0]).stem.lower() if extra_sources else None
    native = lines[target[0]:waited+1]
    for line in native:
        # A successful io_uring setup would permit unobserved asynchronous opens.
        require(not re.search(r'\bio_uring_setup\(.*= [0-9]+$', line), 'native io_uring may hide file access')
        m = OPEN.match(line) or re.match(r'^\d+\s+newfstatat\(.*?("(?:[^"\\]|\\.)*")', line)
        if m:
            try: path = ast.literal_eval(m[1])
            except (ValueError, SyntaxError): require(False, "invalid traced path")
            require(isinstance(path, str), "invalid traced path")
            if strict_payloads and re.search(r'\b(?:open|openat|openat2|creat)\(.*\) = [0-9]+(?:<[^>]*>)?$', line):
                require(not payload_alias(path), 'native opened a payload fd alias')
            folded = path.lower()
            name = Path(path).name.lower()
            python_probe = 'libpython' in folded or any(part.startswith('python') for part in Path(folded).parts)
            require(not python_probe, 'native probed CPython or libpython')
            resolved = Path(normpath(path if Path(path).is_absolute() else str(Path(cwd)/path)))
            withdrawn = (name == stem+'.py'
                or name in {stem+'.pyc', stem+'.pyo'}
                or (name.startswith(stem+'.') and name.endswith(('.pyc', '.pyo')))
                or resolved.is_relative_to(Path(tree))
                or resolved.is_relative_to(Path(stash))
                or Path(stash).name in Path(path).parts
                or resolved in extra_paths
                or (launcher_stem and (name in {launcher_stem+'.pyc',launcher_stem+'.pyo'}
                    or name.startswith(launcher_stem+'.') and name.endswith(('.pyc','.pyo')))))
            # Only an explicit ENOENT proves this withdrawn path was absent.
            require(not withdrawn or re.search(r'\) = -1 ENOENT(?: \([^)]*\))?$', line),
                    'native opened CPython, libpython, withdrawn source or matching bytecode')
        if strict_payloads:
            require(not re.search(r'(?:\b(?:open|openat|openat2|creat)\(.*<unfinished|<\.\.\. (?:open|openat|openat2|creat) resumed>)', line), 'unfinished native payload open is unverified')
            require(not re.search(r'\bcreat\(.*\) = [0-9]+(?:<[^>]*>)?$', line), 'native successful creat is unverified')
    if content_policy is not None:
        native_content(api, native, target[1], image, cwd, content_policy, require)
    for path in api.successful_open_paths('\n'.join(native), cwd, preserve_traversal=strict_payloads):
        lexical = str(path)
        path = Path(normpath(lexical))
        if strict_payloads:
            require(path.suffix.lower() not in {'.py','.pyc','.pyo','.zip','.whl'} and not payload_alias(lexical) and not payload_alias(str(path)),
                    'native opened unauthorized Python source, bytecode, archive or fd payload')
        name = path.name.lower()
        cached = path.parent.name == '__pycache__' and name.startswith(stem+'.') and path.suffix in {'.pyc','.pyo'}
        forbidden = (path.is_relative_to(Path(tree)) or path.is_relative_to(Path(stash))
                     or name in {stem+'.py',stem+'.pyc',stem+'.pyo'} or cached
                     or path in extra_paths
                     or (launcher_stem and (name in {launcher_stem+'.pyc',launcher_stem+'.pyo'}
                         or name.startswith(launcher_stem+'.') and name.endswith(('.pyc','.pyo'))))
                     or 'libpython' in str(path).lower() or name.startswith('python'))
        require(not forbidden, 'native opened CPython, libpython, withdrawn source or matching bytecode')



def native_content(api, lines, pid, image, cwd, context, require):
    """Check the bounded single native PID against the frozen -y open census."""
    policy, family, launcher, extension = context
    require(policy.get('schema') == 'imre.compatibility.native-content-policy.v1' and family in policy.get('families',{}),
            'invalid native content policy')
    # These runs have one native PID. Do not extrapolate the policy across forks.
    require(not any(re.search(r'\b(?:clone|clone3|fork|vfork|io_uring_setup)\(', line) for line in lines),
            'native process or io_uring operation is unverified')
    fixture = policy['fixture']; root = Path(cwd) / fixture['root'].format(pid=pid)
    directories = {root / part for part in fixture['directories']} if family == 'glob' else set()
    aliases = {root/'sym3':root/'a/bcd', root/'sym3/efg':root/'a/bcd/efg'} if family == 'glob' else {}
    static = {Path(launcher).parent, Path(extension)}
    script_case = policy.get('script_cases',{}).get(family)
    if script_case:
        directory = script_case.get('compile_directory')
        require(directory in {'.','scripts'}, 'invalid script compile directory policy')
        static = {Path(launcher).parent.parent / directory}
    allowed_dirs = static | directories | ({Path(cwd)} if family == 'glob' else set())
    system = list(policy['system_common']) + (policy['logging_extra'] if family == 'logging' else [])
    if script_case and 'system' in script_case:
        system = script_case['system']
    expected_system = Counter({(x['lexical'],x['kernel'].format(pid=pid)):x['count'] for x in system})
    if family == 'logging': expected_system[('/etc/ld.so.cache','/etc/ld.so.cache')] = policy['families'][family]['ld_cache_reads']
    observed_system=Counter(); writes=Counter(); reads=Counter(); counts=Counter(); file_access=Counter()
    accesses = script_case['fixture_file_access'] if script_case else policy.get('fixture_file_access',[]) if family == 'ty-05' else []
    require(all(not Path(x['path']).is_absolute() and '..' not in Path(x['path']).parts for x in accesses), 'invalid script fixture path')
    expected_access=Counter({(str(Path(cwd)/x['path']),x['flags'],x['mode']):x['count'] for x in accesses})
    for event in api.successful_open_paths('\n'.join(lines),cwd,kernel_events=True):
        counts['opens'] += 1
        lexical, path, returned, flags = event['lexical'], event['path'], event['returned'], event['flags']
        require(event['pid'] == pid and event['operation'] in {'open','openat'}
                and not any(payload_alias(x,task=True) for x in (lexical,str(path),str(returned)))
                and '..' not in Path(lexical).parts, 'unauthorized native open operation, fd alias or traversal')
        if flags in policy['directory_flags'] and event['mode'] is None:
            require(path in allowed_dirs | set(aliases) and returned == aliases.get(path,path),
                    'unauthorized native directory open')
            counts['directories'] += 1
        elif family == 'glob' and path in {root / x for x in fixture['writes']}:
            require(returned == path and flags == fixture['write_flags'] and event['mode'] == fixture['write_mode'],
                    'unauthorized fixture write mode or target')
            writes[str(path.relative_to(root))] += 1
        elif family == 'glob' and path in directories:
            require(returned == path and flags == fixture['directory_read_flags'] and event['mode'] is None
                    and event['directory_witness'] == str(path), 'unauthorized fixture directory read without S_IFDIR')
            reads['' if path == root else str(path.relative_to(root))] += 1
        elif (str(path),flags,event['mode']) in expected_access:
            require(returned == path, 'unauthorized fixture file target')
            file_access[(str(path),flags,event['mode'])] += 1
        elif path == Path(image) and returned == path and flags == policy['readonly_flags'] and event['mode'] is None:
            counts['image_reads'] += 1
        else:
            pair=(str(path),str(returned))
            require(pair in expected_system and flags == policy['readonly_flags'] and event['mode'] is None,
                    'unauthorized native content open: '+lexical)
            observed_system[pair] += 1
    require({k:counts[k] for k in ('opens','directories','image_reads')} == {k:policy['families'][family][k] for k in ('opens','directories','image_reads')}
            and observed_system == expected_system, 'native content open census differs')
    expected=lambda names:Counter({name:fixture['each_count'] for name in names})
    require(writes == (expected(fixture['writes']) if family == 'glob' else Counter())
            and reads == (expected(fixture['directories']) if family == 'glob' else Counter()) and file_access == expected_access,
            'native fixture operation census differs')


def policy_sources(api, policy, require):
    for item in policy.get('sources',{}).values():
        require(item.get('repo') == 'https://github.com/python/cpython' and item.get('commit') == api.COHORT['cpython_commit'],
                'fixture policy source pin differs')
        path=api.repository_file(item['archive'],api.DATA_ROOT)
        validate_stock_source(path,item.get('member'),item,None,require)
        lines=stock_source_bytes(path,item.get('member'),item,require).splitlines(keepends=True)
        for witness in item['witnesses']:
            first,last=witness['first'],witness['last']
            require(1 <= first <= last <= len(lines) and sha(b''.join(lines[first-1:last])) == witness['sha256'],
                    'fixture source line witness differs')


def event_index(path, require):
    result = parse(path if isinstance(path,bytes) else path.read_bytes(), False)
    require(result['inventory_complete'] and result['summary'] is not None, 'raw unittest collection or summary incomplete')
    index = {}
    for row in result['originals']:
        key = (row['id'], row['occurrence'])
        require(key not in index and row['outcome'] in {'pass','fail','error','skip'}, 'raw unittest original missing a completed outcome')
        index[key] = row
    return index


def comparable_run(run, extension, compiler, require):
    """Compare the finite test configuration while allowing tier launch mechanics."""
    metas = run['runs']
    def cwd(meta):
        value = meta.get('cwd')
        require(isinstance(value, str) and value.startswith('/'), 'finite cohort cwd missing')
        return posixpath.normpath(value)
    def common(meta):
        env = meta.get('environment')
        require(isinstance(env, dict) and all(isinstance(k, str) and isinstance(v, str) for k, v in env.items()), 'finite cohort environment missing')
        return {k:v for k,v in env.items() if k not in {'PYTHONPATH','IMRE_PYTHON','IMREFLAGS'}}
    stock = metas['stock']; settings = common(stock)
    require('IMRE_PYTHON' not in stock['environment'] and 'IMREFLAGS' not in stock['environment'] and settings.get('PYTHONDONTWRITEBYTECODE') == '1', 'stock finite configuration contains native flags or permits bytecode')
    for tier, opt in (('O0','0'),('O3','3')):
        meta = metas[tier]; env = meta.get('environment', {})
        require(cwd(meta) == cwd(stock) and common(meta) == settings and meta.get('timeout_seconds') == stock.get('timeout_seconds') and isinstance(meta.get('timeout_seconds'), (int,float)) and meta['timeout_seconds'] > 0, tier+' finite cohort common configuration differs from stock')
        require(posixpath.normpath(env.get('PYTHONPATH','')) == posixpath.normpath(extension) and env.get('IMRE_PYTHON') == compiler and env.get('IMREFLAGS') == '--opt-level '+opt, tier+' finite cohort tier configuration differs')


SOURCE_PIN = '8571ca2dbecabf8d7863c369dc776611fd57d8ad'
SOURCE_MANIFEST = f'benchmarks/compatibility/revisions/{SOURCE_PIN}/controls/glob-logging-source-stage.json'
SOURCE_FAMILIES = frozenset({'glob', 'logging'})


def superseded_trace_refs(family):
    """Only the two canonical pre-recapture tier traces for this family."""
    if family not in SOURCE_FAMILIES: return frozenset()
    prefix = f'benchmarks/compatibility/revisions/{SOURCE_PIN}/evidence/{family}/strict'
    return frozenset(f'{prefix}/{tier}.trace' for tier in ('O0','O3'))


def validate_capture_archive(api, archive, family, entry):
    archived = archive.get('families',{}).get(family)
    superseded = superseded_trace_refs(family)
    api.require(isinstance(archived,dict) and superseded and superseded <= archived.keys(),
                'missing canonical superseded trace references')
    for loc,digest in archived.items():
        api.require(isinstance(digest,str) and api.HEX64.fullmatch(digest), 'invalid archived SHA-256')
        if loc in superseded: continue  # Historical digest only; never a selected artifact.
        api.require(api.binding_digest(entry,loc) == digest
                    and sha(api.repository_file(loc,api.DATA_ROOT).read_bytes()) == digest,
                    'original capture archive digest differs')


def git(root, *args):
    try:
        return subprocess.check_output(['git','--no-replace-objects',*args], cwd=root, stderr=subprocess.DEVNULL)
    except subprocess.CalledProcessError as exc:
        raise ValueError('source stage Git authority is unavailable or unreachable') from exc


@lru_cache(maxsize=None)
def git_manifest(root, commit, path, blob, digest):
    raw = git(root,'show','--no-textconv',f'{commit}:{path}')
    computed = hashlib.sha1(f'blob {len(raw)}\0'.encode()+raw).hexdigest()
    if computed != blob or sha(raw) != digest: raise ValueError('source-stage manifest Git blob or digest differs')
    return json.loads(raw)


def source_manifest(root, commit, blob, digest):
    return git_manifest(root,commit,SOURCE_MANIFEST,blob,digest)


def public_commit(api, commit):
    head = git(api.ROOT,'rev-parse','HEAD').decode().strip()
    remote = git(api.ROOT,'remote','get-url','origin').decode().strip()
    api.require(isinstance(commit,str) and api.HEX40.fullmatch(commit)
                and remote in {'https://github.com/astral-experiments/imre.git','git@github.com:astral-experiments/imre.git'}
                and commit != head and git(api.ROOT,'cat-file','-t',commit).strip() == b'commit',
                'source stage must be a distinct public commit')
    git(api.ROOT,'merge-base','--is-ancestor',commit,head)
    git(api.ROOT,'merge-base','--is-ancestor',commit,'refs/remotes/origin/main')


@lru_cache(maxsize=None)
def source_stage_authority(api, family, activation_digest):
    """Authenticate a newly activated family against an earlier public source stage."""
    require = api.require
    entry = api.activation_entry(family)
    anchor = entry.get('source_stage')
    require(isinstance(anchor, dict) and set(anchor) == {'commit','manifest','git_blob','sha256'}
            and isinstance(anchor['commit'], str) and api.HEX40.fullmatch(anchor['commit'])
            and anchor['manifest'] == SOURCE_MANIFEST and isinstance(anchor['git_blob'],str)
            and api.HEX40.fullmatch(anchor['git_blob']) and isinstance(anchor['sha256'],str)
            and api.HEX64.fullmatch(anchor['sha256']), 'missing fixed source-stage authority')
    commit = anchor['commit']; public_commit(api,commit)
    manifest = source_manifest(api.ROOT,commit,anchor['git_blob'],anchor['sha256'])
    require(manifest.get('schema') == 'imre.compatibility.source-stage.v1'
            and manifest.get('imre_revision') == api.COHORT['imre_revision']
            and isinstance(manifest.get('sources'),dict) and family in manifest.get('families',{}),
            'invalid source-stage manifest')
    frozen = manifest['families'][family]; binding_ref = frozen['binding']; staged = frozen['staged_receipt']
    receipt = f"benchmarks/compatibility/revisions/{api.COHORT['imre_revision']}/verified/stdlib/{family}.jsonl"
    expected_keys = {'adapter','receipt','staged_receipt','sources','files','source_stage'}
    if staged['originals'] < staged['frozen_originals']: expected_keys.add('staged_originals')
    require(set(entry) == expected_keys and entry.get('adapter') == 'raw-control-v1' and entry.get('receipt') == receipt
            and entry.get('staged_receipt') == staged['path'] and entry.get('sources') == {'binding':binding_ref['path']}
            and ('staged_originals' not in expected_keys or entry['staged_originals'] == staged['originals']),
            'activation entry differs from immutable source stage')
    fixed = {}
    for group in (manifest['sources'], frozen['evidence_files'],
                  {binding_ref['path']:binding_ref['sha256'], staged['path']:staged['sha256']}):
        for loc,digest in group.items():
            require(loc not in fixed or fixed[loc] == digest, 'inconsistent source-stage digest sets')
            fixed[loc] = digest
    files = entry.get('files')
    require(isinstance(files,dict) and set(files) == set(fixed)|{receipt}
            and all(files.get(loc)==digest for loc,digest in fixed.items()), 'activation file set differs from immutable source stage')
    for loc,digest in files.items():
        require(isinstance(digest,str) and api.HEX64.fullmatch(digest) and sha(api.repository_file(loc,api.ROOT).read_bytes()) == digest, 'source-stage file digest differs')
    binding = json.loads(api.repository_file(binding_ref['path'],api.DATA_ROOT).read_text())
    cohorts = binding.get('cohorts',{})
    require(cohorts and all(c.get('strict_source_control') == frozen['strict_source_control'] == 'native-content-v1'
                           for c in cohorts.values()), 'strict source control was removed or changed')
    artifacts = {loc for c in cohorts.values() for loc in c.get('artifacts',{}).values()}
    declared = frozen['artifact_files']
    require(isinstance(declared,list) and len(declared) == len(set(declared)) and artifacts == set(declared)
            and artifacts <= set(fixed), 'binding artifact set differs from immutable source stage')
    return manifest


@lru_cache(maxsize=None)
def load_binding(api, family, locator, digest, activation_digest, entry_json=None):
    require = api.require
    entry = json.loads(entry_json) if entry_json else api.activation_entry(family)
    path = api.bound_artifact(entry,locator) if entry_json else api.checked_file(api.repository_file(locator, api.DATA_ROOT), digest)
    if entry_json:
        from extensions.successor959_v1 import authority as successor
        binding=successor.effective_binding(api,entry)
    else:binding=json.loads(path.read_text())
    require(binding.get('schema') == ('imre.compatibility.successor-input.v1' if entry_json else 'imre.compatibility.raw-control.v1') and binding.get('family') == family, 'invalid normalized raw-control binding')
    cohorts, originals = binding.get('cohorts'), binding.get('originals')
    require(isinstance(cohorts, dict) and cohorts and isinstance(originals, list), 'missing launcher cohorts or original mapping')
    mapping = {}
    for row in originals:
        require(isinstance(row, dict) and isinstance(row.get('id'), str) and type(row.get('occurrence')) is int and row['occurrence'] >= 1, 'invalid original cohort key')
        key = (row['id'], row['occurrence']); cohort = row.get('launcher_cohort')
        require(key not in mapping and isinstance(cohort, str) and cohort in cohorts and isinstance(cohorts[cohort], dict), 'duplicate or unknown original launcher cohort')
        mapping[key] = cohort
    receipt = api.bound_artifact(entry,entry['receipt']) if entry_json else api.repository_file(entry.get('receipt'), api.DATA_ROOT)
    rows = [json.loads(line) for line in receipt.read_text().splitlines()]
    keys = [(row['id'], row['occurrence']) for row in rows]
    require(len(keys) == len(set(keys)) and set(keys) == set(mapping) and set(mapping.values()) == set(cohorts), 'launcher cohort mapping must cover every receipt original exactly once')
    for row in rows:
        kind = cohorts[mapping[(row['id'], row['occurrence'])]].get('kind', 'finite')
        require(kind in {'finite','finite-959','finite-selected-v1','stock-only'} and (kind != 'stock-only' or row.get('paired') is False), 'credited original cannot use a stock-only cohort')
    return binding, mapping


GENERATED_WITNESS_SHA = '5facddf547bed1baedeaa19544ed2f6845f6234d59b64b25df3a2fb991344b95'
BUILD_LOG_SHA = {'ruff-a':'1ab43a2224644ef174b090e9d9e673d29388280a1f13954c9f1891ce64db6758',
                 'ruff-b':'b82d18267aca6539a449f16a006ac972cb4db41715c512f055ee438bf07eb169'}


def depfile_custody(api, lane, revision, worktree, product, build_log, depfiles, witness, require):
    """Join exact depfile outputs and generated inputs to the retrospective donor audit."""
    require(sha(witness) == GENERATED_WITNESS_SHA and sha(build_log) == BUILD_LOG_SHA.get(lane),
            'generated-input witness or build log provenance differs')
    audit = json.loads(witness); donors = [x for x in audit.get('donors',[]) if x.get('lane') == lane]
    require(audit.get('compiler_revision') == revision and len(donors) == 1, 'generated-input donor differs')
    donor = donors[0]; inputs = donor.get('inputs',[])
    require(donor.get('compiler_sha256') == product.get('imre',{}).get('sha256')
            and donor.get('runtime_sha256') == product.get('libimre_runtime.a',{}).get('sha256')
            and len(inputs) == 7 and len({x['path'] for x in inputs}) == 7
            and all(api.HEX64.fullmatch(x.get('sha256','')) and type(x.get('bytes')) is int and x['bytes'] > 0 for x in inputs),
            'generated-input content hashes or build output tuple differs')
    tree = set(git(api.ROOT,'ls-tree','-r','--name-only',revision).decode().splitlines()); prefix=worktree.rstrip('/')+'/'
    expected = {'imre.d':{x['path'] for x in inputs[:2]},'libimre_runtime.d':{x['path'] for x in inputs[2:]}}
    require(set(depfiles) == set(expected), 'depfile set differs')
    for name, raw in depfiles.items():
        require(sha(raw) == product.get(name,{}).get('sha256'), 'depfile digest differs')
        output, sep, body = raw.decode().partition(': '); entries=body.split()
        paths=[x[len(prefix):] for x in entries if x.startswith(prefix)]; generated=[x for x in entries if not x.startswith(prefix)]
        require(sep and output == '/workspace/imre/target/release/'+('imre' if name=='imre.d' else 'libimre_runtime.a')
                and paths and len(paths)==len(set(paths)) and all(posixpath.normpath(x) in tree for x in paths)
                and len(generated)==len(set(generated)) and set(generated)==expected[name], 'depfile source or generated path custody differs')


def custody_tuple(api, setup, build_log, depfiles, witness, require):
    """Check a clean revision build and its depfile source paths against Git."""
    revision = api.COHORT['imre_revision']; product = setup.get('product', {})
    require(setup.get('compiler_revision') == revision and setup.get('tree') == git(api.ROOT,'rev-parse',revision+'^{tree}').decode().strip()
            and setup.get('status_porcelain') == '' and setup.get('origin') == 'https://github.com/astral-experiments/imre.git'
            and 'cargo build' in setup.get('build_command','') and '--release' in setup['build_command'] and '--locked' in setup['build_command']
            and all(x in build_log for x in ('Compiling imre-compiler','Compiling imre-runtime','Finished `release`')), 'clean compiler/runtime build custody differs')
    depfile_custody(api,'ruff-b',revision,setup['worktree'],product,build_log.encode(),depfiles,witness,require)
    result={k:product.get(k,{}).get('sha256') for k in ('imre','libimre_runtime.a')}
    require(all(isinstance(v,str) and api.HEX64.fullmatch(v) for v in result.values()), 'invalid compiler/runtime tuple')
    return result


def bound_artifacts(api, entry, artifacts):
    api.require(isinstance(artifacts,dict), 'missing raw-control artifacts')
    def file(role):
        loc=artifacts.get(role);api.require(isinstance(loc,str), 'missing raw-control role: '+role)
        return api.bound_artifact(entry,loc) if entry.get('adapter')=='raw-control-v2' else api.checked_file(api.repository_file(loc,api.DATA_ROOT),api.binding_digest(entry,loc))
    return file


def finite_gate_delta(positive, negative, operation, require):
    """Apply an exact, bound source insertion without parsing or rewriting upstream code."""
    require(isinstance(operation,dict) and operation.get('op') in {'insert-after-unique-line','insert-after-first-line'} and isinstance(operation.get('line'),str) and sha(negative)==operation.get('negative_sha256'), 'undeclared finite launcher operation/hash')
    line=operation['line'].encode();lines=positive.splitlines(keepends=True)
    if operation['op']=='insert-after-unique-line':
        require(set(operation)=={'op','after','line','negative_sha256'} and isinstance(operation['after'],str), 'invalid unique-line insertion');marker=operation['after'].encode();require(lines.count(marker)==1 and marker.endswith(b'\n'), 'finite insertion marker is not unique');expected=positive.replace(marker,marker+line,1)
    else:
        require(set(operation)=={'op','line','negative_sha256'} and lines and lines[0].endswith(b'\n'), 'invalid first-line insertion');expected=lines[0]+line+b''.join(lines[1:])
    require(negative==expected, 'finite launcher operation/delta differs');return line


def finite_core(api,family,source,binding,file,data,text,obj,context):
    """One finite gate, tier, inventory, effects and withdrawal acceptance path."""
    require=api.require;legacy=context['legacy'];run=context['run'];positive=data('positive_launcher');builds={t:obj(t+'_build_json') for t in ('O0','O3')};command=builds['O0']['argv'];compiler=command[0];runtime=api.runtime_argument(command);extension=api.option(command,'--extension-path');launcher=command[2];comparable_run(run,extension,compiler,require)
    withdrawal=context['withdrawal'];tree_role=withdrawal['tree_relative_to_extension'];archive_role=withdrawal['archive_relative_to_launcher'];suffix=withdrawal['tree_stash_suffix'];require(tree_role=='test' and Path(archive_role).name==archive_role and archive_role.endswith('.py') and suffix.startswith('.') and '/' not in suffix, 'finite withdrawal roles escape their declared source graph');archive=str(Path(launcher).parent/archive_role)
    stock=run['runs']['stock'];require(stock['environment'].get('PYTHONPATH')==extension and stock['argv'][1:]==['-B',launcher], 'finite stock and native launcher/configuration differ')
    gate_role=lambda tier,mode,suffix:(mode+'_'+suffix if legacy else 'gate_'+tier+'_'+mode+'_'+suffix)
    for tier in context['gate_tiers']:
        cmd=builds[tier]['argv']
        for mode in ('direct','imported'):
            role=lambda suffix:gate_role(tier,mode,suffix);meta=obj(role('meta'));trace=text(role('trace'));first,child=execs(trace.splitlines(),require);gated=child[3];diagnostic=text(role('stderr'));negative=data(mode+'_launcher');operation=dict(context['operations'][mode]);operation.update({'negative_sha256':meta['negative_launcher_sha256']} if legacy else {});line=finite_gate_delta(positive,negative,operation,require)
            expected=(b'\nstatus: int = True\n' if legacy else b'status: int = True\n') if mode=='direct' else ('import '+context['poison_module']+'\n').encode();require(line==expected and sha(negative)==meta['negative_launcher_sha256'] and sha(data(role('trace')))==meta['trace_sha256'] and sha(data(role('stderr')))==meta['stderr_sha256'] and data(role('stdout'))==b'' and api.comparable_build(gated)==api.comparable_build(cmd), 'finite gate source, raw evidence or positive build differs')
            if legacy:require(meta.get('controller_sha256')==context['controller_sha256'] and meta.get('compiler_sha256')==context['product']['imre'] and meta.get('runtime_sha256')==context['product']['libimre_runtime.a'] and meta.get('exit_code')==0 and meta.get('image_exists') is False, 'legacy finite gate custody differs')
            else:require(meta['original_launcher_sha256']==sha(positive) and meta['positive_build_metadata_sha256']==sha(data(tier+'_build_json')) and meta['positive_build_argv']==cmd and meta['build_argv']==gated and meta['probe_sha256']==context['probe']['executable_sha256'] and meta['probe_source_sha256']==context['probe']['source_sha256'] and first[2]==context['probe']['executable'] and meta['supervisor_exit_code']==0 and meta['compiler_child_exit_code']==1 and meta['image_absent_before'] is True and meta['image_absent_after'] is True and exec_environment(trace.splitlines()[child[0]],require)==meta['actual_compiler_child_environment']==builds[tier]['environment'], 'finite gate child, no-image or identity differs')
            if context.get('identity'):finite_identity_gate(api,tier,mode,context,builds[tier],meta,child,first,trace,data)
            probe=context['gate_probe'] if legacy else first[2]
            if mode=='direct':trace_direct_gate(trace,gated,probe,diagnostic,require,expected_exit=1)
            else:
                poison=data('poison');path=str(Path(launcher).parent/(context['poison_module']+'.py')) if legacy else meta['poison_path'];require(poison==b'status: int = True\n' and sha(poison)==meta['poison_sha256'] and path==str(Path(launcher).parent/(context['poison_module']+'.py')), 'finite first-party poison differs');trace_gate(trace,gated,probe,path,diagnostic,require,expected_exit=1)
    inventories={};paths={};effects=[];outputs=[]
    for tier in api.TIERS:
        meta=run['runs'][tier];capture=obj(tier+'_meta');raw=data(tier+'_stderr');stdout=data(tier+'_stdout');before=data(tier+'_effects_before');after=data(tier+'_effects_after');require(meta.get('exit_code')==0 and meta.get('timed_out') is False and sha(raw)==meta.get('stderr_sha256') and sha(stdout)==meta.get('stdout_sha256') and stdout==b'' and before==after and sha(before)==meta.get('effects_before_sha256') and sha(after)==meta.get('effects_after_sha256'), tier+' finite observation or effects differ');effects.append(b'no changes' if context.get('identity') else before);outputs.append(raw);inventories[tier]=event_index(raw,require);paths[tier]=context['roles'][tier+'_stderr'] if context.get('identity') else file(tier+'_stderr').relative_to(api.ROOT).as_posix()
        if tier=='stock':
            require(capture==meta,'finite stock capture differs')
            if not legacy:
                trace=text('stock_trace');lines=trace.splitlines();child=exec_sequence(lines,require,1)[0];identity=context['stock_interpreter'];expected_source=str(Path(extension)/Path(source['path']).relative_to('Lib'));events=api.successful_open_paths(trace,meta['cwd'],kernel_events=True);required={launcher,expected_source};opened={str(e['path']) for e in events if e['pid']==child[1] and e['path']==e['returned'] and e['flags'] in {'O_RDONLY','O_RDONLY|O_CLOEXEC'} and 'AT_FDCWD<'+meta['cwd']+'>' in lines[e['line']-1]};require(sha(data('stock_trace'))==meta['trace_sha256'] and child[2]==identity['path']==meta['argv'][0] and child[3]==meta['argv'] and exec_environment(lines[child[0]],require)==meta['environment'] and required<=opened, 'finite stock exec, cwd, environment, launcher or source differs');find(lines,rf'^{child[1]}\s+exit_group\(0\)',child[0]+1)
                if context.get('identity'):finite_identity_stock(api,context,meta,child,data)
            continue
        build=builds[tier];argv=build['argv'];api.matched_build(argv,tier,compiler,runtime);normalized=list(argv);normalized[normalized.index('--opt-level')+1]='0';output=api.option(argv,'-o');image=meta['argv'][0];require((output==image or context.get('identity') and context['join_modes'][tier]=='same-image-content-join' and build['image_sha256']==meta['image_sha256']) and api.comparable_build(normalized)==api.comparable_build(command) and build.get('exit_code')==0 and build['image_sha256']==meta['image_sha256']==meta['before'][image]==meta['after'][image] and meta['argv']==[image] and meta['restored'] is True, tier+' finite build/image differs')
        if legacy:require(build.get('compiler_sha256')==context['product']['imre'] and build.get('runtime_sha256')==context['product']['libimre_runtime.a'] and build.get('controller_sha256')==context['controller_sha256'] and capture.get('argv')==meta.get('capture_argv') and all(capture.get(k)==meta.get(k) for k in ('environment','supervisor_environment','controller_sha256','trace_sha256','image_sha256','exit_code','timed_out','cwd','timeout_seconds')) and meta.get('controller_sha256')==context['collector_sha256'] and meta.get('supervisor_sha256')==context['withdraw_sha256'],tier+' legacy finite custody differs')
        else:require(capture==meta and build.get('source_sha256')==source['sha256'] and build.get('launcher_sha256')==sha(positive) and meta['supervisor_sha256']==context['probe']['executable_sha256'], tier+' finite source or supervisor differs')
        if context.get('identity'):finite_identity_build(api,tier,context,build,meta,data,text)
        trace=text(tier+'_trace');lines=trace.splitlines();parent,child=execs(lines,require);expected_env={**meta['environment'],'IMRE_WITHDRAW_LAUNCHER':launcher,'IMRE_WITHDRAW_ARCHIVE':archive};require(sha(data(tier+'_trace'))==meta['trace_sha256'] and child[2]==image and exec_environment(lines[parent[0]],require)==expected_env and exec_environment(lines[child[0]],require)==meta['environment'] and not any(k.startswith('IMRE_WITHDRAW_') for k in meta['environment']) and (not legacy or meta['supervisor_environment']==expected_env),tier+' finite withdrawal environment differs');probe=context['withdraw_probe'] if legacy else context['probe']['executable'];path=Path(source['path']);require(path.parts[:2]==('Lib','test') and path.suffix=='.py' and '..' not in path.parts, 'finite original leaf escapes test tree');tree=str(Path(extension)/tree_role);leaf=str(Path(extension)/path.relative_to('Lib'));trace_withdraw(api,trace,image,probe,tree,tree+suffix,leaf,meta['cwd'],require,(launcher,archive),content_policy=(obj('policy'),family,launcher,extension))
    require(effects[0]==effects[1]==effects[2] and outputs[0]==outputs[1]==outputs[2] and all(set(x)==set(inventories['stock']) for x in inventories.values()) and (context.get('observed') is None or len(inventories['stock'])==context['observed']), 'finite original IDs, occurrences, output or effects differ')
    return {**run,'platform':context['platform'],'source':source},inventories,paths


def adapt_finite_959(api,family,binding,source,file,data,text,obj):
    """Normalize the immutable legacy capture into the shared finite core."""
    require=api.require;run=obj('run');setup=obj('setup');custody=obj('custody_setup');require(api.COHORT['imre_revision']=='95908fb0acadc2a67304a8080e584deaef395746' and family=='fnmatch' and binding.get('strict_source_control')=='native-content-v1' and run.get('family')==family and run.get('revision')==api.COHORT['imre_revision'] and setup.get('pin')==run['revision'] and setup.get('tree')==custody.get('tree') and binding.get('platform',{}).get('os')=='linux', 'legacy finite source or strict mode differs');validate_stock_source(file('stock_source'),None,source,None,require);require(setup.get('fixture_sha256')==source['sha256'] and setup.get('fixture_blob')==source['git_blob'] and sha(data('positive_launcher'))==setup.get('launcher_sha256') and setup['stock_version'].startswith('Python '+api.COHORT['cpython_version']+' '),'legacy finite fixture/launcher/stock differs')
    product=custody_tuple(api,custody,text('custody_build_log'),{n:data(role) for n,role in (('imre.d','cli_depfile'),('libimre_runtime.d','runtime_depfile'))},data('generated_witness'),require);require(product=={k:setup['product'][k] for k in product} and obj('transfer').get('cli_sha256')==product['imre'] and obj('transfer').get('runtime_sha256')==product['libimre_runtime.a'], 'legacy finite transferred tuple differs');gateprobe=binding.get('gate_probe');withdrawprobe=binding.get('withdraw_probe');require(run['gate_evidence'].get('probe_sha256')==setup.get('probe_sha256') and setup['probe_sha256'] in text('gate_probe_build') and sha(data('gate_probe_source')) in text('gate_probe_build') and run['gate_evidence'].get('controller_sha256')==sha(data('original_controller')) and obj('withdraw_probe_build').get('executable_sha256')==run['withdrawal_evidence'].get('probe_sha256') and obj('withdraw_probe_build').get('source_sha256')==sha(data('withdraw_probe_source'))==run['withdrawal_evidence'].get('probe_source_sha256') and run['withdrawal_evidence'].get('controller_sha256')==sha(data('collector')) and gateprobe!=withdrawprobe, 'legacy as-used finite helpers differ')
    return finite_core(api,family,source,binding,file,data,text,obj,{'legacy':True,'run':run,'platform':binding['platform'],'gate_tiers':('O0',),'poison_module':'fnmatch_control_poison','operations':{'direct':{'op':'insert-after-unique-line','after':'import unittest\n','line':'\nstatus: int = True\n'},'imported':{'op':'insert-after-first-line','line':'import fnmatch_control_poison\n'}},'product':product,'controller_sha256':sha(data('original_controller')),'collector_sha256':sha(data('collector')),'gate_probe':gateprobe,'withdraw_probe':withdrawprobe,'withdraw_sha256':run['withdrawal_evidence']['probe_sha256'],'withdrawal':{'tree_relative_to_extension':'test','tree_stash_suffix':'.raw-control-stash','archive_relative_to_launcher':'stock.source.py'}})


def finite_identity_normalize(api,manifest,raw_obj,data):
    """Normalize sealed capture schemas into the common finite gate/tier protocol."""
    def obj(role):
        raw=raw_obj(role)
        if role=='stock_meta':
            controller=data('stock_controller');matches=re.findall(rb'p\.communicate\(timeout=([1-9][0-9]*)\)',controller);api.require(len(matches)==1,'held stock timeout control missing');return {**raw,'timed_out':False,'timeout_seconds':int(matches[0])}
        if role=='O0_meta':return {**raw,'before':raw['source_before'],'after':raw['source_after'],'restored':raw['source_before']==raw['source_after'],'supervisor_sha256':raw['probe_sha256']}
        if role in {'O0_build_json','O3_build_json'}:return {**raw,'argv':raw['build_argv'],'exit_code':raw['compiler_exit_code'],'image_sha256':raw['new_image_sha256']}
        if role.startswith('gate_') and role.endswith('_meta'):
            _,tier,mode,_=role.split('_');prior=raw_obj('prior_'+role);build=raw_obj(tier+'_build_json');return {**prior,**raw,'negative_launcher_sha256':raw['source_negative_sha256'],'original_launcher_sha256':prior['original_launcher_sha256'],'positive_build_metadata_sha256':sha(data(tier+'_build_json')),'positive_build_argv':build['build_argv'],'actual_compiler_child_environment':raw['actual_compiler_environment'],'poison_path':prior['poison_path']}
        return raw
    run=raw_obj('run');run={**run,'runs':{**run['runs'],'stock':obj('stock_meta'),'O0':obj('O0_meta')}};return obj,run


def finite_identity_stock(api,context,meta,child,data):
    check=api.require;pre=meta['held_fd_pre'];post=meta['held_fd_post'];sample=meta['proc_exe_sample'];expected=context['stock_interpreter'];check(pre==post and pre['sha256']==expected['sha256'] and pre['fstat']==meta['path_post_fstat']==sample['fstat'] and sample['pid']==child[1] and sample['target']==expected['path']==child[2] and meta['proc_exe_sample_error'] is None and meta['start_utc_ns']<meta['finish_utc_ns'] and meta['controller_sha256']==sha(data('stock_controller')) and meta['original_stock_metadata_sha256']==sha(data('prior_stock_meta')) and meta['ordered_streams_and_effects_equal'] is True and len(meta['environment'])==6 and sha(data('stock_stderr'))==sha(data('prior_stock_stderr')) and data('stock_effects_before')==data('stock_effects_after'), 'held stock FD, live proc executable, provenance or observations differ')


def finite_identity_build(api,tier,context,build,meta,data,text):
    check=api.require;raw=context['raw_obj'](tier+'_build_json');argv=raw['build_argv'];product=context['product'];paths={'cli':argv[0],'runtime':api.runtime_argument(argv),'clang':api.option(argv,'--clang')};pre=raw['fd_before'];post=raw['fd_after'];check(set(pre)==set(post)==set(paths) and pre==post and all(pre[k]['path']==paths[k] and Path(pre[k]['fd_realpath']).is_absolute() and pre[k]['sha256']==product[k] and pre[k]['fstat']==pre[k]['path_stat'] and pre[k]['read_fstat_unchanged'] is True for k in paths),'positive build substituted tool path/hash or changed held FD')
    trace=text(tier+'_positive_trace');lines=trace.splitlines();m=EXEC.fullmatch(lines[0]);check(m is not None and m.group(1)==str(raw['compiler_pid']) and json.loads(m.group(2))==raw['actual_compiler_path']==argv[0] and json.loads(m.group(3))==raw['actual_compiler_argv']==argv and exec_environment(lines[0],check)==raw['actual_compiler_environment']==raw['environment']==build['environment'] and raw['trace_sha256']==sha(data(tier+'_positive_trace')) and raw['controller_sha256']==sha(data('build_controller')) and raw['compiler_exit_code']==0 and raw['strace_exit_code']==0 and raw['timed_out'] is False and raw['start_utc_ns']<raw['finish_utc_ns'] and raw['positive_build_metadata_sha256']==sha(data('prior_'+tier+'_build_json')) and raw['source_sha256']==context['source']['sha256'] and raw['launcher_sha256']==sha(data('positive_launcher')) and raw['new_image_sha256']==meta['image_sha256'] and sha(data(tier+'_positive_stdout'))==raw['stdout_sha256'] and sha(data(tier+'_positive_stderr'))==raw['stderr_sha256'] and data(tier+'_positive_effects_before')==data(tier+'_positive_effects_after') and sha(data(tier+'_positive_effects_before'))==raw['effects_before_sha256'] and sha(data(tier+'_positive_effects_after'))==raw['effects_after_sha256'],'positive raw compiler exec/env, source, image or capture differs')
    compiler=str(raw['compiler_pid']);compiler_exit=find(lines,rf'^{compiler}\s+exit_group\(0\)',1);output=api.option(argv,'-o');linkers=[]
    for i,line in enumerate(lines[1:],1):
        event=EXEC.fullmatch(line)
        if not event or event[1]==compiler:continue
        args=json.loads(event[3])
        if output not in args:continue
        try:
            pending=find(lines,rf'^{compiler}\s+wait4\({event[1]},',i+1);waited=exits(lines,compiler,event[1],0,pending+1)
        except ValueError:continue
        if waited<compiler_exit:linkers.append((line,event,args))
    check(len(linkers)==1,'positive build lacks one directly reaped linker child');line,event,args=linkers[0]
    check(json.loads(event[2])==args[0]==pre['clang']['path'] and args.count(output)==1 and api.option(args,'-o')==output and args.count(pre['runtime']['path'])==1 and exec_environment(line,check)==raw['environment'],'positive linker executable, runtime archive or environment differs')
    mode=context['join_modes'][tier]
    if mode=='fresh-image-run':check(raw['new_image_sha256']!=raw['old_image_sha256'] and meta['argv']==[api.option(argv,'-o')] and meta['controller_sha256']==sha(data('native_controller')) and meta['image_identity_before_after'] is True,'fresh image must use its own native withdrawal/run')
    elif mode=='same-image-content-join':check(raw['new_image_sha256']==raw['old_image_sha256']==meta['image_sha256'] and meta['argv']==context['raw_obj']('prior_'+tier+'_meta')['argv'] and raw['elf_identical'] is True,'saved native content join differs')
    else:check(False,'undeclared native image join')


def finite_identity_gate(api,tier,mode,context,build,meta,child,first,trace,data):
    check=api.require;raw=context['raw_obj']('gate_'+tier+'_'+mode+'_meta');br=context['raw_obj']('gate_bracket');positive=context['raw_obj'](tier+'_build_json');pre=br['fd_before'];post=br['fd_after'];product=context['product'];argv=child[3];paths={'cli':argv[0],'runtime':api.runtime_argument(argv),'clang':api.option(argv,'--clang')};check(br['fd_unchanged'] is True and pre==post and set(pre)==set(paths) and all(pre[k]['path']==paths[k] and pre[k]['fd_realpath']==positive['fd_before'][k]['fd_realpath'] and pre[k]['sha256']==product[k] and raw['fd_fstat_before'][k]==raw['fd_fstat_after'][k]==pre[k]['fstat'] and all(positive['fd_before'][k]['fstat'].get(field)==value for field,value in pre[k]['fstat'].items()) for k in paths),'gate held FD bracket, compiler tuple or fstat differs')
    records=[x for x in br['controls'] if x['tier']==tier and x['kind']==mode];check(len(records)==1 and records[0]['record_sha256']==sha(data('gate_'+tier+'_'+mode+'_meta')) and br['controller_sha256']==raw['controller_sha256']==sha(data('gate_controller')) and raw['corrected_gate_record_sha256']==sha(data('prior_gate_'+tier+'_'+mode+'_meta')) and raw['actual_compiler_path']==child[2]==paths['cli'] and raw['actual_compiler_argv']==raw['build_argv']==argv and raw['actual_compiler_environment']==positive['environment']==exec_environment(trace.splitlines()[child[0]],check) and api.comparable_build(argv)==api.comparable_build(positive['build_argv']) and raw['compiler_child_exit_code']==1 and raw['supervisor_exit_code']==0 and raw['image_absent_before'] is True and raw['image_absent_after'] is True and raw['source_negative_sha256']==meta['negative_launcher_sha256'],'gate raw compiler, environment, graph or no-image proof differs')


def adapt_selected_finite(api,family,manifest,binding,source,file,data,text,obj,entry):
    """Normalize a versioned semantic role binding into the same finite core."""
    from extensions.successor959_v1 import authority as successor
    require=api.require;raw_obj=obj;obj,run=finite_identity_normalize(api,manifest,raw_obj,data);assessment=obj('assessment');require(source==manifest['source'] and binding['source_leaf']==source['path'] and run['family']==family and run['revision']==assessment['revision']==api.COHORT['imre_revision'] and assessment['canonical_denominator']==manifest['canonical_denominator'] and assessment['observed_originals']==manifest['observed_originals'] and assessment['other_originals_not_run']==manifest['other_originals_not_run'], 'finite source/denominator/revision differs');validate_stock_source(file('stock_source'),None,source,None,require);successor.donor(api,entry,assessment['product']);require(sha(data('positive_launcher'))==assessment['launcher_sha256'] and assessment['stock_python_version']=='Python '+api.COHORT['cpython_version'], 'finite launcher/stock identity differs');identity=binding['stock_interpreter'];expected=successor.predecessor_stock(api);require((identity['sha256'],identity['version'])==(assessment['stock_python_sha256'],assessment['stock_python_version'])==expected and Path(identity['path']).is_absolute(), 'finite stock identity differs from public predecessor');probe=obj('gate_probe_build');require(probe['source_sha256']==sha(data('gate_probe_source')), 'finite gate probe source differs')
    return finite_core(api,family,source,binding,file,data,text,obj,{'legacy':False,'run':run,'platform':dict(api.COHORT['platform']),'gate_tiers':('O0','O3'),'poison_module':binding['poison_module'],'operations':binding['gate_operations'],'probe':probe,'observed':manifest['observed_originals'],'withdrawal':binding['withdrawal'],'stock_interpreter':identity,'identity':manifest['identity'],'roles':manifest['roles'],'join_modes':manifest['identity']['join_modes'],'raw_obj':raw_obj,'product':assessment['product'],'source':source})


@lru_cache(maxsize=None)
def adapt(api, family, locator, digest, source_json, activation_digest, cohort, entry_json=None):
    require = api.require
    entry = json.loads(entry_json) if entry_json else api.activation_entry(family)
    manifest, mapping = load_binding(api, family, locator, digest, activation_digest, entry_json)
    binding = manifest['cohorts'][cohort]
    kind = binding.get('kind', 'finite')
    artifacts = manifest['roles'] if kind=='finite-selected-v1' else binding.get('artifacts'); file = bound_artifacts(api,entry,artifacts)
    def data(role):
        if entry_json:
            loc=manifest['base_roles'][role.removeprefix('prior_')] if role.startswith('prior_') else artifacts[role]
            return api.bound_bytes(entry,loc)
        return file(role).read_bytes()
    def text(role): return data(role).decode()
    def obj(role): return json.loads(text(role))
    if kind=='finite-selected-v1':return adapt_selected_finite(api,family,manifest,binding,json.loads(source_json),file,data,text,obj,entry)
    if kind == 'finite-959':
        return adapt_finite_959(api,family,binding,json.loads(source_json),file,data,text,obj)
    run, gate, source = obj('run'), obj('gate'), json.loads(source_json)
    revision = api.COHORT['imre_revision']
    head = text('head').splitlines()
    require(run.get('family') == gate.get('family') == family
        and run.get('revision') == revision
        and run.get('source') == source
        and run.get('platform') == binding.get('platform')
        and all(run['platform'].get(k)==v for k,v in api.COHORT['platform'].items()), 'source, platform or revision differs')
    require(text('platform').strip() == f"Linux {run['platform'].get('release')} {run['platform'].get('arch')}", 'raw Cloud platform differs')
    build = gate.get('build', {})
    require(head == [revision, build.get('tree')]
        and build.get('head') == revision
        and text('clean') == ''
        and 'Compiling imre-compiler' in text('cargo')
        and 'Compiling imre-runtime' in text('cargo')
        and build.get('result') in text('cargo')
        and build.get('command', [])[:5] == ['cargo','build','--offline','--release','--locked'], 'raw clean matched compiler/runtime build missing')
    command = gate.get('command')
    require(isinstance(command,list) and len(command)>3, 'missing gate command')
    compiler, runtime = command[0], api.runtime_argument(command)
    api.matched_build(command,'O0',compiler,runtime)
    positive, negative, poison = data('positive_launcher'), data('negative_launcher'), data('poison')
    poison_path = gate.get('first_party_module')
    require(isinstance(poison_path,str)
        and Path(poison_path).parent == Path(command[2]).parent
        and Path(poison_path).name == file('poison').name, 'poison is not first-party')
    lines = positive.splitlines(keepends=True)
    injection = f'import {Path(poison_path).stem}\n'.encode()
    require(lines
        and negative == lines[0]+injection+b''.join(lines[1:])
        and poison == b'status: int = True\n'
        and sha(positive)==run.get('launcher_sha256')
        and sha(negative)==gate.get('launcher_sha256')
        and sha(poison)==gate.get('first_party_module_sha256'), 'positive/negative launcher code delta differs')
    gate_sums = sha_lines(text('gate_files'))
    pos_sums = sha_lines(text('positive_hash'))
    require(gate_sums == {command[2]:sha(negative),poison_path:sha(poison)} and pos_sums == {command[2]:sha(positive)}, 'raw launcher/poison hashes differ')
    module = api.source_module(source['path'])
    require(f'import {module} as ' in positive.decode(), 'launcher does not import original source')
    stock_meta = run['runs']['stock']
    stock_cmd = stock_meta.get('argv')
    require(isinstance(stock_cmd,list)
        and stock_cmd[1:] == ['-B',command[2]]
        and text('stock_version').strip()==run.get('stock_version')
        and text('stock_version').startswith('Python '+api.COHORT['cpython_version']+' '), 'stock interpreter version or invocation differs')
    stock_launcher = data('stock_launcher')
    expected_stock_sha = stock_meta.get('fresh_stock_launcher_sha256',run['launcher_sha256'])
    require(sha(stock_launcher)==expected_stock_sha and f'import {module} as ' in stock_launcher.decode(), 'stock launcher archive differs')
    if kind == 'finite':
        require(stock_launcher == positive and expected_stock_sha == run['launcher_sha256'], 'credited finite cohort stock launcher bytes differ from native')
    capture = obj('stock_capture') if 'stock_capture' in artifacts else None
    validate_stock_source(file('stock_source'), binding.get('stock_source_member'), source, capture, require)
    if capture is not None:
        require(all(capture.get(k) == stock_meta.get(k) for k in ('argv','cwd','environment','timeout_seconds','exit_code','timed_out','stderr_sha256','stdout_sha256','collection','inventory_complete','summary')) and capture.get('launcher_sha256') == sha(stock_launcher) and capture.get('python_sha256') == run.get('stock_python_sha256') and capture.get('version') == run.get('stock_version') and sha(data('stock_stdout')) == stock_meta.get('stdout_sha256'), 'fresh stock capture metadata or raw stdout differs')
    opt = api.option(command,'--extension-path')
    if kind == 'finite':
        comparable_run(run, opt, compiler, require)
    tree = str(Path(opt)/'test')
    leaf = str(Path(opt)/source['path'].removeprefix('Lib/'))
    stash = tree+'.raw-control-stash'
    probe = binding.get('probe')
    stock_lib = stock_meta.get('environment',{}).get('PYTHONPATH','')
    stock_leaf = str(Path(stock_lib)/source['path'].removeprefix('Lib/'))
    require(source['commit'] in stock_lib, 'stock source invocation is not pinned to the immutable commit')
    sums = sha_lines(text('identities'))
    stock_sums = sha_lines(text('stock_identity')) if 'stock_identity' in artifacts else sums
    require(isinstance(probe,str)
        and sums.get(compiler)==build.get('cli_sha256')
        and sums.get(runtime)==build.get('runtime_sha256')
        and stock_sums.get(stock_cmd[0])==run.get('stock_python_sha256')
        and sums.get(leaf)==sums.get(stock_leaf)==source['sha256']
        and bool(api.HEX64.fullmatch(sums.get(probe,'')))
        and sha_lines(text('restored'))=={leaf:source['sha256']}, 'raw compiler/runtime, CPython, source, probe or restoration identity missing')
    probe_build = text('probe_build')
    require(sha(data('probe_source')) in probe_build
        and sums[probe] in probe_build
        and 'gcc -O2' in probe_build
        and 'cmp ' in probe_build, 'supervisor source/build identity missing')
    collector = text('collector')
    mode = binding.get('strict_source_control')
    content = mode == 'native-content-v1'
    syscalls = 'execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,clone,clone3,fork,vfork'
    if content:
        require("SYSCALLS='"+syscalls+"'" in collector and "'-f','-y'" in collector and "'-s','512'" in collector,
                'run-only trace collection lacks kernel path or syscall coverage')
        archive=obj('archive'); policy=obj('policy')
        require(archive.get('schema') == 'imre.compatibility.capture-archive.v1' and family in archive.get('families',{})
                and archive['families'][family].get(artifacts['original_binding']) == sha(data('original_binding'))
                and archive['families'][family].get(artifacts['original_collector']) == sha(data('original_collector')),
                'original capture archive or as-used controller differs')
        validate_capture_archive(api,archive,family,entry)
        require(set(policy.get('sources',{})) == {'glob','os_helper'}
                and policy['sources']['glob']['archive'] == artifacts.get('policy_glob_source')
                and policy['sources']['os_helper']['archive'] == artifacts.get('policy_helper_source'),
                'fixture policy source artifacts differ')
        policy_sources(api,policy,require)
    else:
        require('trace=execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,exit_group' in collector
            and "'-f'" in collector and "'-s','512'" in collector, 'raw trace collection lacks syscall or descendant coverage')
    trace_gate(text('gate_trace'),command,probe,poison_path,text('gate_stderr'),require)
    require(data('gate_stdout')==b'', 'unexpected gate stdout')
    extra_sources = ()
    if mode in {'direct-and-withdraw-v1','native-content-v1'}:
        direct = obj('direct_command'); direct_cmd = direct.get('command')
        require(isinstance(direct_cmd, list) and api.comparable_build(direct_cmd) == api.comparable_build(command)
                and direct.get('supervisor_exit') == 0 and direct.get('image_exists') is False,
                'direct gate does not match the positive build')
        direct_launcher = data('direct_launcher')
        require(positive.count(b'import unittest\n') == 1 and direct_launcher ==
                positive.replace(b'import unittest\n', b'import unittest\n\nstatus: int = True\n', 1)
                and sha_lines(text('direct_files')) == {direct_cmd[2]:sha(direct_launcher),poison_path:sha(poison)}
                and data('direct_stdout') == b'', 'direct poison launcher delta or identity differs')
        trace_direct_gate(text('direct_trace'),direct_cmd,probe,text('direct_stderr'),require)
        extra_sources = (command[2], str(Path(command[2]).parent / file('stock_source').name))
        require(binding.get('withdraw_sources') == list(extra_sources) and sha_lines(text('extra_restored')) == {extra_sources[0]:sha(positive),extra_sources[1]:sha(data('stock_source'))}, 'withdrawn launcher/source archive or restoration digest differs')
    inventories = {}
    paths = {}
    for tier in api.TIERS:
        meta = run['runs'][tier]
        raw = file(tier+'_stderr')
        require(sha(raw.read_bytes())==meta.get('stderr_sha256')
            and meta.get('exit_code')==0
            and meta.get('timed_out') is False, tier+' raw outcome capture differs')
        inventories[tier]=event_index(raw,require)
        paths[tier]=raw.relative_to(api.ROOT).as_posix()
        if tier == 'stock': continue
        pos = meta.get('build',{})
        argv = pos.get('argv')
        api.matched_build(argv,tier,compiler,runtime)
        normalized = list(argv)
        normalized[normalized.index('--opt-level')+1] = '0'
        require(api.comparable_build(normalized)==api.comparable_build(command), tier+' build does not match the gated launcher graph')
        image = api.option(argv,'-o')
        require(pos.get('exit_code')==0
            and pos.get('timed_out') is False
            and argv[2]==command[2]
            and (tier!='O0'
            or api.comparable_build(argv)==api.comparable_build(command))
            and meta.get('argv')==[image]
            and sums.get(image)==meta.get('image_sha256')
            and bool(api.HEX64.fullmatch(sums.get(image,''))), tier+' saved native launcher/build identity differs')
        require(data(tier+'_withdraw_stderr')==raw.read_bytes() and data(tier+'_withdraw_stdout')==b'', tier+' withdrawn raw outcomes differ from baseline')
        if extra_sources:
            hidden = meta.get('withdrawal',{}).get('environment',{})
            require(hidden.get('PATH') == str(Path(api.option(command,'--clang')).parent)
                    and hidden.get('IMRE_WITHDRAW_LAUNCHER') == extra_sources[0]
                    and hidden.get('IMRE_WITHDRAW_ARCHIVE') == extra_sources[1],
                    tier+' external Python path or withdrawn source configuration differs')
        if content:
            rec=obj(tier+'_recapture'); trace_loc=artifacts[tier+'_trace']; argv=rec.get('argv',[])
            require(rec.get('schema') == 'imre.compatibility.run-only.v1' and rec.get('family') == family and rec.get('tier') == tier
                    and rec.get('controller_sha256') == sha(data('collector')) and rec.get('archive_sha256') == sha(data('archive'))
                    and len(argv) == 16 and argv[:9] == ['/usr/bin/strace','-f','-y','-qq','-s','512','-e','trace='+syscalls,'-o']
                    and argv[9].endswith('/'+trace_loc) and argv[10:] == [probe,'withdraw',tree,stash,leaf,image]
                    and rec.get('cwd') == meta.get('cwd') and rec.get('environment') == meta.get('withdrawal',{}).get('environment')
                    and rec.get('timeout_seconds') == 30 and rec.get('exit_code') == 0
                    and rec.get('image_sha256_before') == rec.get('image_sha256_after') == sums[image]
                    and rec.get('trace_sha256') == sha(data(tier+'_trace'))
                    and rec.get('stdout_sha256') == sha(data(tier+'_withdraw_stdout')) and data(tier+'_withdraw_stdout') == data(tier+'_stdout')
                    and rec.get('stderr_sha256') == sha(data(tier+'_withdraw_stderr'))
                    and rec.get('original_output_equal') is True and rec.get('restored_source_sha256') == source['sha256']
                    and rec.get('restored_extra_sha256') == sha_lines(text('extra_restored')),
                    tier+' run-only capture binding or output identity differs')
        trace_withdraw(api,text(tier+'_trace'),image,probe,tree,stash,leaf,meta.get('cwd'),require,extra_sources,
                       mode == 'direct-and-withdraw-v1', (policy,family,command[2],opt) if content else None)
        elf = meta.get('withdrawal',{}).get('readelf',{})
        elfraw = data(tier+'_readelf')
        require(elf.get('argv')==['readelf','-d',image]
            and elf.get('exit_code')==0
            and sha(elfraw)==elf.get('stdout_sha256')
            and b'Dynamic section' in elfraw
            and b'libpython' not in elfraw.lower(), tier+' raw ELF links libpython or is missing')
    assigned = {key for key, name in mapping.items() if name == cohort}
    stock_keys = list(inventories['stock']); native_keys = list(inventories['O0'])
    require(assigned and assigned <= set(stock_keys) <= set(mapping), 'launcher cohort stock inventory does not cover its assigned originals')
    require(native_keys == list(inventories['O3']), 'native cohort inventories differ in ID, occurrence or order')
    if kind == 'finite':
        require(stock_keys == native_keys and assigned == set(native_keys), 'finite cohort assigned and ordered stock/native inventories differ')
    else:
        require(assigned.isdisjoint(native_keys) and set(native_keys) <= set(stock_keys), 'stock-only cohort contains a native original')
    return run, inventories, paths


def validate_record(api, r, eligible, check, entry):
    versioned=entry.get('adapter')=='raw-control-v2';locator=entry.get('binding') if versioned else entry.get('sources',{}).get('binding')
    check(isinstance(locator,str), 'missing normalized raw-control binding')
    digest = api.binding_digest(entry, locator)
    activation_digest = sha(json.dumps(entry, sort_keys=True).encode())
    entry_json=json.dumps(entry,sort_keys=True) if versioned else None
    binding, mapping = load_binding(api, r['family'], locator, digest, activation_digest,entry_json)
    key = (r['id'], r['occurrence'])
    check(key in mapping, 'original has no explicit launcher cohort')
    cohort = mapping[key]
    run, inventories, paths = adapt(api, r['family'], locator, digest, json.dumps(r['source'], sort_keys=True), activation_digest, cohort,entry_json)
    outcomes=[]
    check(r.get('platform')==run.get('platform'), 'row platform differs from raw replay')
    for tier in api.TIERS:
        row=r['runs'][tier]
        event=inventories[tier].get(key)
        check(row.get('argv')==run['runs'][tier].get('argv') and (not versioned or row.get('environment')==run['runs'][tier].get('environment')),tier+' original invocation or environment differs')
        if event is None:
            check(tier!='stock'
                and row.get('outcome')=='not_run'
                and bool(row.get('reason'))
                and not r.get('omission_reason'),tier+' absent original must remain raw not_run without omission')
            outcomes.append('not_run')
            continue
        check(row.get('outcome')==event['outcome']
            and row.get('evidence')==paths[tier]+'#L'+str(event['line']),tier+' raw original ID, occurrence, outcome or line differs')
        outcomes.append(event['outcome'])
    derived=not r.get('omission_reason') and outcomes==['pass','pass','pass']
    if versioned:
        check(derived==bool(eligible) and type(r.get('paired')) is bool and (not r['paired'] or derived) and (r['paired'] or r.get('credit_reason')==binding['identity_blocker']), 'paired differs from authenticated finite outcomes or stage credit reason')
    else:
        check(derived==bool(eligible) and type(r.get('paired')) is bool and r['paired']==derived,'paired does not match validated raw outcomes and controls')
