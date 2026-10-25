"""Identify upstream patch.dict callers and their recorded native blockers."""

import ast
import gzip
import hashlib
import json
from pathlib import Path

from projection_audit import stage_test_tree


HERE = Path(__file__).resolve().parent
SOURCE = 'Lib/test/test_unittest/testmock/testpatch.py'
REVIEWED = {'test_patch_dict_as_context_manager', 'test_patch_dict_start_stop',
            'test_patch_dict_stop_without_start'}


def methods():
    raw = (HERE / 'upstream' / SOURCE).read_bytes()
    lines = raw.splitlines(keepends=True)
    owner, = [n for n in ast.parse(raw).body if isinstance(n, ast.ClassDef) and n.name == 'PatchTest']
    selected = {}
    for m in owner.body:
        if not isinstance(m, ast.FunctionDef) or not m.name.startswith('test'):
            continue
        if any(isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'dict'
               and isinstance(n.func.value, ast.Name) and n.func.value.id == 'patch' for n in ast.walk(m)):
            first = min([m.lineno, *(d.lineno for d in m.decorator_list)])
            selected[m.name] = hashlib.sha256(b''.join(lines[first - 1:m.end_lineno])).hexdigest()
    return selected


def stage(root, cpython_source):
    if cpython_source is None:
        import test
        cpython_source = root / 'interpreter'
        lib = cpython_source / 'Lib'
        lib.mkdir(parents=True)
        (lib / 'test').symlink_to(Path(test.__file__).resolve().parent, target_is_directory=True)
    stage_test_tree(cpython_source, root, json.loads((HERE / 'cpython-test-tree.json').read_bytes()))


def known_blockers(reachable, level):
    archive = json.loads(gzip.decompress((HERE / 'patch-dict-default-linux.json.gz').read_bytes()))
    mode = ('reachable' if reachable else 'ordinary') + f'-O{level}'
    records = archive['modes'][mode]['records']
    for name in ('patch-dict-rechecked-linux.json', 'patch-dict-mixed-values-linux.json', 'patch-dict-compiler-blockers-linux.json'):
        records.extend(json.loads((HERE / name).read_bytes())['modes'][mode])
    return {(r['id'], r['occurrence']): r for r in records if r['outcome'] == 'blocked'}


def recorded_blocker(row, prior, method_sha, root):
    if row['outcome'] != 'blocked':
        return False
    reason = row['reason'].replace(str(root), '<root>').strip()
    row['reason'] = reason
    evidence = row['evidence']
    if evidence['stage'] == 'original fixture':
        # Staging stopped before a body could be emitted; pin the upstream definition.
        evidence.setdefault('method_sha256', method_sha)
    if not prior or (row['id'], row['occurrence']) != (prior['id'], prior['occurrence']):
        return False
    return (evidence['stage'], evidence['source_blob'], evidence.get('method_sha256'), reason) == (
        prior['evidence']['stage'], prior['evidence']['source_blob'], method_sha, prior['reason']
    ) and method_sha == prior['evidence']['definition_sha256']
