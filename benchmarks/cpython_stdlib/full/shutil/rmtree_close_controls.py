"""Check ordinary and cleanup close errors from the real Linux kernel."""
from contextlib import chdir, redirect_stdout
import io
import os
from pathlib import Path
import subprocess
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SOURCE = '''import shutil
import sys
try:
    shutil.rmtree('root', ignore_errors=sys.argv[1] == 'ignore')
    print('removed')
except OSError as error:
    print(repr(error))
    print(str(error))
    print(repr(error.args))
    print(repr(error.errno))
    print(repr(error.filename))
    print(repr(error.filename2))
'''


def compare(tools, work):
    binary = tools.build(SOURCE.encode(), work) if tools is not None else None
    if binary is not None:
        shim = work / 'close.so'
        subprocess.run([tools.clang, '-std=c11', '-shared', '-fPIC', '-Wall', '-Wextra', '-Werror',
                        HERE / 'rmtree_close_fault.c', '-o', shim], check=True)
    reports = {}
    for fault, handling in [('child', 'raise'), ('both', 'raise'), ('both', 'ignore')]:
        def fresh(suffix):
            cwd = work / f'{fault}-{handling}-{suffix}'
            (cwd / 'root/child').mkdir(parents=True)
            return cwd

        ref = fresh('cpython')
        events = []
        original_close = os.close
        def close(fd):
            path = Path(os.readlink(f'/proc/self/fd/{fd}')).relative_to(ref).as_posix()
            assert path in ('root', 'root/child'), path
            events.append(path)
            original_close(fd)
            if fault == 'both' or path == 'root/child':
                original_close(fd)  # the kernel raises EBADF on this same descriptor

        text = io.StringIO()
        with chdir(ref), patch('sys.argv', ['case.py', handling]), patch('os.close', close), redirect_stdout(text):
            exec(compile(SOURCE, 'rmtree-close-case.py', 'exec'), {})
        def observed(cwd, output, calls):
            return {'stdout': output.splitlines(), 'closes': calls,
                    'root_exists': (cwd/'root').exists(), 'child_exists': (cwd/'root/child').exists()}
        expected = observed(ref, text.getvalue(), events)
        assert events == ['root/child', 'root'] and expected['root_exists'] == (handling != 'ignore')
        if handling != 'ignore':
            assert expected['stdout'][-2:] == [repr('root/child' if fault == 'child' else None), 'None']
        if binary is not None:
            cwd = fresh('native')
            with (cwd/'close.log').open('w+') as log:
                env = {**os.environ, 'LD_PRELOAD': str(shim), 'IMRE_SHUTIL_CLOSE_ROOT': str(cwd/'root'),
                       'IMRE_SHUTIL_CLOSE_FAULT': fault, 'IMRE_SHUTIL_CLOSE_LOG_FD': str(log.fileno())}
                proc = subprocess.run([binary, handling], cwd=cwd, env=env, capture_output=True,
                                      text=True, pass_fds=(log.fileno(),), timeout=30)
                assert proc.returncode == 0 and not proc.stderr, (proc.returncode, proc.stderr)
                log.seek(0)
                actual = observed(cwd, proc.stdout, log.read().splitlines())
            assert actual == expected, (fault, handling, expected, actual)
        reports[f'{fault}-{handling}'] = expected
    return reports
