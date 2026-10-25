"""Verification branch only: test a clean product tree and preserve its exact inputs."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path.cwd()
suite = root / 'benchmarks/cpython_stdlib/full/collections'
out = Path(sys.argv[1])
out.mkdir()
imre = root / 'target/debug/imre'
runtime = root / 'target/debug/libimre_runtime.a'
clang = '/usr/bin/clang-18'


def git(spec):
    return subprocess.check_output(['git', 'rev-parse', spec], text=True).strip()


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fixture(rel, level):
    src = root / 'tests/fixtures' / rel
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        native, reference = temp / 'native', temp / 'reference'
        native.mkdir()
        reference.mkdir()
        expected = subprocess.run([sys.executable, '-E', '-B', src], cwd=reference, capture_output=True, check=True)
        binary = temp / 'case'
        modules = ['--module-root', src.parent] if src.name == 'main.py' else []
        subprocess.run([imre, 'build', src, *modules, '--runtime', runtime, '--clang', clang,
                        '--opt-level', str(level), '--no-cache', '-o', binary], cwd=temp, check=True)
        actual = subprocess.run([binary], cwd=native, capture_output=True, check=True)
        if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
            raise AssertionError((rel, level, actual.stdout, expected.stdout, actual.stderr, expected.stderr))
        def files(directory):
            return {str(p.relative_to(directory)): digest(p) for p in directory.rglob('*') if p.is_file()}
        assert files(native) == files(reference), (rel, level)
        return {'lines': len(expected.stdout.splitlines()), 'stdout_sha256': hashlib.sha256(actual.stdout).hexdigest()}


def write(name, value):
    (out / name).write_text(json.dumps(value, indent=2) + '\n')


def load(path):
    return json.loads(path.read_text())


revision = git('HEAD')
write('source.json', {'compiler_revision': revision, 'tree': git('HEAD^{tree}'), 'base': git('HEAD^1'),
                      'head': git('HEAD^2'), 'imre_sha256': digest(imre), 'runtime_sha256': digest(runtime),
                      'cpython': sys.version, 'cpython_commit': '5607950ef232dad16d75c0cf53101d9649d89115'})

with tempfile.TemporaryDirectory() as integration_dir:
    program = Path(integration_dir) / 'whitespace.py'
    program.write_text('"""Verify the pinned expression, its class binding and the complete stock module independently."""\nimport argparse\nimport ast\nimport hashlib\nimport json\nfrom pathlib import Path\nimport subprocess\nimport sys\nimport tempfile\n\np=argparse.ArgumentParser()\np.add_argument(\'--cpython-source\', type=Path, required=True)\np.add_argument(\'--result\', type=Path, required=True)\np.add_argument(\'--imre\', type=Path)\np.add_argument(\'--runtime\', type=Path)\np.add_argument(\'--clang\', type=Path)\np.add_argument(\'--combined-tree\')\np.add_argument(\'--tested-ord-commit\', default=\'27525b613276d3e3862effd4be25e4517d3feaa3\')\nargs=p.parse_args()\nfull=(args.cpython_source/\'Lib/textwrap.py\').read_bytes()\nblob=hashlib.sha1(b\'blob \'+str(len(full)).encode()+b\'\\0\'+full).hexdigest()\nassert blob==\'41366fbf443a4fc46768a6745d0b9e64c1d3b056\' and sys.version_info[:3]==(3,14,5)\nlines=full.splitlines(keepends=True)\ntree=ast.parse(full)\ndef assignment(nodes,name):\n    return next(n for n in nodes if isinstance(n,ast.Assign)\n                and any(isinstance(t,ast.Name) and t.id==name for t in n.targets))\ndef text(n):\n    return b\'\'.join(lines[n.lineno-1:n.end_lineno])\nws=assignment(tree.body,\'_whitespace\')\ncls=next(n for n in tree.body if isinstance(n,ast.ClassDef) and n.name==\'TextWrapper\')\ninit=assignment(cls.body,\'unicode_whitespace_trans\')\nassert ws.lineno<cls.lineno<init.lineno\nexpression=ast.get_source_segment(full.decode(),init.value).encode()\nassert expression==b"dict.fromkeys(map(ord, _whitespace), ord(\' \'))"\nshow=b\'print(list(TextWrapper.unicode_whitespace_trans.items()))\\n\'\nexpected=b\'[(9, 32), (10, 32), (11, 32), (12, 32), (13, 32), (32, 32)]\\n\'\nprograms={\n    \'expression\':text(ws)+b\'\\nprint(list((\'+expression+b\').items()))\\n\',\n    \'class-initializer\':text(ws)+b\'\\n\'+lines[cls.lineno-1]+text(init)+b\'\\n\'+show,\n    \'whole-module\':b\'from stock_textwrap import TextWrapper\\n\'+show,\n}\nreport={\'cpython_commit\':\'5607950ef232dad16d75c0cf53101d9649d89115\',\n        \'textwrap_git_blob\':blob,\'whitespace_line\':ws.lineno,\'class_line\':cls.lineno,\n        \'initializer_line\':init.lineno,\'initializer_sha256\':hashlib.sha256(text(init)).hexdigest(),\n        \'private_reference_tip\':\'27525b613276d3e3862effd4be25e4517d3feaa3\',\n        \'tested_ord_commit\':args.tested_ord_commit,\n        \'combined_tree\':args.combined_tree,\'probes\':{}}\nfor name,program in programs.items():\n    with tempfile.TemporaryDirectory() as d:\n        work=Path(d); src=work/\'entry.py\'; src.write_bytes(program)\n        if name==\'whole-module\': (work/\'stock_textwrap.py\').write_bytes(full)\n        ref=subprocess.run([sys.executable,\'-E\',\'-B\',src],cwd=work,capture_output=True)\n        assert (ref.returncode,ref.stdout,ref.stderr)==(0,expected,b\'\'),(name,ref)\n        row={\'program_sha256\':hashlib.sha256(program).hexdigest(),\'reference\':ref.stdout.decode().strip(),\n             \'modes\':{}}\n        if args.imre:\n            for level in (0,3):\n                for reach in (False,True):\n                    binary=work/\'case\'\n                    call=[args.imre,\'build\',src,\'--module-root\',work,\'--no-cache\',\'--runtime\',args.runtime,\n                          \'--clang\',args.clang,\'--opt-level\',str(level),\'-o\',binary]\n                    if reach: call.append(\'--reachable-only\')\n                    c=subprocess.run(call,cwd=work,capture_output=True)\n                    item={\'compile_exit\':c.returncode}\n                    if c.returncode:\n                        item.update(outcome=\'compile-blocked\',diagnostic=c.stderr.decode().replace(d,\'<work>\'))\n                    else:\n                        result=subprocess.run([binary],cwd=work,capture_output=True)\n                        item.update(outcome=\'matched\' if (result.returncode,result.stdout,result.stderr)==(0,expected,b\'\') else \'failed\',\n                                    exit=result.returncode,stdout=result.stdout.decode(),stderr=result.stderr.decode())\n                    row[\'modes\'][f\'O{level}-reachable-{int(reach)}\']=item\n        report[\'probes\'][name]=row\nargs.result.write_text(json.dumps(report,indent=2)+\'\\n\')\n')
    subprocess.run([sys.executable, program, '--cpython-source', root / '.cpython-collections', '--result', out / 'textwrapper.json', '--imre', imre, '--runtime', runtime, '--clang', clang, '--tested-ord-commit', git('HEAD^1'), '--combined-tree', git('HEAD^{tree}')], check=True)
# The mapping dependency has its own untouched dict test and denominator.
subprocess.run([sys.executable, suite / 'fromkeys/run.py', '--imre', imre, '--runtime', runtime, '--clang', clang,
                '--compiler-revision', revision, '--result', out / 'fromkeys-original.json'], check=True)
focused_failures = []
fixture_path = root / 'tests/fixtures/containers/dict_fromkeys.py'
oracle = subprocess.run([sys.executable, '-E', '-B', fixture_path], capture_output=True, check=True)
assert len(oracle.stdout.splitlines()) == 26
assert b'{9: 32, 10: 32, 11: 32, 12: 32, 13: 32, 32: 32}' in oracle.stdout
for level in (0, 3):
    for reachable in (False, True):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'case'
            flags = ['--reachable-only'] if reachable else []
            compile = subprocess.run([imre, 'build', fixture_path, '--no-cache', '--runtime', runtime, '--clang', clang,
                                      '--opt-level', str(level), *flags, '-o', binary], capture_output=True)
            row = {'opt_level':level, 'reachable_only':reachable, 'compile_exit':compile.returncode,
                   'compiler_diagnostic':compile.stderr.decode().replace(directory, '<work>')}
            if compile.returncode:
                focused_failures.append(row)
            else:
                native = subprocess.run([binary], capture_output=True)
                row.update(exit=native.returncode, lines=len(native.stdout.splitlines()),
                           stdout_sha256=hashlib.sha256(native.stdout).hexdigest(),
                           stderr=native.stderr.decode(), matches_cpython=(native.stdout,native.stderr)==(oracle.stdout,oracle.stderr))
                if native.returncode or not row['matches_cpython']: focused_failures.append(row)
            write(f'fromkeys-O{level}-reachable-{int(reachable)}.json', row)
import ast
import textwrap

# Diagnostic projections are outside both original denominators. Use the source
# of the original classes so absent constructor and iterator protocols remain visible.
source = fixture_path.read_text()
prefix, body = source.split('def main() -> None:\n', 1)
a, b = body.split('    shared: list[int] = []', 1)
b, c = b.split('    source = [1, 2]', 1)
c, d = c.split('    try:\n        print(dict.fromkeys(3))', 1)
d, e = d.split('    counts = Counter("aba")', 1)
e, f = e.split('    groups: defaultdict[str, list[int]]', 1)
standalone = {name: prefix + 'def main() -> None:\n' + snippet.rstrip() + '\n\nmain()\n' for name, snippet in (
    ('builtin-shapes', a), ('shared-values', '    shared: list[int] = []'+b),
    ('generator-errors', '    source = [1, 2]'+c), ('noniterable', '    try:\n        print(dict.fromkeys(3))'+d),
    ('counter-class-policy', '    counts = Counter("aba")'+e),
    ('defaultdict-class-policy', '    groups: defaultdict[str, list[int]]'+f.split('\n\nmain()')[0]))}
standalone['set-range'] = (suite/'fromkeys/probes/set_range.py').read_text()
upstream = (suite / 'fromkeys/upstream/Lib/test/test_dict.py').read_text()
method = next(node for node in ast.walk(ast.parse(upstream)) if isinstance(node,ast.FunctionDef) and node.name == 'test_fromkeys')
defs = {node.name: textwrap.dedent(''.join(upstream.splitlines(True)[node.lineno-1:node.end_lineno])) for node in method.body if isinstance(node,ast.ClassDef)}
observations = {'dictlike': "v=dictlike.fromkeys('a')\nprint(isinstance(v,dictlike),v)\nv=dictlike().fromkeys('a')\nprint(isinstance(v,dictlike),v)",
                'mydict': "v=mydict.fromkeys('ab')\nprint(isinstance(v,collections.UserDict),v)",
                'baddict1': "try:\n    baddict1.fromkeys([1])\nexcept Exc:\n    print('raised Exc')",
                'BadSeq': "try:\n    dict.fromkeys(BadSeq())\nexcept Exc:\n    print('raised Exc')",
                'baddict2': "try:\n    baddict2.fromkeys([1])\nexcept Exc:\n    print('raised Exc')",
                'baddict3': "d = {i : i for i in range(1000)}\nv = baddict3.fromkeys({'a','b','c'})\nprint(v is d, len(v), v[999], v['a'])",
                'baddict4': "d = {i : i for i in range(1000)}\nv = baddict4.fromkeys({'a','b','c'})\nprint(isinstance(v,baddict4), v is d, len(v), v[999], v['a'])"}
for name, use in observations.items():
    definitions = (defs['Exc'] if name in ('baddict1','BadSeq','baddict2') else '') + defs[name]
    standalone['upstream-class-'+name] = ('import collections\n' if name=='mydict' else '') + 'def control():\n' + textwrap.indent(definitions+use+'\n','    ') + '\ncontrol()\n'
for name, program in standalone.items():
    with tempfile.TemporaryDirectory() as location:
        folder=Path(location); src=folder/'entry.py'; src.write_text(program); binary=folder/'case'
        oracle=subprocess.run([sys.executable,'-E','-B',src],cwd=folder,capture_output=True,check=True)
        row={'program_sha256':hashlib.sha256(program.encode()).hexdigest(),'expected':oracle.stdout.decode(),'modes':{}}
        for level in (0,3):
            for reach in (False,True):
                flags=['--reachable-only'] if reach else []
                built=subprocess.run([imre,'build',src,'--no-cache','--runtime',runtime,'--clang',clang,'--opt-level',str(level),*flags,'-o',binary],cwd=folder,capture_output=True)
                item={'compile_exit':built.returncode}
                if built.returncode:
                    item.update(outcome='compile-blocked',diagnostic=built.stderr.decode().replace(location,'<work>'))
                else:
                    result=subprocess.run([binary],cwd=folder,capture_output=True)
                    item.update(outcome='matched' if (result.returncode,result.stdout,result.stderr)==(0,oracle.stdout,oracle.stderr) else 'failed', exit=result.returncode,stdout=result.stdout.decode(),stderr=result.stderr.decode())
                row['modes'][f'O{level}-reachable-{int(reach)}']=item
        write('mapping-control-'+name+'.json',row)
