"""Digest-bound finite-unittest raw control adapter for snapshot receipts.

The binding describes locations, not control verdicts. Credit is derived from
raw process/file traces and independently anchored original result events.
"""
from __future__ import annotations

import hashlib
import json
import posixpath
import re
import tarfile
import zipfile
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


def execs(lines, require):
    calls = [(i, line) for i, line in enumerate(lines) if re.search(r'\bexecve(?:at)?\(', line)]
    require(len(calls) == 2, 'raw control must execute exactly supervisor and target; a native child exec is unverified')
    result = []
    for i, line in calls:
        match = EXEC.fullmatch(line)
        require(match is not None, 'raw exec attempt failed, was truncated, or used execveat')
        result.append((i, match[1], json.loads(match[2]), json.loads(match[3])))
    require(result[0][1] != result[1][1], 'supervisor and target PID must differ')
    return result


def find(lines, pattern, start=0):
    for i in range(start, len(lines)):
        if re.search(pattern, lines[i]): return i
    raise ValueError('missing raw trace operation: ' + pattern)


def stat(lines, pid, path, absent, start=0):
    tail = r'= -1 ENOENT\b' if absent else r'= 0$'
    return find(lines, rf'^{pid}\s+newfstatat\(AT_FDCWD, {re.escape(json.dumps(path))}, .*{tail}', start)


def exits(lines, parent, child, code, start):
    end = find(lines, rf'^{child}\s+exit_group\({code}\)', start)
    waited = find(lines, rf'^{parent}\s+(?:<\.\.\. wait4 resumed>|wait4\({child},).*WEXITSTATUS\(s\) == {code}.*= {child}$', end + 1)
    return waited


def trace_gate(text, command, probe, poison_path, diagnostic, require):
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
            if m and line.startswith(target[1] + ' ') and json.loads(m[1]) == path and re.search(r'= [0-9]+$', line): found = True
        require(found, 'gate compiler did not open the actual launcher and imported poison')
    m = re.search(rf'^{target[1]}\s+exit_group\(([1-9][0-9]*)\)', text, re.M)
    require(m is not None, 'gate compiler did not exit nonzero')
    waited = exits(lines, first[1], target[1], int(m[1]), target[0])
    after = stat(lines, first[1], out, True, waited + 1)
    find(lines, rf'^{first[1]}\s+exit_group\(0\)', after + 1)
    require(poison_path in diagnostic and re.search(r'expected\s+int,\s*got\s+bool', diagnostic), 'missing imported int/bool diagnostic')


def trace_withdraw(api, text, image, probe, tree, stash, leaf, cwd, require):
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
    require(isinstance(cwd, str) and Path(cwd).is_absolute(), 'native cwd missing')
    stem = Path(leaf).stem.lower()
    for line in lines[target[0]:waited+1]:
        # A successful io_uring setup would permit unobserved asynchronous opens.
        require(not re.search(r'\bio_uring_setup\(.*= [0-9]+$', line), 'native io_uring may hide file access')
        m = OPEN.match(line) or re.match(r'^\d+\s+newfstatat\(.*?("(?:[^"\\]|\\.)*")', line)
        if m:
            path = json.loads(m[1])
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
                or Path(stash).name in Path(path).parts)
            # Only an explicit ENOENT proves this withdrawn path was absent.
            require(not withdrawn or re.search(r'\) = -1 ENOENT(?: \([^)]*\))?$', line),
                    'native opened CPython, libpython, withdrawn source or matching bytecode')
    for path in api.successful_open_paths('\n'.join(lines[target[0]:waited+1]), cwd):
        name = path.name.lower()
        cached = path.parent.name == '__pycache__' and name.startswith(stem+'.') and path.suffix in {'.pyc','.pyo'}
        forbidden = (path.is_relative_to(Path(tree)) or path.is_relative_to(Path(stash))
                     or name in {stem+'.py',stem+'.pyc',stem+'.pyo'} or cached
                     or 'libpython' in str(path).lower() or name.startswith('python'))
        require(not forbidden, 'native opened CPython, libpython, withdrawn source or matching bytecode')


def event_index(path, require):
    result = parse(path.read_bytes(), False)
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


@lru_cache(maxsize=None)
def load_binding(api, family, locator, digest, activation_digest):
    require = api.require
    entry = api.activation_entry(family)
    path = api.checked_file(api.repository_file(locator, api.DATA_ROOT), digest)
    binding = json.loads(path.read_text())
    require(binding.get('schema') == 'imre.compatibility.raw-control.v1' and binding.get('family') == family, 'invalid normalized raw-control binding')
    cohorts, originals = binding.get('cohorts'), binding.get('originals')
    require(isinstance(cohorts, dict) and cohorts and isinstance(originals, list), 'missing launcher cohorts or original mapping')
    mapping = {}
    for row in originals:
        require(isinstance(row, dict) and isinstance(row.get('id'), str) and type(row.get('occurrence')) is int and row['occurrence'] >= 1, 'invalid original cohort key')
        key = (row['id'], row['occurrence']); cohort = row.get('launcher_cohort')
        require(key not in mapping and isinstance(cohort, str) and cohort in cohorts and isinstance(cohorts[cohort], dict), 'duplicate or unknown original launcher cohort')
        mapping[key] = cohort
    receipt = api.repository_file(entry.get('receipt'), api.DATA_ROOT)
    rows = [json.loads(line) for line in receipt.read_text().splitlines()]
    keys = [(row['id'], row['occurrence']) for row in rows]
    require(len(keys) == len(set(keys)) and set(keys) == set(mapping) and set(mapping.values()) == set(cohorts), 'launcher cohort mapping must cover every receipt original exactly once')
    for row in rows:
        kind = cohorts[mapping[(row['id'], row['occurrence'])]].get('kind', 'finite')
        require(kind in {'finite','stock-only'} and (kind != 'stock-only' or row.get('paired') is False), 'credited original cannot use a stock-only cohort')
    return binding, mapping


@lru_cache(maxsize=None)
def adapt(api, family, locator, digest, source_json, activation_digest, cohort):
    require = api.require
    entry = api.activation_entry(family)
    manifest, mapping = load_binding(api, family, locator, digest, activation_digest)
    binding = manifest['cohorts'][cohort]
    kind = binding.get('kind', 'finite')
    artifacts = binding.get('artifacts')
    require(isinstance(artifacts, dict), 'missing raw-control artifacts')
    def file(role):
        loc = artifacts.get(role)
        require(isinstance(loc, str), 'missing raw-control role: '+role)
        return api.checked_file(api.repository_file(loc, api.DATA_ROOT), api.binding_digest(entry, loc))
    def data(role): return file(role).read_bytes()
    def text(role): return data(role).decode()
    def obj(role): return json.loads(text(role))
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
    require('trace=execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,exit_group' in collector
        and "'-f'" in collector
        and "'-s','512'" in collector, 'raw trace collection lacks syscall or descendant coverage')
    trace_gate(text('gate_trace'),command,probe,poison_path,text('gate_stderr'),require)
    require(data('gate_stdout')==b'', 'unexpected gate stdout')
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
        trace_withdraw(api,text(tier+'_trace'),image,probe,tree,stash,leaf,meta.get('cwd'),require)
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
    locator = entry.get('sources',{}).get('binding')
    check(isinstance(locator,str), 'missing normalized raw-control binding')
    digest = api.binding_digest(entry, locator)
    activation_digest = sha(json.dumps(entry, sort_keys=True).encode())
    _, mapping = load_binding(api, r['family'], locator, digest, activation_digest)
    key = (r['id'], r['occurrence'])
    check(key in mapping, 'original has no explicit launcher cohort')
    cohort = mapping[key]
    run, inventories, paths = adapt(api, r['family'], locator, digest, json.dumps(r['source'], sort_keys=True), activation_digest, cohort)
    outcomes=[]
    check(r.get('platform')==run.get('platform'), 'row platform differs from raw replay')
    for tier in api.TIERS:
        row=r['runs'][tier]
        event=inventories[tier].get(key)
        check(row.get('argv')==run['runs'][tier].get('argv'),tier+' original invocation differs')
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
    check(derived==bool(eligible) and type(r.get('paired')) is bool and r['paired']==derived,'paired does not match validated raw outcomes and controls')
