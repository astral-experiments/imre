"""Try the shared native default-fixture runner on every original unittest occurrence."""

import argparse
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from projection_audit import PIN, TEST_TREE, method_metadata, stage_test_tree
from run import reference_skips
from suite_evidence import build_report, occurrence_keys, reference_cases, roles, validate_pair
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeDefaultMethod, Toolchain


def file_sha(path):
    with path.open('rb') as src:
        return hashlib.file_digest(src, 'sha256').hexdigest()


def unobserved():
    return roles(body='unobserved', assertions='unobserved', operation='unobserved', fixture='unobserved')


def normalize(reason, work, root):
    # ty sometimes emits a path without its leading slash.
    for path, label in ((work, '<case>'), (root, '<root>')):
        reason = reason.replace(str(path), label).replace(str(path).lstrip('/'), label)
    return reason.replace(work.name, '<probe>').strip()


def try_default(case, known, files, tools, root):
    cls = type(case)
    module_path = 'Lib/' + cls.__module__.replace('.', '/')
    owner = next((p for p in (module_path + '.py', module_path + '/__init__.py') if p in files), None)
    metadata = method_metadata(case, files)
    if owner is None:
        return {'outcome': 'blocked', 'reason': f'{cls.__module__} is outside the verified unittest sources',
                'roles': unobserved(), 'evidence': {'stage': 'original fixture', **metadata}}
    with tempfile.TemporaryDirectory(dir=root, prefix='probe_') as tmp:
        work = Path(tmp)
        (work / '__init__.py').write_bytes(b'')
        (work / 'test').symlink_to(root / 'test', target_is_directory=True)
        # Eligibility is decided by the discovered object and its exact source, not by the receipt.
        entry = NativeDefaultMethod(tools, work, HERE / 'upstream' / owner, files[owner],
                                    ('unknown', 'pending operation review'), assertions='unknown')
        obs = entry.run(case)
        proof = obs['evidence']
        proof.update(metadata)
        if 'reason' in obs:
            obs['reason'] = normalize(obs['reason'], work, root)
        if proof.get('stage') == 'compile':
            reason = obs['reason']
            if match := re.search(r'cannot resolve first-party module [\'"`]([^\'"`]+)', reason):
                mod = match[1].split('.')
                exists = (root.joinpath(*mod) / '__init__.py').is_file() or root.joinpath(*mod).with_suffix('.py').is_file()
                if mod[0] == 'test' and exists:
                    raise RuntimeError(f'compiler could not see staged {match[1]}: {reason}')
            if match := re.search(r'<(?:case|root)>/(?:test/)?(test/)?([^:\n]+\.py):(\d+):(\d+)', reason):
                filename, line, col = match[2], int(match[3]), match[4]
                if filename == 'case.py' and 'source' in metadata:
                    staged = (work / 'case.py').read_bytes().splitlines()
                    marker = f'    def {case._testMethodName}('.encode()
                    first = next(i for i, text in enumerate(staged, 1) if text.startswith(marker))
                    source, original = metadata['source'].rsplit(':', 1)
                    end = next(i for i, text in enumerate(staged, 1) if text == b"if __name__ == '__main__':") - 2
                    if first <= line <= end:
                        proof['upstream_diagnostic'] = f'{source}:{int(original) + line - first}:{col}'
                elif (root / 'test' / filename).is_file():
                    proof['upstream_diagnostic'] = f'Lib/test/{filename}:{line}:{col}'
        if obs['outcome'] != 'pass':
            return obs
        oracle = subprocess.run([sys.executable, '-I', '-B', work / 'case.py', '-v'], cwd=work,
                                capture_output=True, text=True, timeout=60)
        stderr = re.sub(r'(?m)^(Ran 1 test in )\d+\.\d+s$', r'\1<elapsed>s', oracle.stderr)
        if (oracle.returncode, oracle.stdout, stderr) != (proof['status'], proof['stdout'], proof['stderr']):
            raise ValueError(f'{case.id()}: native and staged CPython default-fixture transcripts differ')
        proof['staged_reference_matches'] = True
        prior = known.get('evidence', {})
        if (known['outcome'] == 'pass' and prior.get('method_sha256') == proof['method_sha256']
                and prior.get('operation') and known['roles']['body'] == 'imre'):
            obs['roles']['operation'] = known['roles']['operation']
            obs['roles']['assertions'] = known['roles']['assertions']
            proof['operation'] = prior['operation']
        else:
            obs.update(outcome='blocked', reason='native method returned; original operation and assertions await review')
            proof['stage'] = 'operation review'
        return obs


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ('imre', 'runtime', 'clang', 'cpython-source', 'result'):
        p.add_argument('--' + flag, type=Path, required=True)
    p.add_argument('--compiler-commit', required=True)
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or not re.fullmatch('[0-9a-f]{40}', args.compiler_commit):
        p.error('requires CPython 3.14.5 and the full Imre compiler commit')
    sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    manifest = json.loads((HERE / 'UPSTREAM.json').read_bytes())
    reference = json.loads((HERE / 'reference-linux.json').read_bytes())
    if reference['cpython_commit'] != PIN:
        raise ValueError('wrong upstream version')
    verify_files(HERE / 'upstream', manifest['files'])
    verify_interpreter_tests(manifest)
    baseline = json.loads((HERE / 'native-linux.json').read_bytes())
    validate_pair(baseline, reference)
    refs = reference_cases(reference)
    discovered = list(cases(load_suite(manifest['module'])))
    keys = list(occurrence_keys(case.id() for case in discovered))
    if set(keys) != refs.keys():
        raise ValueError('original discovery differs from the complete reference')
    variants = [(c.id(), f'{type(c).__module__}.{type(c).__qualname__}') for c in discovered]
    data = {'compiler_commit': args.compiler_commit, 'compiler_sha256': file_sha(args.imre),
            'runtime_sha256': file_sha(args.runtime), 'cpython_commit': PIN, 'test_tree': TEST_TREE,
            'baseline_sha256': file_sha(HERE / 'native-linux.json'), 'modes': {}}
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp).resolve()
        data['verified_fixture_blobs'] = stage_test_tree(args.cpython_source, root,
                                                       json.loads((HERE / 'cpython-test-tree.json').read_bytes()))
        for reachable in (False, True):
            mode = 'reachable' if reachable else 'ordinary'
            pair = []
            for base in baseline:
                level = base['opt_level']
                prior = {(r['id'], r['occurrence']): r for r in base['records']}
                tools = Toolchain(args.imre, args.runtime, args.clang, level, reachable, root)
                observations = {}
                for i, (key, case) in enumerate(zip(keys, discovered, strict=True), 1):
                    if refs[key] == 'pass':
                        observations[key] = try_default(case, prior[key], manifest['files'], tools, root)
                    elif (*key, level) not in reference_skips(level):
                        raise ValueError(f'no reviewed exception for {key}: {refs[key]}')
                    if i % 100 == 0:
                        print(f'{mode} O{level}: {i}/{len(keys)}', flush=True)
                report = build_report(reference, level, variants, observations, exceptions=reference_skips(level))
                print(mode, level, report['counts'], Counter(x.get('evidence', {}).get('stage') for x in report['records']), flush=True)
                pair.append(report)
            validate_pair(pair, reference)
            data['modes'][mode] = pair
    with args.result.open('wb') as out, gzip.GzipFile(filename='', fileobj=out, mode='wb', mtime=0) as gz:
        gz.write(json.dumps(data, separators=(',', ':'), sort_keys=True).encode() + b'\n')


if __name__ == '__main__':
    main()
