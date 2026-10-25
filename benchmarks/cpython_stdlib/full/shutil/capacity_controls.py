"""Compare native terminal environment and ioctl precedence, tuple fields and fd errors."""
import fcntl
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import termios

PROGRAM = """import shutil
import sys
from pathlib import Path
with open(sys.argv[1], 'w', encoding='utf-8') as out:
    if sys.argv[2] == 'size':
        t = shutil.get_terminal_size(fallback=(-5, 6))
        out.write(repr(t) + '\\n' + str(t[0]) + '\\n' + str(t.lines) + ' ' + str(t.n_fields) + ' ' + str(t.n_sequence_fields) + ' ' + str(t.n_unnamed_fields) + ' ' + repr(t.__match_args__))
    elif sys.argv[2] == 'usage':
        usage = shutil.disk_usage(sys.argv[3])
        out.write(repr(usage) + '\\n' + str(usage.total == usage[0]) + '\\n' + repr(usage._fields))
    elif sys.argv[2] == 'fd':
        try:
            du = shutil.disk_usage(int(sys.argv[3]))
            out.write(str(du.total) + '\\n' + str(du.used <= du.total))
        except OSError as exc:
            out.write(repr(exc) + '\\n' + str(exc) + '\\n' + repr(exc.args) + '\\n' + repr(exc.errno) + '\\n' + repr(exc.strerror) + '\\n' + repr(exc.filename) + '\\n' + repr(exc.filename2))
    elif sys.argv[2] in ('text', 'bytes', 'path'):
        try:
            if sys.argv[2] == 'text':
                shutil.disk_usage(sys.argv[3])
            elif sys.argv[2] == 'bytes':
                shutil.disk_usage(sys.argv[3].encode())
            else:
                shutil.disk_usage(Path(sys.argv[3]))
        except FileNotFoundError as exc:
            out.write(repr(exc) + '\\n' + str(exc) + '\\n' + repr(exc.args) + '\\n' + repr(exc.errno) + '\\n' + repr(exc.strerror) + '\\n' + repr(exc.filename) + '\\n' + repr(exc.filename2))
    else:
        try:
            invalid = shutil.get_terminal_size(fallback=(2, 4, 6))
            out.write(repr(invalid))
        except TypeError as exc:
            out.write(repr(exc))
"""


def compare(tools, root):
    work = root / 'capacity-controls'
    work.mkdir()
    program = tools.build(PROGRAM.encode(), work)
    far = str(2**190)
    count = 0
    def pair(op, arg, env, stdout, fds=()):
        nonlocal count
        env = {**os.environ, **env}
        for key in ('COLUMNS', 'LINES'):
            if env.get(key) is None:
                env.pop(key, None)
        values = []
        for command in ([sys.executable, work / 'case.py'], [program]):
            with tempfile.NamedTemporaryFile(dir=work) as out:
                result = subprocess.run([*command, out.name, op, str(arg)], stdout=stdout,
                                        stderr=subprocess.PIPE, env=env, pass_fds=fds,
                                        timeout=30, check=True)
                assert not result.stderr, result.stderr
                values.append(out.read())
        assert values[0] == values[1], (op, arg, env.get('COLUMNS'), values)
        count += 1

    master, slave = os.openpty()
    try:
        for rows, columns in ((23, 72), (0, 0), (12, 0)):
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
            for x, y in ((None, None), ('+１_０', ' -3 '), ('nonsense', far), ('_'*30, '0')):
                pair('size', '', {'COLUMNS': x, 'LINES': y}, slave)
        pair('invalid', '', {'COLUMNS': '31', 'LINES': '18'}, subprocess.DEVNULL)
        pair('invalid', '', {'COLUMNS': None, 'LINES': None}, subprocess.DEVNULL)
        pair('size', '', {'COLUMNS': '1'*4301, 'LINES': '+23'}, subprocess.DEVNULL)
        pair('usage', '/sys', {}, subprocess.DEVNULL)
        fd = os.open('/sys', os.O_RDONLY)
        try:
            pair('fd', fd, {}, subprocess.DEVNULL, (fd,))
        finally:
            os.close(fd)
        for fd in (-1, -2, 1000000):
            pair('fd', fd, {}, subprocess.DEVNULL)
        for mode in ('text', 'bytes', 'path'):
            pair(mode, work / 'missing', {}, subprocess.DEVNULL)
    finally:
        os.close(master)
        os.close(slave)
    return count
