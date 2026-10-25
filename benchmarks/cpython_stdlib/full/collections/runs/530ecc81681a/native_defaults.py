"""Run verified original methods in native unittest.main; preserve earlier fixture receipts."""
import argparse
import ast
import collections
import doctest
from functools import cache
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

here=Path.cwd()/'benchmarks/cpython_stdlib/full/collections'
sys.path[:0]=[str(here),str(here.parents[1])]
from chainmap_methods import OPERATIONS as CHAINMAP
from deque_methods import BIG_METHODS, BIG_OPERATION
from routines import COUNTER_OPERATIONS
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeDefaultMethod, Toolchain

@cache
def own_methods(path):
    return {cls.name:{n.name for n in cls.body if isinstance(n,ast.FunctionDef)}
            for cls in ast.parse(path.read_bytes()).body if isinstance(cls,ast.ClassDef)}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pair',type=Path,required=True)
    parser.add_argument('--result',type=Path,required=True)
    parser.add_argument('--clang',type=Path,default=shutil.which('clang-18') or shutil.which('clang'),required=False)
    parser.add_argument('--opt-level',type=int,choices=(0,3),required=True)
    parser.add_argument('--all-methods',action='store_true')
    args=parser.parse_args()
    assert sys.version_info[:3]==(3,14,5) and sys.implementation.name=='cpython'
    import test
    manifest=json.loads((here/'UPSTREAM.json').read_bytes())
    reference=json.loads((here/'reference-linux.json').read_bytes())['reference']
    verify_files(here/'upstream',manifest['files'])
    verify_files(Path(test.__file__).resolve().parents[2],manifest['files'])
    metadata=json.loads((args.pair/'source.json').read_bytes())
    for name in ('imre','libimre_runtime.a'):
        with (args.pair/name).open('rb') as f:
            assert hashlib.file_digest(f,'sha256').hexdigest()==metadata['imre_sha256' if name=='imre' else 'runtime_sha256']
    if args.clang is None:parser.error('supply --clang when clang is not on PATH')
    tool=Toolchain(args.pair/'imre',args.pair/'libimre_runtime.a',args.clang,args.opt_level)
    audited={**{'test.test_collections.TestCounter.'+m:v for m,v in COUNTER_OPERATIONS.items()},
             **{'test.test_collections.TestChainMap.'+m:v for m,v in CHAINMAP.items()},
             **{'test.test_deque.TestBasic.'+m:BIG_OPERATION[1] for m in BIG_METHODS},
             'test.test_deque.TestBasic.test_clear':'Clear an existing native deque and measure its length',
             'test.test_deque.TestBasic.test_maxlen_zero':'Zero-length deque construction, extend and extendleft consume the original iterator',
             'test.test_deque.TestBasic.test_init':'Call deque with too many positional arguments and an unexpected keyword'}
    output={}
    counts=collections.Counter()
    for module in manifest['modules']:
        try:suite=load_suite(module)
        except unittest.SkipTest:continue
        for case in cases(suite):
            name=case.id()
            if (name not in audited and not args.all_methods) or isinstance(reference[name],dict) or isinstance(case,doctest.DocTestCase):continue
            cls=type(case)
            rel='Lib/'+cls.__module__.replace('.','/')+'.py'
            if rel not in manifest['files']:raise ValueError((name,rel))
            with tempfile.TemporaryDirectory(prefix='imre-collections-native-default-') as work:
                op=('imre',audited[name]) if name in audited else ('unobserved','Module operation requires a separate audit.')
                path=here/'upstream'/rel
                if cls.__bases__==(unittest.TestCase,) and (case._testMethodName not in own_methods(path).get(cls.__name__,set())):
                    row={'outcome':'blocked','reason':f'{name}: discovery installed the method without its own class-body def in {rel}',
                         'roles':dict.fromkeys(('body','assertions','operation','fixture'),'unobserved'),
                         'evidence':{'stage':'original fixture','source_blob':manifest['files'][rel]}}
                else:
                    row=NativeDefaultMethod(tool,Path(work),path,manifest['files'][rel],op).run(case)
            assert name not in output,name
            output[name]=row;counts[row['outcome']]+=1
            if row['outcome']=='pass' or name in audited:print(args.opt_level,name,row['outcome'],row.get('reason','').splitlines()[-1:] or '',flush=True)
    assert set(audited)<=output.keys()
    result={'schema_version':1,'source':metadata,'opt_level':args.opt_level,'reference_denominator':len(reference),'observations':output}
    args.result.write_text(json.dumps(result,indent=2,sort_keys=True)+'\n')
    print(args.opt_level,dict(counts),flush=True)

if __name__=='__main__':main()
