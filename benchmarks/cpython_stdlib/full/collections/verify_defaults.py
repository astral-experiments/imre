"""Verify the seven promoted collections methods on pinned CPython and in four native compiler modes."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

parser = argparse.ArgumentParser(description=__doc__)
for name in ("imre", "runtime", "result"):
    parser.add_argument("--" + name, required=True, type=Path)
parser.add_argument("--compiler-revision", required=True)
parser.add_argument("--clang", type=Path, default=shutil.which("clang-18") or shutil.which("clang"))
parser.add_argument("--suite", type=Path, default=Path(__file__).resolve().parent)
parser.add_argument("--compare", type=Path, help="optional second four-mode receipt to check identical source and fixture hashes")
args = parser.parse_args()
if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython" or args.clang is None:
    parser.error("use test-enabled CPython 3.14.5 and supply a clang executable")
here = args.suite.resolve()
sys.path[:0] = [str(here), str(here.parents[1])]
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeDefaultMethod, Toolchain
from routines import COUNTER_OPERATIONS
import test

def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

manifest = json.loads((here / "UPSTREAM.json").read_bytes())
reference = json.loads((here / "reference-linux.json").read_bytes())["reference"]
verify_files(here / "upstream", manifest["files"])
verify_files(Path(test.__file__).resolve().parents[2], manifest["files"])
imre, runtime, clang = (p.resolve() for p in (args.imre, args.runtime, args.clang))
revision, tree, *parents = subprocess.check_output(
    ["git", "-C", here, "show", "-s", "--format=%H %T %P", args.compiler_revision], text=True).split()
source = {"compiler_revision": revision, "tree": tree, "parents": parents, "cpython_commit": manifest["commit"],
          "imre_sha256": digest(imre), "runtime_sha256": digest(runtime)}
selected = {"test.test_collections.TestCounter." + m: COUNTER_OPERATIONS[m]
            for m in ("test_ge", "test_gt", "test_le", "test_lt", "test_total")}
selected.update({
    "test.test_deque.TestBasic.test_clear": "Clear an existing native deque and measure its length",
    "test.test_deque.TestBasic.test_maxlen_zero": "Zero-length deque construction, extend and extendleft consume the original iterator",
})
comparison = json.loads(args.compare.read_bytes()) if args.compare else None
discovered = {}
for module in manifest['modules']:
    try: suite = load_suite(module)
    except unittest.SkipTest: continue
    for case in cases(suite):
        if case.id() in selected:
            assert case.id() not in discovered
            discovered[case.id()] = case
assert discovered.keys()==selected.keys()
summary = {'source':source,'reference_denominator':len(reference),
           'reference_controls':{},'modes':{}}
for ident,case in discovered.items():
    result = unittest.TestResult()
    type(case)(case._testMethodName).run(result)
    assert result.wasSuccessful() and result.testsRun==1 and not result.skipped, ident
    summary['reference_controls'][ident]='pass'
for opt in (0,3):
    for reach in (False,True):
        tool = Toolchain(imre, runtime, clang, opt, reach)
        rows = {}
        for ident,case in discovered.items():
            src='Lib/'+type(case).__module__.replace('.','/')+'.py'
            with tempfile.TemporaryDirectory(prefix='current-seven-') as work:
                row=NativeDefaultMethod(tool,Path(work),here/'upstream'/src,manifest['files'][src],('imre',selected[ident])).run(type(case)(case._testMethodName))
            rows[ident]=row
            if row['outcome']=='pass':
                assert set(row['roles'].values()) == {'imre'}
                if comparison:
                    other = comparison['modes'][f'O{opt}-'+('reachable' if reach else 'whole')][ident]
                    assert other['outcome'] == 'pass' and other['roles'] == row['roles']
                    for key in ('method_sha256','source_blob','staged_id','entry_sha256'):
                        assert row['evidence'][key] == other['evidence'][key], (ident, key)
                assert row['evidence']['reachable_only'] is reach
            print(opt,reach,ident,row['outcome'],flush=True)
        summary['modes'][f'O{opt}-'+('reachable' if reach else 'whole')]=rows
args.result.write_text(json.dumps(summary,indent=2,sort_keys=True)+'\n')
assert all(x['outcome']=='pass' for rows in summary['modes'].values() for x in rows.values())

assert digest(imre) == source['imre_sha256'] and digest(runtime) == source['runtime_sha256']
