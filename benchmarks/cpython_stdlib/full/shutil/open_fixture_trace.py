"""Trace the original copyfile context-manager fixtures on pinned CPython."""
import argparse
from contextlib import chdir
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SHUTIL_BLOB = 'b7608f7edfc93c5d300c2a91fdfa38969df0ded0'
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import run_in_fixtures


def observe(original, module):
    events, objects, errors = [], {}, []
    source = str(Path(module.__file__).resolve())
    functions = {getattr(module.shutil, n).__code__: n for n in
                 ('copyfile', 'copyfileobj', '_fastcopy_sendfile', '_fastcopy_copy_file_range')
                 if hasattr(module.shutil, n)}
    faux = module.TestCopyFile.Faux
    methods = {getattr(faux, n).__code__: 'Faux.' + n for n in ('__enter__', '__exit__', 'read')}

    def describe(error):
        if error is None:
            return None
        for index, existing in enumerate(errors):
            if existing is error:
                break
        else:
            index = len(errors)
            errors.append(error)
        return {'object': index+1, 'type': type(error).__qualname__, 'args': repr(error.args)}

    def trace(frame, event, arg):
        code = frame.f_code
        name = methods.get(code) or functions.get(code)
        if not name and code.co_filename == source and code.co_name == '_open':
            name = '_open'
        if not name:
            return None
        if event not in {'call', 'return', 'exception'}:
            return trace
        row = {'function': name, 'event': event}
        args = frame.f_locals
        if code in methods:
            row['stream'] = objects[id(args['self'])]
        if event == 'call' and name == '_open':
            row.update(path=args['filename'], mode=args['mode'])
        if event == 'call' and name == 'Faux.__exit__':
            row.update(error=describe(args['exc_val']), traceback=args['exc_tb'] is not None)
            assert type(args['exc_val']) is args['exc_type'] or args['exc_type'] is args['exc_val'] is None
        if event == 'exception':
            row['error'] = describe(arg[1])
        if event == 'return':
            if isinstance(arg, faux):
                objects[id(arg)] = args['filename']
                row['result'] = {'stream': args['filename']}
            else:
                row['result'] = repr(arg)
        events.append(row)
        return trace

    previous = sys.gettrace()
    try:
        sys.settrace(trace)
        original()
    finally:
        sys.settrace(previous)
    return {'execution': 'CPython reference only', 'events': events}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--result', type=lambda v: Path(v).resolve(), required=True)
    args = p.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3,14,5):
        p.error('requires test-enabled CPython 3.14.5')
    manifest = json.loads((HERE/'UPSTREAM.json').read_text())
    verify_files(STDLIB/'upstream', manifest['files'])
    verify_interpreter_tests(manifest)
    import test
    test.__path__.insert(0, str(STDLIB/'upstream/Lib/test'))
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        from test import test_shutil
        assert Path(test_shutil.__file__).resolve() == STDLIB/'upstream/Lib/test/test_shutil.py'
        verify_files(Path(test_shutil.shutil.__file__).resolve().parent, {'shutil.py': SHUTIL_BLOB})
        suite = load_suite('test.test_shutil')
        selected = {c.id(): lambda case, original: observe(original, test_shutil)
                    for c in cases(suite) if isinstance(c, test_shutil.TestCopyFile)}
        result, failures, skips = run_in_fixtures(suite, selected)
    rows = {n: records[0] for n, records in result.items() if records}
    rows.update({n:{'failure':detail} for n,detail in failures})
    rows.update({n:{'reference_skip':reason} for n,reason in skips})
    assert len(rows)==len(selected)==4
    args.result.write_text(json.dumps({'cpython_commit':manifest['commit'], 'blobs': {
        **manifest['files'], 'Lib/shutil.py': SHUTIL_BLOB}, 'platform':sys.platform,
        'execution':'CPython reference only; no native assertion or operation', 'cases':rows},indent=2)+'\n')
    return int(bool(failures))


if __name__ == '__main__':
    sys.exit(main())
