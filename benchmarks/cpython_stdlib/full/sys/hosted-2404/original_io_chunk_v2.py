"""Run complete pinned CTextIOWrapperTest bodies inside their original host fixture."""
import ast
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
import importlib.util
upstream = Path(os.environ['GITHUB_WORKSPACE']) / '.cpython-stdlib-reference' / 'Lib' / 'test'
spec = importlib.util.spec_from_file_location('test', upstream/'__init__.py', submodule_search_locations=[str(upstream)])
test = importlib.util.module_from_spec(spec)
sys.modules['test'] = test
spec.loader.exec_module(test)
from test import test_io

base = Path(__file__).parent
repo = Path(os.environ['GITHUB_WORKSPACE'])
sys.path.insert(0, str(repo/'benchmarks/cpython_stdlib'))
from suite_native import method_bytes, run_in_fixtures

source = Path(test_io.__file__)
raw = source.read_bytes()
pin = '5607950ef232dad16d75c0cf53101d9649d89115'
def blob(body):
    return hashlib.sha1(b'blob '+str(len(body)).encode()+b'\0'+body).hexdigest()
assert blob(raw) == '57b42fbb65fe107b1573a2a6e0848de942ff8367' and raw == (upstream/'test_io.py').read_bytes()
assert blob((repo/'.cpython-stdlib-reference'/'LICENSE').read_bytes()) == '20cf39097c68baa17cc566b64e76d34ebf034044'
def digest(body):
    return hashlib.sha256(body).hexdigest()
def leaves(suite):
    if isinstance(suite, unittest.TestSuite):
        for item in suite:
            yield from leaves(item)
    else:
        yield suite
def all_tests():
    return unittest.defaultTestLoader.loadTestsFromModule(test_io)
def wrapper():
    return unittest.defaultTestLoader.loadTestsFromTestCase(test_io.CTextIOWrapperTest)
def run_code(code, level, work):
    ir = work/'method.ll'
    elf = work/'method'
    entry = work/'main.py'
    entry.write_bytes(code)
    def cmd(argv, env=None):
        p = subprocess.run(argv, cwd=work, env=env, capture_output=True, text=True, timeout=90)
        return dict(exit=p.returncode, stdout=p.stdout, stderr=p.stderr.replace(str(work),'CASE_ROOT'))
    build = cmd([pair['paths']['imre'],'build',entry,'--runtime-imports','--module-root',work,'--no-cache','--opt-level',str(level),'--runtime',pair['paths']['libimre_runtime.a'],'--clang','/usr/bin/clang-18','--emit-llvm',ir,'-o',elf])
    result = dict(build=build)
    if build['exit']:
        return dict(result, outcome='compile_blocked')
    result['boxed'] = b'@imre_dyn_op' in ir.read_bytes()
    assert result['boxed']
    result['elf_sha256'] = digest(elf.read_bytes())
    result['run'] = observed = cmd([elf], {'PATH':'/no-host','LC_ALL':'C.UTF-8'})
    good = observed['exit'] == 0 and observed['stdout'].endswith('complete\n')
    return dict(result, outcome='passed' if good else 'failed_after_entry' if 'calling original\n' in observed['stdout'] else 'failed_before_entry')
prefix = b"""import sys
import io
import os
sys.path.append("unused")
class Raises:
    def __init__(self, expected): self.expected=expected
    def __enter__(self): return self
    def __exit__(self, kind, error, tb):
        if kind is None: raise AssertionError("missing exception")
        self.exception = error
        return issubclass(kind,self.expected)
class NativeCase:
    def __getattr__(self, name): return getattr(io,name)
    def assertEqual(self, left, right):
        if left != right: raise AssertionError("unequal original operands")
    def assertIs(self, left, right):
        if left is not right: raise AssertionError("different original objects")
    def assertTrue(self, value):
        if not value: raise AssertionError("unexpected false operand")
    def assertIsInstance(self, value, kind):
        if not isinstance(value,kind): raise AssertionError("unexpected type")
    def assertFalse(self, value):
        if value: raise AssertionError("unexpected true operand")
    def assertRaises(self, expected, *args, **kwargs):
        if not args: return Raises(expected)
        with Raises(expected): args[0](*args[1:], **kwargs)
"""
negative = prefix + b"""
case=NativeCase()
for n in (0,1,2,3,4,5):
    try:
        if n == 0: case.assertEqual(io.BytesIO(b"a").read(), b"b")
        if n == 1: case.assertRaises(ValueError,io.BytesIO().read)
        if n == 2: case.assertIs(io.BytesIO(),io.BytesIO())
        if n == 3: case.assertFalse(1)
        if n == 4: case.assertTrue(False)
        if n == 5: case.assertIsInstance(1, str)
    except AssertionError: pass
    else: raise AssertionError("bad native fixture accepted a false assertion")
print("complete")
"""
# Source the exact C-mock classes installed by test_io.load_tests.
mock_names = ('MockRawIOWithoutRead','MockRawIO','CMockRawIO','MockUnseekableIO','CMockUnseekableIO')
source_lines = raw.splitlines(keepends=True)
mock_blobs = {}
mock_source = bytearray()
for node in ast.parse(raw).body:
    if isinstance(node, ast.ClassDef) and node.name in mock_names:
        segment = b''.join(source_lines[node.lineno-1:node.end_lineno])
        assert node.name not in mock_blobs
        mock_blobs[node.name] = digest(segment)
        mock_source.extend(b'\n' + segment)
assert set(mock_blobs) == set(mock_names)
prefix = prefix.replace(b'class NativeCase:\n', bytes(mock_source) + b'\nclass NativeCase:\n    MockRawIO = CMockRawIO\n    MockUnseekableIO = CMockUnseekableIO\n')
names = ('test_rawio','test_rawio_write_through','test_unseekable','test_del__CHUNK_SIZE_SystemError','test_internal_buffer_size','test_issue119506')
src = sys.argv[1]; pair=json.loads((base/'pair.json').read_text())
assert all(digest(Path(v).read_bytes())==pair['sha256'][k] for k,v in pair['paths'].items())
ids = [c.id() for c in leaves(all_tests())]; wrapped = [c.id() for c in leaves(wrapper())]
selected = {test_io.CTextIOWrapperTest(n).id(): n for n in names}
assert set(selected) <= set(wrapped) <= set(ids)
ref=unittest.TestResult(); unittest.TestSuite(test_io.CTextIOWrapperTest(n) for n in names).run(ref)
assert ref.testsRun==len(names) and not (ref.skipped or ref.errors or ref.failures),(ref.skipped,ref.errors,ref.failures)
report={'pair':pair,'pin':pin,'blob':blob(raw),'license_blob':blob((repo/'.cpython-stdlib-reference'/'LICENSE').read_bytes()),'source_sha256':digest(raw),'selected':list(selected),'full_test_io_ids':ids,'full_CTextIOWrapperTest_ids':wrapped,'cpython_outcome':'all selected passed','native_assertion_fixture':prefix.decode(),'levels':{}}
report['upstream_mock_class_sha256'] = mock_blobs
out=base/f'upstream-io-chunk-v2-{src}.json'
for level in (0,3):
    rows=[]; run={}
    def go(case,original):
        name=case._testMethodName
        cls=next(c for c in type(case).__mro__ if name in c.__dict__)
        body=method_bytes(source,cls.__name__,name)
        code=prefix+body+f'\ncase=NativeCase()\ncase.testdata={case.testdata!r}\nprint("calling original")\ncase.{name}()\nprint("complete")\n'.encode()
        with tempfile.TemporaryDirectory() as tmp: row=run_code(code,level,Path(tmp))
        row.update(id=case.id(),occurrence=1,optimization=level,body_sha256=digest(body),body_owner=cls.__name__,fixture_input_sha256=digest(case.testdata),roles={'fixture':'cpython','body':'native' if row['outcome'] in ('failed_after_entry','passed') else 'unobserved','assertions':'native' if row['outcome']=='passed' else 'unconfirmed','operation':'native' if row['outcome']=='passed' else 'see original traceback'},fully_native=False)
        rows.append(row); print(level,name,row['outcome'],row.get('run',row['build'])['stderr'].splitlines()[-1:],flush=True)
        return row
    _,errors,skips=run_in_fixtures(wrapper(),dict.fromkeys(selected,go))
    assert not (errors or skips),(errors,skips)
    with tempfile.TemporaryDirectory() as tmp: neg=run_code(negative,level,Path(tmp))
    assert neg['outcome']=='passed',neg
    report['levels'][f'O{level}']={'rows':rows,'negative_control':neg}
    out.write_text(json.dumps(report,indent=2)+'\n')
