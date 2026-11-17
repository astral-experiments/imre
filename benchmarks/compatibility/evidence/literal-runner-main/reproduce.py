"""Reproduce the current-main finite-launcher experiment on Imre Cloud."""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
COMPAT = HERE.parent.parent
ROOT = COMPAT.parent.parent
sys.path.insert(0, str(COMPAT))
from collect_unittest_results import capture, parse
from generate_unittest_runner import generate

SCRATCH = Path('/workspace/scratch')
FIXTURE = SCRATCH / 'imre-cpython-test-fixtures/Lib'
STOCK = SCRATCH / 'python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14'
IMRE = ROOT / 'target/release/imre'
RUNTIME = ROOT / 'target/release/libimre_runtime.a'
CLANG = Path('/workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang')
MODULES = ['test.test_fnmatch', 'test.test_tomllib.test_data', 'test.test_tomllib.test_error', 'test.test_tomllib.test_misc']
SOURCES = [FIXTURE / (m.replace('.', '/') + '.py') for m in MODULES]
BASE_ENV = os.environ.copy()
BASE_ENV.update(PYTHONPATH=str(FIXTURE), PYTHONDONTWRITEBYTECODE='1', PYTHON_COLORS='0', NO_COLOR='1', TMPDIR=str(SCRATCH / 'imre-tmp'))
BASE_ENV.pop('IMRE_ENCODED_FLAGS', None)
def git_blob(path):
    data = path.read_bytes()
    return hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()


def source_blobs():
    manifests = [json.loads((ROOT / 'benchmarks/cpython_stdlib/full' / family / 'UPSTREAM.json').read_text()) for family in ('fnmatch','tomllib')]
    blobs = {}
    for source in SOURCES:
        relative = 'Lib/' + source.relative_to(FIXTURE).as_posix()
        expected = next(m['files'][relative] for m in manifests if relative in m['files'])
        actual = git_blob(source)
        if actual != expected: raise ValueError(f'upstream Git blob mismatch: {relative}')
        blobs[relative] = actual
    return blobs


META = {'base_revision': '4184e6ac609c77f1bf2cfa963321db596a5611cf', 'cpython_commit': '5607950ef232dad16d75c0cf53101d9649d89115', 'modules': MODULES, 'runs': {}, 'source_sha256': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCES}, 'source_git_blobs': source_blobs(), 'compiler_runtime_build': {'source_revision': '4184e6ac609c77f1bf2cfa963321db596a5611cf', 'command': 'CARGO_HOME=/workspace/.toolchains/cargo RUSTUP_HOME=/workspace/.toolchains/rustup PATH=/workspace/.toolchains/cargo/bin:$PATH cargo build --offline --release --locked -p imre-compiler -p imre-runtime --target-dir /workspace/imre/target'}, 'platform': {'os': 'linux', 'arch': 'x86_64'}, 'stock_full_version': subprocess.check_output([STOCK, '-VV'], text=True).strip()}


def digest(p): return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def save(name, argv, env, timeout, launcher=False):
    print(name, flush=True)
    code, out, err, timed = capture([str(x) for x in argv], timeout, env=env)
    (HERE / (name + '.stdout')).write_bytes(out)
    (HERE / (name + '.stderr')).write_bytes(err)
    run = {'argv': [str(x) for x in argv], 'environment': {k: env[k] for k in ('PYTHONPATH','PYTHONDONTWRITEBYTECODE','PYTHON_COLORS','NO_COLOR','TMPDIR','IMREFLAGS','IMRE_PYTHON') if k in env}, 'timeout_seconds': timeout, 'exit_code': code, 'timed_out': timed, 'stdout_sha256': hashlib.sha256(out).hexdigest(), 'stderr_sha256': hashlib.sha256(err).hexdigest()}
    if launcher:
        report = parse(err, timed)
        run.update(collection=report['collection'], inventory_complete=report['inventory_complete'], outcomes=dict(Counter(x['outcome'] for x in report['originals'])), fixtures=report['fixtures'], summary=report['summary'])
        (HERE / (name + '.json')).write_text(json.dumps(report, indent=2) + '\n')
    META['runs'][name] = run
    (HERE / 'run.json').write_text(json.dumps(META, indent=2) + '\n')
    return run


def invoke(name, argv, env, timeout, launcher=False):
    return save(name, argv, env, timeout, launcher)


def build(name, source, image, opt, env):
    if image.exists(): image.unlink()
    argv = [IMRE, 'build', source, '--module-root', HERE, '--extension-path', FIXTURE, '--no-cache', '--opt-level', opt, '--runtime', RUNTIME, '--clang', CLANG, '-o', image]
    run = invoke(name, argv, env, 240)
    run['image_exists'] = image.exists()
    if image.exists(): run['image_sha256'] = digest(image)
    return run


def sequence(name):
    report = json.loads((HERE / (name + '.json')).read_text())
    return [(r['id'], r['occurrence'], r['outcome']) for r in report['originals']]


def main():
    META['artifacts_sha256'] = {str(p): digest(p) for p in (IMRE,RUNTIME,CLANG,STOCK,SCRATCH/'cpython-3.14.5.tar.gz')}
    runner = HERE / 'runner.py'; runner.write_text(generate(MODULES))
    META['runner_sha256'] = digest(runner)
    invoke('stock-cli', [STOCK,'-B','-m','unittest','-v','test.test_fnmatch','test.test_tomllib'], BASE_ENV, 120)
    invoke('stock', [STOCK,'-B',runner], BASE_ENV, 120, True)
    # TextTestResult puts a test's docstring on a second line.
    cli = []; pending = None
    for line in (HERE/'stock-cli.stderr').read_text().splitlines():
        m = re.match(r'^\S+ \((test\.[^)]+)\)(?: \.\.\. (.*))?$',line)
        if m:
            pending=m[1]
            if m[2]: cli.append((pending,m[2])); pending=None
        elif pending and ' ... ' in line:
            cli.append((pending,line.rsplit(' ... ',1)[1])); pending=None
    status = {'ok':'pass','FAIL':'fail','ERROR':'error','expected failure':'expected_failure','unexpected success':'unexpected_success'}
    cli_seq=[(i,1,status.get(s,'skip' if s.startswith('skipped ') else s)) for i,s in cli]
    META['stock_cli_sequence_matches_generated'] = cli_seq == sequence('stock')
    images = {}
    for opt in ('0','3'):
        tier='O'+opt; env=BASE_ENV.copy(); env.update(IMREFLAGS='--opt-level '+opt,IMRE_PYTHON=str(IMRE))
        image=SCRATCH/('literal-runner-'+tier+'.exe')
        b=build(tier+'.build',runner,image,opt,env)
        if b['exit_code']==0 and b['image_exists']:
            images[tier]=(image,env)
            invoke(tier,[image],env,180,True)
            META['runs'][tier]['sequence_matches_stock'] = sequence(tier)==sequence('stock')
    # Move only original test source files, and forbid their corresponding pyc files.
    moved=[]
    try:
        for source in SOURCES:
            pycs=list((source.parent/'__pycache__').glob(source.stem+'.*.pyc'))
            if pycs: raise RuntimeError(f'cached originals exist: {pycs}')
            dest=SCRATCH/('withdrawn-'+source.parent.name+'-'+source.name)
            source.rename(dest); moved.append((source,dest))
        META['withdrawal']={'sources_absent':all(not p.exists() for p in SOURCES),'matching_pyc_absent':True}
        for tier,(image,env) in images.items():
            invoke(tier+'.withdrawn',[image],env,180,True)
            META['runs'][tier+'.withdrawn']['sequence_matches_baseline']=sequence(tier+'.withdrawn')==sequence(tier)
    finally:
        for source,dest in reversed(moved): dest.rename(source)
        META.setdefault('withdrawal',{})['restored_sha256']={str(p):digest(p) for p in SOURCES}
    env=BASE_ENV.copy(); env.update(IMREFLAGS='--opt-level 0',IMRE_PYTHON=str(IMRE))
    finite=[IMRE,'run',ROOT/'crates/imre-compiler/stdlib/runpy.py','--module-root',ROOT,'--extension-path',FIXTURE,'--quiet','--no-cache','--opt-level','0','--runtime',RUNTIME,'--clang',CLANG]
    for m in ['unittest.__main__','test.test_fnmatch','test.test_tomllib',*MODULES[1:]]: finite += ['--include-module',m]
    finite += ['--','unittest','-v','test.test_fnmatch','test.test_tomllib']
    invoke('runpy-finite',finite,env,180)
    poison=HERE/'literal_poison.py'; poison.write_text('status: int = True\n')
    poisoned=HERE/'poisoned_runner.py'; poisoned.write_text(generate([*MODULES,'literal_poison']))
    small=HERE/'small_import.py'; small.write_text('import literal_poison\n')
    for name,source in [('poison-graph',poisoned),('poison-import',small),('poison-entry',poison)]:
        build(name,source,SCRATCH/(name+'.exe'),'0',env)
    poison_run=META['runs']['poison-graph']
    diagnostic=(HERE/'poison-graph.stderr').read_text(errors='replace')
    rejected=poison_run['exit_code']==1 and not poison_run['image_exists'] and 'expected int, got bool' in diagnostic
    META['poison_rejected_before_image']=rejected
    matched=all(META['runs'].get(t,{}).get('sequence_matches_stock') and META['runs'].get(t+'.withdrawn',{}).get('sequence_matches_baseline') for t in ('O0','O3'))
    META['strict_paired_credit']=sum(x[2]=='pass' for x in sequence('stock')) if rejected and matched else 0
    (HERE/'run.json').write_text(json.dumps(META,indent=2)+'\n')
    print('complete',flush=True)

if __name__=='__main__': main()
