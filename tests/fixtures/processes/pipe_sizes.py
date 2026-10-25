import subprocess
import warnings
from sys import argv, platform

# The child only queries the three file descriptors supplied by the parent.
observer = """import fcntl, os, sys
if hasattr(fcntl, 'F_GETPIPE_SZ'):
    r, w = os.pipe()
    try:
        default = fcntl.fcntl(w, fcntl.F_GETPIPE_SZ)
    finally:
        os.close(r); os.close(w)
    expected = os.sysconf('SC_PAGESIZE') if sys.argv[1] == 'page' else default
    actual = [fcntl.fcntl(fd, fcntl.F_GETPIPE_SZ) for fd in (0, 1, 2)]
    assert actual == [expected] * 3, (expected, actual)
assert sys.stdin.buffer.read() == b'pear'
sys.stdout.buffer.write(b'out')
sys.stderr.buffer.write(b'error!')
"""


def check(value: int | None, mode: str) -> None:
    with subprocess.Popen([argv[1], '-c', observer, mode], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          pipesize=value) as child:
        out, err = child.communicate(b'pear')
        assert out == b'out' and err == b'error!' and child.wait() == 0


check(None, 'default')
check(-2, 'default')
check(0, 'default')
check(1, 'page')
# fcntl's integer argument takes the low C unsigned-int bits after Popen's
# original positive-Python-integer test. A wrapped zero still resizes the pipe.
check(1267650600228229401496703205376, 'page')
check(-1267650600228229401496703205376, 'default')

# Extra launch settings must not shift either the pipesize word or the final
# restore_signals operand. bool is an int for Popen's validation.
with subprocess.Popen([argv[1], '-c', observer, 'page'], stdin=subprocess.PIPE,
                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, pipesize=True,
                      pass_fds=(1,), restore_signals=False, process_group=0) as child:
    out, err = child.communicate(b'pear')
    assert out == b'out' and err == b'error!' and child.wait() == 0

# Validation does not require a PIPE. A valid but impractically large size
# does not affect inherited standard streams or stop this child launching.
assert subprocess.call(['/bin/sh', '-c', 'exit 23'], pipesize=2147483647) == 23
if platform == 'linux':
    # A pipe setup error has no executable filename, even with a missing command.
    try:
        subprocess.Popen(['/nonexistent-after-pipe-allocation'], stdout=subprocess.PIPE,
                         pipesize=4294967295)
    except OSError as exc:
        assert exc.filename is None
        code = exc.errno
        if code is None:
            raise AssertionError('missing pipe setup errno')
        print(code)
    else:
        raise AssertionError('kernel accepted a 4 GiB pipe')
try:
    subprocess.Popen(['/nonexistent-before-pipesize-validation'], pipesize='invalid')
except TypeError as exc:
    assert str(exc) == 'pipesize must be an integer'
else:
    raise AssertionError('invalid pipesize did not raise')
# stdout is validated before pipesize; pipesize is validated before the descriptor warning.
warnings.simplefilter('error', RuntimeWarning)
try:
    subprocess.Popen(['/nonexistent-before-pipesize-warning'], pipesize='invalid',
                     pass_fds=(1,), close_fds=False)
except TypeError as exc:
    assert str(exc) == 'pipesize must be an integer'
else:
    raise AssertionError('invalid pipesize did not precede the descriptor warning')
try:
    subprocess.Popen(['/nonexistent-before-pipesize-stdout'], pipesize='invalid',
                     stdout=subprocess.STDOUT)
except ValueError as exc:
    assert str(exc) == 'STDOUT can only be used for stderr'
else:
    raise AssertionError('invalid stdout did not precede pipesize validation')

print('checked kernel pipe capacities and data on all three streams')
